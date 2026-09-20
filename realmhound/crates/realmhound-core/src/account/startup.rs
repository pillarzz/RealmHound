//! Selected-profile startup resolution.
//!
//! [`StartupResolver`] is the single entry point that decides how RealmHound
//! starts. It reconciles the account registry, runs the recoverable flat-layout
//! migration before touching any account resource, reconciles again, and then
//! resolves exactly one of three outcomes:
//!
//! - [`StartupResolution::Selected`]: one fully validated, exclusively locked
//!   profile exposed through a single immutable [`AccountContext`].
//! - [`StartupResolution::Discovery`]: no account resource is opened; only the
//!   bounded identity-discovery state is permitted.
//! - [`StartupResolution::Recovery`]: a redacted, user-facing error for a
//!   missing, damaged, mismatched, future-version, locked, or blocked startup.
//!
//! There is never a fallback to legacy flat paths: a blocked or failed migration
//! resolves to recovery, and a corrupt or missing profile never becomes empty
//! replacement state. The resolver itself legitimately reads the registry and
//! manifests, and migration may use credentials for a known migration; the
//! zero-open guarantee applies to the resolved discovery/recovery outcomes.

use std::io;
use std::sync::Arc;

use chrono::{DateTime, Utc};

use super::migration::backup::record_validated_launch;
use super::migration::journal::RecoveryStatus;
use super::migration::{FlatLayoutMigration, MigrationError, MigrationOutcome};
use super::paths::PROFILE_VERSION;
use super::{
    credential_target, AccountError, AccountId, AccountKey, AccountPaths, AccountPersistencePaths,
    AccountRegistry, AccountRegistryEntry, AccountRegistryStore, CredentialStore, ProfileLock,
    ProfileLockError, ProfileManifest, ProfileSchemas, ReconciliationReport, RegistryMode,
    SavedCredential,
};
use crate::settings::{Settings, SettingsLoadError};
use crate::storage::{load_json, LoadOutcome, StorageError, StorageRoot};
use crate::vault::AccountData;

/// Relative path of the global installation settings document.
const SETTINGS_FILE: &str = "settings.json";

/// The global installation settings loaded from the explicit root, paired with
/// the resolved startup mode.
///
/// Startup loads settings once, from the same explicit root the resolver owns,
/// so `main` never performs a second default-root load. On a damaged settings
/// document the settings fall back to defaults (for window bootstrapping only)
/// and the resolution is [`StartupResolution::Recovery`].
pub struct StartupOutcome {
    /// Global installation settings loaded from the explicit root.
    pub settings: Settings,
    /// The resolved startup mode.
    pub resolution: StartupResolution,
}

/// The three mutually exclusive ways RealmHound can start.
pub enum StartupResolution {
    /// One validated profile is selected and exclusively locked.
    Selected(Box<SelectedStartup>),
    /// No profile is selected; only bounded identity discovery is permitted.
    Discovery(DiscoveryStartup),
    /// A recoverable error prevents opening a profile; the user may retry.
    Recovery(RecoveryStartup),
}

/// A resolved selected-profile startup.
///
/// Holds the single immutable [`AccountContext`] plus the reconciled registry
/// snapshot the UI uses for display name and migrated client timestamps.
pub struct SelectedStartup {
    context: AccountContext,
    registry: AccountRegistry,
    recovery: RecoveryStatus,
}

impl SelectedStartup {
    /// The immutable account context for the process lifetime.
    pub fn context(&self) -> &AccountContext {
        &self.context
    }

    /// Consume the startup, returning the owned account context.
    pub fn into_context(self) -> AccountContext {
        self.context
    }

    /// The reconciled registry entry for the selected account.
    pub fn selected_entry(&self) -> Option<&AccountRegistryEntry> {
        self.registry.entry(self.context.account_key())
    }

    /// The reconciled registry snapshot.
    pub fn registry(&self) -> &AccountRegistry {
        &self.registry
    }

    /// Migration recovery status for follow-up backup actions.
    pub fn recovery(&self) -> RecoveryStatus {
        self.recovery
    }
}

/// A resolved discovery startup. Opens no account-scoped resource.
pub struct DiscoveryStartup {
    registry_store: AccountRegistryStore,
    registry: AccountRegistry,
}

impl DiscoveryStartup {
    /// The account restored if discovery exits without verifying an account.
    pub fn previous_account_key(&self) -> Option<AccountKey> {
        self.registry.previous_account_key()
    }

    /// The reconciled registry snapshot.
    pub fn registry(&self) -> &AccountRegistry {
        &self.registry
    }

    /// Restore the previously selected account when discovery exits without
    /// verifying an account. Called on a clean exit that produced no new
    /// verified identity.
    pub fn cancel_discovery(&self, now: DateTime<Utc>) -> Result<AccountRegistry, AccountError> {
        self.registry_store.cancel_discovery(now)
    }

    /// Register a verified account and select it for the next startup.
    pub fn register_and_select(
        &self,
        account_id: AccountId,
        display_name: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<AccountKey, AccountError> {
        let entry = self
            .registry_store
            .register_verified(account_id, display_name, now)?;
        let key = entry.key();
        self.registry_store.select(key, now)?;
        Ok(key)
    }

    /// Access the underlying registry store for credential operations.
    pub fn registry_store(&self) -> &AccountRegistryStore {
        &self.registry_store
    }
}

/// A resolved recoverable-error startup.
pub struct RecoveryStartup {
    error: StartupError,
}

impl RecoveryStartup {
    /// The redacted, user-facing error describing why startup could not proceed.
    pub fn error(&self) -> &StartupError {
        &self.error
    }

    /// Consume the recovery, returning the owned error.
    pub fn into_error(self) -> StartupError {
        self.error
    }
}

/// Classified startup failure. Every message is redacted and user-facing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupError {
    /// Migration could not proceed; no legacy source was changed.
    MigrationBlocked(String),
    /// Migration failed with a redacted reason.
    MigrationFailed(String),
    /// The selected registry entry has no profile on disk.
    MissingProfile,
    /// The selected profile document or manifest is damaged or unreadable.
    DamagedProfile(String),
    /// The manifest identity does not match the registry binding.
    ManifestMismatch,
    /// The profile manifest or schema version is newer than supported.
    UnsupportedVersion,
    /// A reconciliation health issue affects the selected profile.
    ProfileUnhealthy(String),
    /// The selected profile is locked by another instance.
    Locked,
    /// The migration outcome and committed registry disagree.
    InconsistentState,
    /// The global installation settings are present but damaged.
    DamagedSettings(String),
    /// A validated storage operation failed.
    Storage(String),
}

impl StartupError {
    fn from_migration(error: &MigrationError) -> Self {
        StartupError::MigrationFailed(redact_migration(error))
    }

    fn from_account(error: AccountError) -> Self {
        match error {
            AccountError::Storage(storage) => StartupError::Storage(storage.to_string()),
            AccountError::UnsupportedRegistryVersion(_)
            | AccountError::UnsupportedManifestVersion { .. } => StartupError::UnsupportedVersion,
            AccountError::ManifestMismatch(_) => StartupError::ManifestMismatch,
            AccountError::AccountNotFound(_) => StartupError::MissingProfile,
            other => StartupError::DamagedProfile(other.to_string()),
        }
    }
}

impl std::fmt::Display for StartupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartupError::MigrationBlocked(reason) => {
                write!(f, "Account migration is blocked: {reason}")
            }
            StartupError::MigrationFailed(reason) => {
                write!(f, "Account migration could not finish: {reason}")
            }
            StartupError::MissingProfile => {
                write!(f, "The selected account profile is missing.")
            }
            StartupError::DamagedProfile(detail) => {
                write!(f, "The selected account profile is damaged: {detail}")
            }
            StartupError::ManifestMismatch => write!(
                f,
                "The selected account profile identity does not match its registry entry."
            ),
            StartupError::UnsupportedVersion => write!(
                f,
                "The selected account profile was created by a newer version of RealmHound."
            ),
            StartupError::ProfileUnhealthy(detail) => {
                write!(f, "The selected account profile needs recovery: {detail}")
            }
            StartupError::Locked => write!(
                f,
                "The selected account profile is already open in another RealmHound instance."
            ),
            StartupError::InconsistentState => write!(
                f,
                "The account startup state is inconsistent and needs recovery."
            ),
            StartupError::DamagedSettings(detail) => {
                write!(f, "RealmHound settings could not be read: {detail}")
            }
            StartupError::Storage(detail) => write!(f, "Account storage error: {detail}"),
        }
    }
}

/// Redact a migration error down to its variant category so no path fragment or
/// token-adjacent detail reaches the user surface.
fn redact_migration(error: &MigrationError) -> String {
    match error {
        MigrationError::LockContended { .. } => "another instance owns migration".to_string(),
        MigrationError::TokenMalformed => "the saved sign-in token is malformed".to_string(),
        MigrationError::JournalVersionUnsupported { .. } => {
            "the migration record is a newer version".to_string()
        }
        MigrationError::JournalCorrupt { .. } | MigrationError::JournalMismatch { .. } => {
            "the migration record is inconsistent".to_string()
        }
        MigrationError::DatabaseVersionUnsupported { .. } => {
            "a history database is a newer version".to_string()
        }
        _ => "the migration could not complete".to_string(),
    }
}

/// One immutable set of resources bound to one selected profile for the lifetime
/// of the process.
///
/// The context owns the held [`ProfileLock`], so every account-scoped reader,
/// writer, cache, import, log, and credential access derives from the same
/// exclusively locked profile. It is shared through `Arc<AccountContext>` between
/// the UI and the worker; selection is immutable for a normal process lifetime.
pub struct AccountContext {
    local: StorageRoot,
    account_key: AccountKey,
    account_id: AccountId,
    display_name: Option<String>,
    credential_target: String,
    paths: AccountPaths,
    persistence: AccountPersistencePaths,
    credentials: Arc<dyn CredentialStore>,
    // Held for the process lifetime; released only when the context drops.
    _lock: ProfileLock,
}

impl std::fmt::Debug for AccountContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountContext")
            .field("account_key", &self.account_key)
            .field("account_id", &self.account_id)
            .field("display_name", &self.display_name)
            .finish_non_exhaustive()
    }
}

impl AccountContext {
    /// The selected local account key (immutable startup identity).
    pub fn account_key(&self) -> AccountKey {
        self.account_key
    }

    /// The verified server account identity read from the profile manifest.
    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// The latest known display name from the registry, if any.
    pub fn display_name(&self) -> Option<&str> {
        self.display_name.as_deref()
    }

    /// The deterministic credential-store target for the selected profile.
    pub fn credential_target(&self) -> &str {
        &self.credential_target
    }

    /// The account-scoped persistence paths for every profile resource.
    pub fn persistence(&self) -> &AccountPersistencePaths {
        &self.persistence
    }

    /// The validated profile paths.
    pub fn paths(&self) -> &AccountPaths {
        &self.paths
    }

    /// The storage root that owns this profile and the global registry.
    pub fn storage_root(&self) -> &StorageRoot {
        &self.local
    }

    /// Strictly load the account snapshot for this profile.
    ///
    /// A corrupt snapshot fails and is never replaced with empty state; a
    /// legitimately missing snapshot (opened after the lock) initializes empty.
    pub fn load_account_data_strict(&self) -> io::Result<AccountData> {
        self.persistence.account_data_repository().load_strict()
    }

    /// Read the selected profile credential through the credential store.
    ///
    /// Never blocks startup: a store error is a redacted warning and means "no
    /// token", and the plaintext `access_token.txt` is never consulted.
    pub fn read_credential(&self) -> Option<SavedCredential> {
        match self.credentials.read(&self.credential_target) {
            Ok(credential) => credential,
            Err(error) => {
                tracing::warn!(
                    "[ACCOUNT] Credential read unavailable ({error}); continuing without a token"
                );
                None
            }
        }
    }

    /// The shared credential store bound to this process.
    pub fn credentials(&self) -> &Arc<dyn CredentialStore> {
        &self.credentials
    }

    /// Record exactly one validated launch of this migrated profile.
    ///
    /// Called only after the complete profile has opened successfully. A no-op
    /// for a non-migrated, unrelated, unknown, or incomplete migration journal.
    pub fn record_validated_launch(&self) -> Result<(), MigrationError> {
        record_validated_launch(&self.local, self.account_key)
    }
}

/// Resolver that decides the startup mode for one explicit storage root.
///
/// Never resolves a real `%LOCALAPPDATA%`: the local root, optional Roaming
/// root, and credential store are all injected.
pub struct StartupResolver {
    local: StorageRoot,
    roaming: Option<StorageRoot>,
    credentials: Arc<dyn CredentialStore>,
    registry_store: AccountRegistryStore,
    /// When set, selected-profile startup waits this long for a contended lock
    /// instead of failing to Recovery (used by a relaunch replacement).
    relaunch_lock_wait: Option<std::time::Duration>,
}

impl StartupResolver {
    /// Create a resolver over an explicit local root and optional Roaming root.
    pub fn new(
        local: StorageRoot,
        roaming: Option<StorageRoot>,
        credentials: Arc<dyn CredentialStore>,
    ) -> Self {
        let registry_store = AccountRegistryStore::new(local.clone());
        Self {
            local,
            roaming,
            credentials,
            registry_store,
            relaunch_lock_wait: None,
        }
    }

    /// Wait this long for a contended profile lock during selected startup
    /// instead of failing to Recovery (used by a relaunch replacement).
    pub fn with_relaunch_lock_wait(mut self, timeout: std::time::Duration) -> Self {
        self.relaunch_lock_wait = Some(timeout);
        self
    }

    /// Resolve the startup mode using the current time.
    pub fn resolve(&self) -> StartupResolution {
        self.resolve_at(Utc::now())
    }

    /// Cheap, lock-free check of whether the flat-layout migration still has
    /// work to do, used to decide whether to surface the migration UI before
    /// the authoritative resolve runs.
    pub fn migration_pending(&self) -> bool {
        FlatLayoutMigration::new(
            self.local.clone(),
            self.roaming.clone(),
            self.credentials.clone(),
        )
        .pending()
    }

    /// Load the global installation settings, then resolve the startup mode.
    ///
    /// Settings are read first, from the resolver's explicit root and without
    /// ever relocating a corrupt document, so `main` never issues a second
    /// default-root load and a corrupt settings file surfaces recovery while
    /// staying in place. A damaged document short-circuits to recovery before
    /// the registry is reconciled or the migration runs.
    pub fn resolve_startup(&self) -> StartupOutcome {
        self.resolve_startup_at(Utc::now())
    }

    /// [`Self::resolve_startup`] at an explicit time (for tests).
    pub fn resolve_startup_at(&self, now: DateTime<Utc>) -> StartupOutcome {
        match self.load_settings() {
            Ok(settings) => StartupOutcome {
                settings,
                resolution: self.resolve_at(now),
            },
            Err(error) => StartupOutcome {
                settings: Settings::default(),
                resolution: recovery(error),
            },
        }
    }

    /// Load global settings from the explicit root without relocating a corrupt
    /// document. A missing file yields defaults; a malformed file is recovery.
    pub fn load_settings(&self) -> Result<Settings, StartupError> {
        let path = self
            .local
            .ensure_no_links(SETTINGS_FILE)
            .map_err(|error| StartupError::Storage(error.to_string()))?;
        Settings::load_explicit(&path).map_err(|error| match error {
            SettingsLoadError::Parse(_) => StartupError::DamagedSettings(error.to_string()),
            SettingsLoadError::Read(_) => StartupError::Storage(error.to_string()),
        })
    }

    /// Resolve the startup mode at an explicit time (for tests).
    pub fn resolve_at(&self, now: DateTime<Utc>) -> StartupResolution {
        // Reconcile the registry before any account resource opens.
        if let Err(error) = self.registry_store.reconcile() {
            return recovery(StartupError::from_account(error));
        }

        // Run (or resume) the recoverable migration before touching a profile.
        let migration = FlatLayoutMigration::new(
            self.local.clone(),
            self.roaming.clone(),
            self.credentials.clone(),
        );
        let outcome = match migration.run_at(now) {
            Ok(outcome) => outcome,
            Err(error) => return recovery(StartupError::from_migration(&error)),
        };
        match &outcome {
            MigrationOutcome::ReadyKnown { account_key, .. } => {
                tracing::info!("[STARTUP] Migration ready: known account (key={account_key})")
            }
            MigrationOutcome::AlreadyComplete { .. } => {}
            MigrationOutcome::AwaitingAttribution { .. } => {
                tracing::info!("[STARTUP] Migration awaiting attribution, entering discovery")
            }
            MigrationOutcome::Blocked { reason } => {
                tracing::warn!("[STARTUP] Migration blocked: {reason}")
            }
        }
        if let MigrationOutcome::Blocked { reason } = outcome {
            return recovery(StartupError::MigrationBlocked(reason));
        }

        // A known migration commits a selection; carry the recovery status and
        // the expected key so the committed registry must agree.
        let (expected_key, recovery_status) = match &outcome {
            MigrationOutcome::ReadyKnown {
                account_key,
                recovery,
                ..
            } => (Some(*account_key), *recovery),
            _ => (None, RecoveryStatus::Healthy),
        };

        // Reconcile again now that migration has reached a safe outcome.
        let reconciled = match self.registry_store.reconcile() {
            Ok(reconciled) => reconciled,
            Err(error) => return recovery(StartupError::from_account(error)),
        };
        let registry = reconciled.registry;
        let report = reconciled.report;

        match registry.mode() {
            RegistryMode::Selected => {
                let Some(key) = registry.selected_account_key() else {
                    return recovery(StartupError::InconsistentState);
                };
                if matches!(expected_key, Some(expected) if expected != key) {
                    return recovery(StartupError::InconsistentState);
                }
                self.open_selected(registry, report, key, recovery_status)
            }
            RegistryMode::Discovering => {
                // A known migration that committed a selection cannot resolve to
                // discovery.
                if expected_key.is_some() {
                    return recovery(StartupError::InconsistentState);
                }
                StartupResolution::Discovery(DiscoveryStartup {
                    registry_store: self.registry_store.clone(),
                    registry,
                })
            }
        }
    }

    /// Validate and exclusively lock the selected profile, then build the single
    /// immutable account context. Fails closed to recovery on any problem.
    fn open_selected(
        &self,
        registry: AccountRegistry,
        report: ReconciliationReport,
        key: AccountKey,
        recovery_status: RecoveryStatus,
    ) -> StartupResolution {
        let Some(entry) = registry.entry(key) else {
            return recovery(StartupError::MissingProfile);
        };
        let entry = entry.clone();
        let paths = AccountPaths::new(self.local.clone(), key);

        // A reconciliation health issue affecting the selected profile is fatal,
        // never merely listed.
        if let Some(detail) = fatal_health_issue(&report, &paths, key) {
            return recovery(StartupError::ProfileUnhealthy(detail));
        }

        // Validate the profile directory, manifest, version, schema, key, and
        // account-id binding before opening any account data.
        let manifest = match self.load_selected_manifest(&paths) {
            Ok(Some(manifest)) => manifest,
            Ok(None) => return recovery(StartupError::MissingProfile),
            Err(error) => return recovery(error),
        };
        if manifest.version() != PROFILE_VERSION {
            return recovery(StartupError::UnsupportedVersion);
        }
        if manifest.account_key() != key {
            return recovery(StartupError::ManifestMismatch);
        }
        if manifest.account_id() != entry.account_id() {
            return recovery(StartupError::ManifestMismatch);
        }
        if !schemas_supported(manifest.schemas()) {
            return recovery(StartupError::UnsupportedVersion);
        }

        // Lock the profile before any account resource; a relaunch replacement
        // waits out the previous instance instead of failing to Recovery.
        let lock_result = match self.relaunch_lock_wait {
            Some(timeout) => ProfileLock::acquire_blocking(&self.local, key, timeout),
            None => ProfileLock::acquire(&self.local, key),
        };
        let lock = match lock_result {
            Ok(lock) => lock,
            Err(ProfileLockError::Contended { .. }) => return recovery(StartupError::Locked),
            Err(ProfileLockError::Storage(error)) => {
                return recovery(StartupError::Storage(error.to_string()))
            }
            Err(ProfileLockError::Io { source, .. }) => {
                return recovery(StartupError::Storage(source.to_string()))
            }
        };

        let persistence = match AccountPersistencePaths::for_profile(&paths) {
            Ok(persistence) => persistence,
            Err(error) => return recovery(StartupError::Storage(error.to_string())),
        };

        let context = AccountContext {
            local: self.local.clone(),
            account_key: key,
            account_id: manifest.account_id().clone(),
            display_name: entry.display_name().map(str::to_string),
            credential_target: credential_target(key),
            paths,
            persistence,
            credentials: self.credentials.clone(),
            _lock: lock,
        };

        tracing::info!(
            "[STARTUP] Opened selected profile: {}",
            context.display_name.as_deref().unwrap_or("-")
        );
        StartupResolution::Selected(Box::new(SelectedStartup {
            context,
            registry,
            recovery: recovery_status,
        }))
    }

    /// Load the selected manifest strictly, mapping storage errors to redacted
    /// startup errors. Returns `Ok(None)` when the manifest is absent.
    fn load_selected_manifest(
        &self,
        paths: &AccountPaths,
    ) -> Result<Option<ProfileManifest>, StartupError> {
        let manifest_relative = paths.relative_manifest();
        match load_json::<ProfileManifest>(&self.local, manifest_relative) {
            Ok(LoadOutcome::Primary(manifest)) | Ok(LoadOutcome::RecoveredBackup(manifest)) => {
                Ok(Some(manifest))
            }
            Ok(LoadOutcome::Missing) => Ok(None),
            Err(error @ StorageError::InvalidDocument { .. })
            | Err(error @ StorageError::InvalidDocuments { .. }) => {
                Err(StartupError::DamagedProfile(error.to_string()))
            }
            Err(error) => Err(StartupError::Storage(error.to_string())),
        }
    }
}

/// Whether every profile schema version is supported (not newer than current).
fn schemas_supported(schemas: ProfileSchemas) -> bool {
    let supported = ProfileSchemas::default();
    schemas.account_data <= supported.account_data
        && schemas.loot_database <= supported.loot_database
        && schemas.combat_database <= supported.combat_database
}

/// Return the redacted detail of a reconciliation health issue that affects the
/// selected profile, if any. Such issues are fatal for the selected profile.
fn fatal_health_issue(
    report: &ReconciliationReport,
    paths: &AccountPaths,
    key: AccountKey,
) -> Option<String> {
    use super::ProfileHealthKind;
    let profile_root = paths.relative_root();
    for issue in &report.health_issues {
        let path_matches = issue.path.starts_with(&profile_root);
        let key_matches = match &issue.kind {
            ProfileHealthKind::MissingRegisteredProfile(k)
            | ProfileHealthKind::MissingRegistryReference(k) => *k == key,
            ProfileHealthKind::ManifestKeyMismatch { directory_key, .. } => *directory_key == key,
            _ => false,
        };
        if path_matches || key_matches {
            return Some(health_kind_label(&issue.kind));
        }
    }
    None
}

/// A redacted, user-facing label for a profile health problem.
fn health_kind_label(kind: &super::ProfileHealthKind) -> String {
    use super::ProfileHealthKind;
    match kind {
        ProfileHealthKind::MissingManifest => "the profile manifest is missing".to_string(),
        ProfileHealthKind::InvalidManifest(_) => "the profile manifest is unreadable".to_string(),
        ProfileHealthKind::UnsupportedManifestVersion(_) => {
            "the profile manifest is a newer version".to_string()
        }
        ProfileHealthKind::ManifestKeyMismatch { .. } => {
            "the profile identity does not match its directory".to_string()
        }
        ProfileHealthKind::RegistryManifestMismatch { .. } => {
            "the profile identity does not match its registry entry".to_string()
        }
        ProfileHealthKind::MissingRegisteredProfile(_) => {
            "the profile is missing on disk".to_string()
        }
        ProfileHealthKind::MissingRegistryReference(_) => {
            "the profile registry reference is broken".to_string()
        }
        _ => "the profile needs recovery".to_string(),
    }
}

fn recovery(error: StartupError) -> StartupResolution {
    StartupResolution::Recovery(RecoveryStartup { error })
}

#[cfg(test)]
mod tests;

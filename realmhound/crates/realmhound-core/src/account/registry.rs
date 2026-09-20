use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::paths::PROFILE_VERSION;
use super::{AccountId, AccountKey, AccountPaths, ProfileManifest};
use crate::storage::{
    load_json, write_json_atomic, BackupPolicy, LoadOutcome, StorageError, StorageRoot,
};

const REGISTRY_VERSION: u32 = 1;
const REGISTRY_PATH: &str = "accounts.json";

/// Committed account mode stored in the global registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistryMode {
    /// Load one selected profile.
    Selected,
    /// Load no profile while identifying an account.
    Discovering,
}

/// Reason for an account registry transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionReason {
    /// Switch to another known account.
    AccountSwitch,
    /// Restart without a profile to identify an account.
    AccountDiscovery,
}

/// Persisted intent that has not replaced the committed registry state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingTransition {
    target_mode: RegistryMode,
    target_account_key: Option<AccountKey>,
    reason: TransitionReason,
    started_at: DateTime<Utc>,
}

impl PendingTransition {
    /// Create a pending transition to a selected profile.
    pub fn selected(account_key: AccountKey, started_at: DateTime<Utc>) -> Self {
        Self {
            target_mode: RegistryMode::Selected,
            target_account_key: Some(account_key),
            reason: TransitionReason::AccountSwitch,
            started_at,
        }
    }

    /// Create a pending transition to discovery mode.
    pub fn discovering(started_at: DateTime<Utc>) -> Self {
        Self {
            target_mode: RegistryMode::Discovering,
            target_account_key: None,
            reason: TransitionReason::AccountDiscovery,
            started_at,
        }
    }

    /// Return the intended mode.
    pub fn target_mode(&self) -> RegistryMode {
        self.target_mode
    }

    /// Return the intended account key, if any.
    pub fn target_account_key(&self) -> Option<AccountKey> {
        self.target_account_key
    }

    /// Return the transition reason.
    pub fn reason(&self) -> TransitionReason {
        self.reason
    }

    /// Return when the transition started.
    pub fn started_at(&self) -> DateTime<Utc> {
        self.started_at
    }

    fn validate(&self) -> Result<(), AccountError> {
        let valid = matches!(
            (self.target_mode, self.target_account_key, self.reason),
            (
                RegistryMode::Selected,
                Some(_),
                TransitionReason::AccountSwitch
            ) | (
                RegistryMode::Discovering,
                None,
                TransitionReason::AccountDiscovery
            )
        );
        if valid {
            Ok(())
        } else {
            Err(AccountError::InvalidRegistryState(
                "pending transition target, key, and reason disagree".to_string(),
            ))
        }
    }
}

/// Mutable account metadata stored in the global registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountRegistryEntry {
    key: AccountKey,
    account_id: AccountId,
    display_name: Option<String>,
    created_at: DateTime<Utc>,
    last_verified_at: Option<DateTime<Utc>>,
    last_opened_at: Option<DateTime<Utc>>,
    /// Reference to this account's credential-store target. Never the token
    /// itself. `serde(default)` keeps pre-credential registry files loadable.
    #[serde(default)]
    credential_target: Option<String>,
    /// Unix seconds of the last time the game client was observed contacting the
    /// server, migrated from legacy settings. `serde(default)` keeps older
    /// registry files loadable.
    #[serde(default)]
    last_client_seen_unix: i64,
    /// Unix seconds of the last confirmed game-client launch, migrated from
    /// legacy settings. `serde(default)` keeps older registry files loadable.
    #[serde(default)]
    last_client_launch_unix: i64,
}

impl AccountRegistryEntry {
    fn verified(
        key: AccountKey,
        account_id: AccountId,
        display_name: Option<String>,
        verified_at: DateTime<Utc>,
    ) -> Self {
        Self {
            key,
            account_id,
            display_name,
            created_at: verified_at,
            last_verified_at: Some(verified_at),
            last_opened_at: None,
            credential_target: None,
            last_client_seen_unix: 0,
            last_client_launch_unix: 0,
        }
    }

    fn orphan(manifest: &ProfileManifest) -> Self {
        Self {
            key: manifest.account_key(),
            account_id: manifest.account_id().clone(),
            display_name: None,
            created_at: manifest.created_at(),
            last_verified_at: None,
            last_opened_at: None,
            credential_target: None,
            last_client_seen_unix: 0,
            last_client_launch_unix: 0,
        }
    }

    /// Return the local account key.
    pub fn key(&self) -> AccountKey {
        self.key
    }

    /// Return the authoritative server account ID copied from the manifest.
    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Return the latest verified display name.
    pub fn display_name(&self) -> Option<&str> {
        self.display_name.as_deref()
    }

    /// Return when the profile was created.
    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    /// Return the last identity verification time.
    pub fn last_verified_at(&self) -> Option<DateTime<Utc>> {
        self.last_verified_at
    }

    /// Return the last profile-open time.
    pub fn last_opened_at(&self) -> Option<DateTime<Utc>> {
        self.last_opened_at
    }

    /// Return this account's credential-store target reference, if bound. This
    /// is never the token itself.
    pub fn credential_target(&self) -> Option<&str> {
        self.credential_target.as_deref()
    }

    /// Return the migrated last-client-seen Unix seconds (0 when unknown).
    pub fn last_client_seen_unix(&self) -> i64 {
        self.last_client_seen_unix
    }

    /// Return the migrated last-client-launch Unix seconds (0 when unknown).
    pub fn last_client_launch_unix(&self) -> i64 {
        self.last_client_launch_unix
    }
}

/// Versioned global account registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountRegistry {
    version: u32,
    mode: RegistryMode,
    selected_account_key: Option<AccountKey>,
    previous_account_key: Option<AccountKey>,
    pending_transition: Option<PendingTransition>,
    accounts: Vec<AccountRegistryEntry>,
}

impl AccountRegistry {
    /// Create an empty registry in discovery mode.
    pub fn new() -> Self {
        Self {
            version: REGISTRY_VERSION,
            mode: RegistryMode::Discovering,
            selected_account_key: None,
            previous_account_key: None,
            pending_transition: None,
            accounts: Vec::new(),
        }
    }

    /// Return the registry document version.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Return the committed registry mode.
    pub fn mode(&self) -> RegistryMode {
        self.mode
    }

    /// Return the committed selected account key.
    pub fn selected_account_key(&self) -> Option<AccountKey> {
        self.selected_account_key
    }

    /// Return the account restored when discovery is cancelled.
    pub fn previous_account_key(&self) -> Option<AccountKey> {
        self.previous_account_key
    }

    /// Return the uncommitted transition intent.
    pub fn pending_transition(&self) -> Option<&PendingTransition> {
        self.pending_transition.as_ref()
    }

    /// Return all registered accounts.
    pub fn accounts(&self) -> &[AccountRegistryEntry] {
        &self.accounts
    }

    /// Find an entry by local account key.
    pub fn entry(&self, key: AccountKey) -> Option<&AccountRegistryEntry> {
        self.accounts.iter().find(|entry| entry.key == key)
    }

    /// Find an entry by normalized server account ID.
    pub fn find_by_account_id(&self, account_id: &AccountId) -> Option<&AccountRegistryEntry> {
        self.accounts
            .iter()
            .find(|entry| entry.account_id == *account_id)
    }

    fn validate_intrinsic(&self) -> Result<(), AccountError> {
        if self.version != REGISTRY_VERSION {
            return Err(AccountError::UnsupportedRegistryVersion(self.version));
        }
        match self.mode {
            RegistryMode::Selected if self.selected_account_key.is_none() => {
                return Err(AccountError::InvalidRegistryState(
                    "selected mode requires selected_account_key".to_string(),
                ));
            }
            RegistryMode::Selected if self.previous_account_key.is_some() => {
                return Err(AccountError::InvalidRegistryState(
                    "selected mode cannot retain previous_account_key".to_string(),
                ));
            }
            RegistryMode::Discovering if self.selected_account_key.is_some() => {
                return Err(AccountError::InvalidRegistryState(
                    "discovering mode cannot select an account".to_string(),
                ));
            }
            _ => {}
        }
        if let Some(pending) = &self.pending_transition {
            pending.validate()?;
        }

        let mut keys = HashSet::new();
        let mut account_ids = HashSet::new();
        for entry in &self.accounts {
            if !keys.insert(entry.key) {
                return Err(AccountError::DuplicateRegistryKey(entry.key));
            }
            if !account_ids.insert(entry.account_id.clone()) {
                return Err(AccountError::DuplicateAccountId(entry.account_id.clone()));
            }
        }
        Ok(())
    }
}

impl Default for AccountRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// A profile problem found without preventing unrelated profile recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileHealthIssue {
    /// Root-relative path associated with the problem.
    pub path: PathBuf,
    /// Classified profile problem.
    pub kind: ProfileHealthKind,
}

/// Health problems reported while scanning active profile directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileHealthKind {
    /// A child of `accounts` is unsafe or unreadable.
    UnsafePath(String),
    /// A profile directory name is not a canonical account key.
    InvalidDirectoryKey,
    /// An account-key path exists but is not a directory.
    NotDirectory,
    /// The profile manifest is absent.
    MissingManifest,
    /// The profile manifest cannot be loaded.
    InvalidManifest(String),
    /// The profile manifest version is unsupported.
    UnsupportedManifestVersion(u32),
    /// The manifest key differs from its directory key.
    ManifestKeyMismatch {
        /// Key encoded by the directory.
        directory_key: AccountKey,
        /// Key stored in the manifest.
        manifest_key: AccountKey,
    },
    /// More than one active profile claims the same server account ID.
    DuplicateProfileAccountId(AccountId),
    /// Registry and manifest identity bindings disagree.
    RegistryManifestMismatch {
        /// Identity stored in the registry.
        registry_account_id: AccountId,
        /// Identity stored in the manifest.
        manifest_account_id: AccountId,
    },
    /// A registry entry has no valid active profile.
    MissingRegisteredProfile(AccountKey),
    /// A selected or previous key is absent after orphan adoption.
    MissingRegistryReference(AccountKey),
    /// A valid orphan conflicts with another registry identity binding.
    RegistryIdentityConflict(AccountId),
}

/// Details of startup reconciliation and recovery.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconciliationReport {
    /// Whether `accounts.json.bak` supplied the registry.
    pub recovered_registry_backup: bool,
    /// Manifests supplied by their rolling backups.
    pub recovered_manifest_backups: Vec<AccountKey>,
    /// Valid orphan profiles adopted into the registry.
    pub adopted_account_keys: Vec<AccountKey>,
    /// Pending intent cleared while retaining committed state.
    pub cleared_pending_transition: Option<PendingTransition>,
    /// Profile problems that did not block unrelated reconciliation.
    pub health_issues: Vec<ProfileHealthIssue>,
}

/// Reconciled registry and its recovery report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciledRegistry {
    /// Current registry contents.
    pub registry: AccountRegistry,
    /// Recovery and health details.
    pub report: ReconciliationReport,
}

/// Errors returned by account registry and profile operations.
#[derive(Debug, Error)]
pub enum AccountError {
    /// A validated storage operation failed.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// The registry document version is not supported.
    #[error("unsupported account registry version {0}")]
    UnsupportedRegistryVersion(u32),
    /// A profile manifest version is not supported.
    #[error("unsupported profile manifest version {version} for account {account_key}")]
    UnsupportedManifestVersion {
        /// Profile key.
        account_key: AccountKey,
        /// Unsupported version.
        version: u32,
    },
    /// Registry fields form an invalid state.
    #[error("invalid account registry state: {0}")]
    InvalidRegistryState(String),
    /// A registry contains a duplicate local key.
    #[error("duplicate registry account key {0}")]
    DuplicateRegistryKey(AccountKey),
    /// A registry or active profile set contains a duplicate identity.
    #[error("duplicate server account ID {0}")]
    DuplicateAccountId(AccountId),
    /// An account key is not registered.
    #[error("account key {0} is not registered")]
    AccountNotFound(AccountKey),
    /// A profile directory already exists unexpectedly.
    #[error("account profile already exists at `{0}`")]
    ProfileAlreadyExists(PathBuf),
    /// A manifest does not match its registry entry or directory.
    #[error("profile manifest does not match account {0}")]
    ManifestMismatch(AccountKey),
    /// More than one profile may own the requested identity.
    #[error("account ID {0} has an ambiguous active profile")]
    AmbiguousAccountId(AccountId),
    /// A transition does not match the pending intent.
    #[error("account transition does not match the pending intent")]
    InvalidTransition,
    /// Registry metadata cannot be removed while its profile remains active.
    #[error("account profile {0} is still active")]
    ActiveProfileExists(AccountKey),
    /// A unique account key could not be allocated.
    #[error("could not allocate a unique account key")]
    AccountKeyCollision,
}

/// Identity and metadata bound when committing a known-account migration.
///
/// Never carries the plaintext token; `credential_target` is only the reference
/// to the secure store, recorded when a token import succeeded.
#[derive(Debug, Clone)]
pub struct MigrationKnownBinding {
    /// Exact account key persisted in the migration journal before profile
    /// creation.
    pub account_key: AccountKey,
    /// Authoritative server account identity read from legacy settings.
    pub account_id: AccountId,
    /// Optional display name read from legacy settings.
    pub display_name: Option<String>,
    /// Time the migration committed the profile selection.
    pub verified_at: DateTime<Utc>,
    /// Migrated last-client-seen Unix seconds (0 when unknown).
    pub last_client_seen_unix: i64,
    /// Migrated last-client-launch Unix seconds (0 when unknown).
    pub last_client_launch_unix: i64,
    /// Credential-store target reference, present only when a token import
    /// succeeded. Never the token itself.
    pub credential_target: Option<String>,
}

/// Root-bound persistence service for account registries and profile manifests.
#[derive(Debug, Clone)]
pub struct AccountRegistryStore {
    storage_root: StorageRoot,
}

impl AccountRegistryStore {
    /// Create a registry service for a validated storage root.
    pub fn new(storage_root: StorageRoot) -> Self {
        Self { storage_root }
    }

    /// Load, reconcile, and recover the registry without opening profile data.
    pub fn reconcile(&self) -> Result<ReconciledRegistry, AccountError> {
        self.reconcile_internal(true).map(|(result, _)| result)
    }

    /// Register or refresh one verified account without selecting it.
    pub fn register_verified(
        &self,
        account_id: AccountId,
        display_name: Option<&str>,
        verified_at: DateTime<Utc>,
    ) -> Result<AccountRegistryEntry, AccountError> {
        let (mut reconciled, scan) = self.reconcile_internal(true)?;
        let display_name = normalize_display_name(display_name);

        if scan.blocked_account_ids.contains(&account_id) {
            return Err(AccountError::AmbiguousAccountId(account_id));
        }
        if let Some(entry) = reconciled
            .registry
            .accounts
            .iter_mut()
            .find(|entry| entry.account_id == account_id)
        {
            entry.display_name = display_name;
            entry.last_verified_at = Some(verified_at);
            let result = entry.clone();
            self.write_registry(&reconciled.registry)?;
            return Ok(result);
        }
        let key = self.create_profile(account_id.clone(), verified_at)?;
        let entry = AccountRegistryEntry::verified(key, account_id, display_name, verified_at);
        reconciled.registry.accounts.push(entry.clone());
        self.write_registry(&reconciled.registry)?;
        Ok(entry)
    }

    /// Find a registered profile by normalized server account ID.
    pub fn find_by_account_id(
        &self,
        account_id: &AccountId,
    ) -> Result<Option<AccountRegistryEntry>, AccountError> {
        let (reconciled, scan) = self.reconcile_internal(true)?;
        if scan.blocked_account_ids.contains(account_id) {
            return Err(AccountError::AmbiguousAccountId(account_id.clone()));
        }
        Ok(reconciled.registry.find_by_account_id(account_id).cloned())
    }

    /// Persist pending intent to switch to a known profile.
    pub fn begin_select(
        &self,
        account_key: AccountKey,
        started_at: DateTime<Utc>,
    ) -> Result<AccountRegistry, AccountError> {
        let mut registry = self.reconcile_internal(true)?.0.registry;
        self.validate_registered_profile(&registry, account_key)?;
        registry.pending_transition = Some(PendingTransition::selected(account_key, started_at));
        self.write_registry(&registry)?;
        Ok(registry)
    }

    /// Commit a previously prepared selected-profile transition.
    pub fn commit_select(
        &self,
        account_key: AccountKey,
        opened_at: DateTime<Utc>,
    ) -> Result<AccountRegistry, AccountError> {
        let mut registry = self.reconcile_internal(false)?.0.registry;
        let expected = registry.pending_transition.as_ref().is_some_and(|pending| {
            pending.target_mode == RegistryMode::Selected
                && pending.target_account_key == Some(account_key)
        });
        if !expected {
            return Err(AccountError::InvalidTransition);
        }
        self.validate_registered_profile(&registry, account_key)?;
        commit_selected(&mut registry, account_key, opened_at)?;
        self.write_registry(&registry)?;
        Ok(registry)
    }

    /// Select a validated profile in one atomic registry replacement.
    pub fn select(
        &self,
        account_key: AccountKey,
        opened_at: DateTime<Utc>,
    ) -> Result<AccountRegistry, AccountError> {
        let mut registry = self.reconcile()?.registry;
        self.validate_registered_profile(&registry, account_key)?;
        commit_selected(&mut registry, account_key, opened_at)?;
        self.write_registry(&registry)?;
        Ok(registry)
    }

    /// Persist pending intent to enter discovery mode.
    pub fn begin_discovery(
        &self,
        started_at: DateTime<Utc>,
    ) -> Result<AccountRegistry, AccountError> {
        let mut registry = self.reconcile()?.registry;
        registry.pending_transition = Some(PendingTransition::discovering(started_at));
        self.write_registry(&registry)?;
        Ok(registry)
    }

    /// Commit a previously prepared discovery transition.
    pub fn commit_discovery(&self) -> Result<AccountRegistry, AccountError> {
        let mut registry = self.reconcile_internal(false)?.0.registry;
        let expected = registry
            .pending_transition
            .as_ref()
            .is_some_and(|pending| pending.target_mode == RegistryMode::Discovering);
        if !expected {
            return Err(AccountError::InvalidTransition);
        }
        commit_discovering(&mut registry);
        self.write_registry(&registry)?;
        Ok(registry)
    }

    /// Deselect the current profile in one atomic registry replacement.
    pub fn deselect(&self) -> Result<AccountRegistry, AccountError> {
        let mut registry = self.reconcile()?.registry;
        commit_discovering(&mut registry);
        self.write_registry(&registry)?;
        Ok(registry)
    }

    /// Restore the profile that was selected before discovery began.
    pub fn cancel_discovery(
        &self,
        opened_at: DateTime<Utc>,
    ) -> Result<AccountRegistry, AccountError> {
        let mut registry = self.reconcile()?.registry;
        if registry.mode != RegistryMode::Discovering {
            return Err(AccountError::InvalidTransition);
        }
        registry.pending_transition = None;
        if let Some(previous) = registry.previous_account_key {
            self.validate_registered_profile(&registry, previous)?;
            commit_selected(&mut registry, previous, opened_at)?;
        }
        self.write_registry(&registry)?;
        Ok(registry)
    }

    /// Update mutable display and verification metadata.
    pub fn update_verified(
        &self,
        account_key: AccountKey,
        display_name: Option<&str>,
        verified_at: DateTime<Utc>,
    ) -> Result<AccountRegistryEntry, AccountError> {
        let mut registry = self.reconcile()?.registry;
        self.validate_registered_profile(&registry, account_key)?;
        let entry = registry
            .accounts
            .iter_mut()
            .find(|entry| entry.key == account_key)
            .ok_or(AccountError::AccountNotFound(account_key))?;
        entry.display_name = normalize_display_name(display_name);
        entry.last_verified_at = Some(verified_at);
        let result = entry.clone();
        self.write_registry(&registry)?;
        Ok(result)
    }

    /// Bind or clear the credential-store target reference for an account.
    ///
    /// Stores only the target reference, never the token itself. Passing `None`
    /// clears the reference (e.g. after the credential is deleted).
    pub fn set_credential_target(
        &self,
        account_key: AccountKey,
        credential_target: Option<&str>,
    ) -> Result<AccountRegistryEntry, AccountError> {
        let mut registry = self.reconcile()?.registry;
        let entry = registry
            .accounts
            .iter_mut()
            .find(|entry| entry.key == account_key)
            .ok_or(AccountError::AccountNotFound(account_key))?;
        entry.credential_target = credential_target.map(str::to_string);
        let result = entry.clone();
        self.write_registry(&registry)?;
        Ok(result)
    }

    /// Update the last-opened metadata for a validated profile.
    pub fn update_last_opened(
        &self,
        account_key: AccountKey,
        opened_at: DateTime<Utc>,
    ) -> Result<AccountRegistryEntry, AccountError> {
        let mut registry = self.reconcile()?.registry;
        self.validate_registered_profile(&registry, account_key)?;
        let entry = registry
            .accounts
            .iter_mut()
            .find(|entry| entry.key == account_key)
            .ok_or(AccountError::AccountNotFound(account_key))?;
        entry.last_opened_at = Some(opened_at);
        let result = entry.clone();
        self.write_registry(&registry)?;
        Ok(result)
    }

    /// Record the last confirmed game-client launch Unix seconds for a validated
    /// profile, atomically. This is the runtime home for the value that used to
    /// live in `settings.account`; a selected profile is the authoritative owner.
    pub fn update_last_client_launch(
        &self,
        account_key: AccountKey,
        last_client_launch_unix: i64,
    ) -> Result<AccountRegistryEntry, AccountError> {
        let mut registry = self.reconcile()?.registry;
        self.validate_registered_profile(&registry, account_key)?;
        let entry = registry
            .accounts
            .iter_mut()
            .find(|entry| entry.key == account_key)
            .ok_or(AccountError::AccountNotFound(account_key))?;
        entry.last_client_launch_unix = last_client_launch_unix;
        let result = entry.clone();
        self.write_registry(&registry)?;
        Ok(result)
    }

    /// Delete an account's profile directory, registry entry, and credential.
    ///
    /// The profile directory is removed first, then the registry entry, and
    /// finally the credential (best-effort: a missing or unavailable store is
    /// not fatal). This ordering ensures a filesystem failure never leaves a
    /// registered profile without its credential.
    pub fn delete_account(
        &self,
        account_key: AccountKey,
        credentials: &dyn super::CredentialStore,
    ) -> Result<AccountRegistry, AccountError> {
        let registry = self.reconcile()?.registry;
        let entry = registry
            .entry(account_key)
            .ok_or(AccountError::AccountNotFound(account_key))?;
        let target = entry
            .credential_target()
            .map(str::to_string)
            .unwrap_or_else(|| super::credential_target(account_key));

        let paths = AccountPaths::new(self.storage_root.clone(), account_key);
        self.storage_root
            .remove_directory_all(paths.relative_root())?;

        // Remove the sibling lock file (`accounts/<key>.lock`).
        let lock_relative = super::migration::lock::target_lock_relative(&paths.relative_root());
        let lock_path = self.storage_root.ensure_no_links(&lock_relative);
        if let Ok(lock_path) = lock_path {
            let _ = std::fs::remove_file(lock_path);
        }

        let updated = self.remove_entry(account_key)?;

        if let Err(e) = credentials.delete(&target) {
            tracing::warn!("[ACCOUNT] credential cleanup for {account_key}: {e}");
        }

        Ok(updated)
    }

    /// Remove registry metadata after the active profile has already left `accounts`.
    pub fn remove_entry(&self, account_key: AccountKey) -> Result<AccountRegistry, AccountError> {
        let mut registry = self.reconcile()?.registry;
        let paths = AccountPaths::new(self.storage_root.clone(), account_key);
        if self.storage_root.directory_exists(paths.relative_root())? {
            return Err(AccountError::ActiveProfileExists(account_key));
        }
        let original_len = registry.accounts.len();
        registry.accounts.retain(|entry| entry.key != account_key);
        if registry.accounts.len() == original_len {
            return Err(AccountError::AccountNotFound(account_key));
        }
        if registry.selected_account_key == Some(account_key) {
            registry.mode = RegistryMode::Discovering;
            registry.selected_account_key = None;
            registry.previous_account_key = None;
        } else if registry.previous_account_key == Some(account_key) {
            registry.previous_account_key = None;
        }
        if registry
            .pending_transition
            .as_ref()
            .is_some_and(|pending| pending.target_account_key == Some(account_key))
        {
            registry.pending_transition = None;
        }
        self.write_registry(&registry)?;
        Ok(registry)
    }

    /// Create one account profile under an exact, caller-supplied key without
    /// selecting it. Used by flat-layout migration, which persists the generated
    /// key in its journal before creating the profile so the operation is safe
    /// to resume. Idempotent: if the profile already exists with a matching
    /// manifest identity, the existing manifest is returned; a directory that
    /// exists with a contradictory identity fails closed.
    pub fn create_migration_profile(
        &self,
        account_key: AccountKey,
        account_id: AccountId,
        created_at: DateTime<Utc>,
    ) -> Result<ProfileManifest, AccountError> {
        let paths = AccountPaths::new(self.storage_root.clone(), account_key);
        if self.storage_root.directory_exists(paths.relative_root())? {
            match load_json::<ProfileManifest>(&self.storage_root, paths.relative_manifest())? {
                LoadOutcome::Primary(manifest) | LoadOutcome::RecoveredBackup(manifest) => {
                    if manifest.version() != PROFILE_VERSION {
                        return Err(AccountError::UnsupportedManifestVersion {
                            account_key,
                            version: manifest.version(),
                        });
                    }
                    if manifest.account_key() != account_key || manifest.account_id() != &account_id
                    {
                        return Err(AccountError::ManifestMismatch(account_key));
                    }
                    return Ok(manifest);
                }
                // A directory exists but carries no manifest. Only an *empty*
                // directory is an acceptable exact-journal resume target; a
                // non-empty directory is foreign content and must never be
                // adopted by writing a manifest over it.
                LoadOutcome::Missing => {
                    if !self
                        .storage_root
                        .read_directory(paths.relative_root())?
                        .is_empty()
                    {
                        return Err(AccountError::ProfileAlreadyExists(paths.relative_root()));
                    }
                }
            }
        } else {
            self.storage_root.create_directory(paths.relative_root())?;
        }
        let manifest = ProfileManifest::new(account_key, account_id, created_at);
        if let Err(error) = write_json_atomic(
            &self.storage_root,
            paths.relative_manifest(),
            &manifest,
            BackupPolicy::Single,
        ) {
            return Err(error.into());
        }
        Ok(manifest)
    }

    /// Atomically insert (or refresh) and select the exact migrated account.
    ///
    /// The prepared profile manifest must already exist and match the binding
    /// identity. Idempotent on resume: re-committing the same key and identity
    /// succeeds, while a real conflict (the key bound to a different identity, or
    /// the identity bound to a different key) fails closed.
    pub fn commit_migration_known(
        &self,
        binding: &MigrationKnownBinding,
    ) -> Result<AccountRegistry, AccountError> {
        self.validate_prepared_profile(binding.account_key, &binding.account_id)?;
        let mut registry = self.reconcile_internal(true)?.0.registry;

        if let Some(existing) = registry
            .accounts
            .iter()
            .find(|entry| entry.account_id == binding.account_id)
        {
            if existing.key != binding.account_key {
                return Err(AccountError::DuplicateAccountId(binding.account_id.clone()));
            }
        }
        if let Some(existing) = registry
            .accounts
            .iter()
            .find(|entry| entry.key == binding.account_key)
        {
            if existing.account_id != binding.account_id {
                return Err(AccountError::DuplicateRegistryKey(binding.account_key));
            }
        }

        let display_name = normalize_display_name(binding.display_name.as_deref());
        match registry
            .accounts
            .iter_mut()
            .find(|entry| entry.key == binding.account_key)
        {
            Some(entry) => {
                entry.account_id = binding.account_id.clone();
                entry.display_name = display_name;
                entry.last_verified_at = Some(binding.verified_at);
                entry.credential_target = binding.credential_target.clone();
                entry.last_client_seen_unix = binding.last_client_seen_unix;
                entry.last_client_launch_unix = binding.last_client_launch_unix;
            }
            None => {
                let mut entry = AccountRegistryEntry::verified(
                    binding.account_key,
                    binding.account_id.clone(),
                    display_name,
                    binding.verified_at,
                );
                entry.credential_target = binding.credential_target.clone();
                entry.last_client_seen_unix = binding.last_client_seen_unix;
                entry.last_client_launch_unix = binding.last_client_launch_unix;
                registry.accounts.push(entry);
            }
        }

        commit_selected(&mut registry, binding.account_key, binding.verified_at)?;
        self.write_registry(&registry)?;
        Ok(registry)
    }

    /// Atomically commit discovery mode for an unknown-account migration.
    ///
    /// Idempotent: committing discovery when no account is selected simply
    /// clears any pending intent and keeps the registry in discovery mode.
    pub fn commit_migration_discovery(&self) -> Result<AccountRegistry, AccountError> {
        let mut registry = self.reconcile_internal(true)?.0.registry;
        commit_discovering(&mut registry);
        self.write_registry(&registry)?;
        Ok(registry)
    }

    fn validate_prepared_profile(
        &self,
        account_key: AccountKey,
        account_id: &AccountId,
    ) -> Result<(), AccountError> {
        let paths = AccountPaths::new(self.storage_root.clone(), account_key);
        let manifest =
            match load_json::<ProfileManifest>(&self.storage_root, paths.relative_manifest())? {
                LoadOutcome::Primary(manifest) | LoadOutcome::RecoveredBackup(manifest) => manifest,
                LoadOutcome::Missing => return Err(AccountError::ManifestMismatch(account_key)),
            };
        if manifest.version() != PROFILE_VERSION {
            return Err(AccountError::UnsupportedManifestVersion {
                account_key,
                version: manifest.version(),
            });
        }
        if manifest.account_key() != account_key || manifest.account_id() != account_id {
            return Err(AccountError::ManifestMismatch(account_key));
        }
        Ok(())
    }

    fn reconcile_internal(
        &self,
        recover_pending: bool,
    ) -> Result<(ReconciledRegistry, ProfileScan), AccountError> {
        let (mut registry, recovered_registry_backup) = self.load_registry()?;
        let mut scan = self.scan_profiles()?;
        let mut report = ReconciliationReport {
            recovered_registry_backup,
            recovered_manifest_backups: scan.recovered_backups.clone(),
            health_issues: scan.health_issues.clone(),
            ..ReconciliationReport::default()
        };
        let mut changed = false;

        for entry in &registry.accounts {
            match scan.profiles.get(&entry.key) {
                Some(manifest) if manifest.account_id() != &entry.account_id => {
                    scan.blocked_account_ids.insert(entry.account_id.clone());
                    scan.blocked_account_ids
                        .insert(manifest.account_id().clone());
                }
                None => {
                    scan.blocked_account_ids.insert(entry.account_id.clone());
                }
                Some(_) => {}
            }
        }

        for manifest in scan.profiles.values() {
            if registry.entry(manifest.account_key()).is_some() {
                continue;
            }
            if registry.find_by_account_id(manifest.account_id()).is_some() {
                scan.blocked_account_ids
                    .insert(manifest.account_id().clone());
                report.health_issues.push(ProfileHealthIssue {
                    path: AccountPaths::new(self.storage_root.clone(), manifest.account_key())
                        .relative_root(),
                    kind: ProfileHealthKind::RegistryIdentityConflict(
                        manifest.account_id().clone(),
                    ),
                });
                continue;
            }
            registry
                .accounts
                .push(AccountRegistryEntry::orphan(manifest));
            report.adopted_account_keys.push(manifest.account_key());
            changed = true;
        }

        self.validate_registry_profiles(&registry, &scan.profiles, &mut report);
        if recover_pending {
            if let Some(pending) = registry.pending_transition.take() {
                report.cleared_pending_transition = Some(pending);
                changed = true;
            }
        }
        self.validate_registry_references(&registry, &mut report);
        registry.validate_intrinsic()?;

        if changed {
            self.write_registry(&registry)?;
        }
        Ok((ReconciledRegistry { registry, report }, scan))
    }

    fn load_registry(&self) -> Result<(AccountRegistry, bool), AccountError> {
        let (registry, recovered_backup) = match load_json(&self.storage_root, REGISTRY_PATH)? {
            LoadOutcome::Primary(registry) => (registry, false),
            LoadOutcome::RecoveredBackup(registry) => (registry, true),
            LoadOutcome::Missing => (AccountRegistry::new(), false),
        };
        registry.validate_intrinsic()?;
        Ok((registry, recovered_backup))
    }

    fn scan_profiles(&self) -> Result<ProfileScan, AccountError> {
        let mut scan = ProfileScan::default();
        let mut candidates = Vec::new();
        for child in self.storage_root.read_directory("accounts")? {
            let child_name = child.file_name().and_then(|name| name.to_str());
            let Some(child_name) = child_name else {
                scan.issue(child, ProfileHealthKind::InvalidDirectoryKey);
                continue;
            };
            let Ok(directory_key) = child_name.parse::<AccountKey>() else {
                scan.issue(child, ProfileHealthKind::InvalidDirectoryKey);
                continue;
            };
            match self.storage_root.directory_exists(&child) {
                Ok(true) => {}
                Ok(false) => {
                    scan.issue(child, ProfileHealthKind::NotDirectory);
                    continue;
                }
                Err(error) => {
                    scan.issue(child, ProfileHealthKind::UnsafePath(error.to_string()));
                    continue;
                }
            }

            let manifest_path = child.join("profile.json");
            let (manifest, recovered_backup) =
                match load_json::<ProfileManifest>(&self.storage_root, &manifest_path) {
                    Ok(LoadOutcome::Primary(manifest)) => (manifest, false),
                    Ok(LoadOutcome::RecoveredBackup(manifest)) => (manifest, true),
                    Ok(LoadOutcome::Missing) => {
                        scan.issue(manifest_path, ProfileHealthKind::MissingManifest);
                        continue;
                    }
                    Err(error) => {
                        scan.issue(
                            manifest_path,
                            ProfileHealthKind::InvalidManifest(error.to_string()),
                        );
                        continue;
                    }
                };

            if manifest.version() != PROFILE_VERSION {
                scan.blocked_account_ids
                    .insert(manifest.account_id().clone());
                scan.issue(
                    manifest_path,
                    ProfileHealthKind::UnsupportedManifestVersion(manifest.version()),
                );
                continue;
            }
            if manifest.account_key() != directory_key {
                scan.blocked_account_ids
                    .insert(manifest.account_id().clone());
                scan.issue(
                    manifest_path,
                    ProfileHealthKind::ManifestKeyMismatch {
                        directory_key,
                        manifest_key: manifest.account_key(),
                    },
                );
                continue;
            }
            if recovered_backup {
                scan.recovered_backups.push(directory_key);
            }
            candidates.push((manifest_path, manifest));
        }

        let mut account_counts: HashMap<AccountId, usize> = HashMap::new();
        for (_, manifest) in &candidates {
            *account_counts
                .entry(manifest.account_id().clone())
                .or_default() += 1;
        }
        for (path, manifest) in candidates {
            if account_counts
                .get(manifest.account_id())
                .copied()
                .unwrap_or(0)
                > 1
            {
                scan.blocked_account_ids
                    .insert(manifest.account_id().clone());
                scan.issue(
                    path,
                    ProfileHealthKind::DuplicateProfileAccountId(manifest.account_id().clone()),
                );
                continue;
            }
            scan.profiles.insert(manifest.account_key(), manifest);
        }
        Ok(scan)
    }

    fn validate_registry_profiles(
        &self,
        registry: &AccountRegistry,
        profiles: &BTreeMap<AccountKey, ProfileManifest>,
        report: &mut ReconciliationReport,
    ) {
        for entry in &registry.accounts {
            let paths = AccountPaths::new(self.storage_root.clone(), entry.key);
            match profiles.get(&entry.key) {
                Some(manifest) if manifest.account_id() != &entry.account_id => {
                    report.health_issues.push(ProfileHealthIssue {
                        path: paths.relative_manifest(),
                        kind: ProfileHealthKind::RegistryManifestMismatch {
                            registry_account_id: entry.account_id.clone(),
                            manifest_account_id: manifest.account_id().clone(),
                        },
                    });
                }
                Some(_) => {}
                None => report.health_issues.push(ProfileHealthIssue {
                    path: paths.relative_root(),
                    kind: ProfileHealthKind::MissingRegisteredProfile(entry.key),
                }),
            }
        }
    }

    fn validate_registry_references(
        &self,
        registry: &AccountRegistry,
        report: &mut ReconciliationReport,
    ) {
        for key in [registry.selected_account_key, registry.previous_account_key]
            .into_iter()
            .flatten()
        {
            if registry.entry(key).is_none() {
                report.health_issues.push(ProfileHealthIssue {
                    path: PathBuf::from(REGISTRY_PATH),
                    kind: ProfileHealthKind::MissingRegistryReference(key),
                });
            }
        }
    }

    fn create_profile(
        &self,
        account_id: AccountId,
        created_at: DateTime<Utc>,
    ) -> Result<AccountKey, AccountError> {
        for _ in 0..32 {
            let key = AccountKey::generate();
            let paths = AccountPaths::new(self.storage_root.clone(), key);
            if self.storage_root.directory_exists(paths.relative_root())? {
                continue;
            }
            self.storage_root.create_directory(paths.relative_root())?;
            let manifest = ProfileManifest::new(key, account_id.clone(), created_at);
            if let Err(error) = write_json_atomic(
                &self.storage_root,
                paths.relative_manifest(),
                &manifest,
                BackupPolicy::Single,
            ) {
                let _ = self
                    .storage_root
                    .remove_empty_directory(paths.relative_root());
                return Err(error.into());
            }
            return Ok(key);
        }
        Err(AccountError::AccountKeyCollision)
    }

    fn validate_registered_profile(
        &self,
        registry: &AccountRegistry,
        account_key: AccountKey,
    ) -> Result<(), AccountError> {
        let entry = registry
            .entry(account_key)
            .ok_or(AccountError::AccountNotFound(account_key))?;
        let paths = AccountPaths::new(self.storage_root.clone(), account_key);
        let manifest =
            match load_json::<ProfileManifest>(&self.storage_root, paths.relative_manifest())? {
                LoadOutcome::Primary(manifest) | LoadOutcome::RecoveredBackup(manifest) => manifest,
                LoadOutcome::Missing => return Err(AccountError::ManifestMismatch(account_key)),
            };
        if manifest.version() != PROFILE_VERSION {
            return Err(AccountError::UnsupportedManifestVersion {
                account_key,
                version: manifest.version(),
            });
        }
        if manifest.account_key() != account_key || manifest.account_id() != entry.account_id() {
            return Err(AccountError::ManifestMismatch(account_key));
        }
        Ok(())
    }

    fn write_registry(&self, registry: &AccountRegistry) -> Result<(), AccountError> {
        registry.validate_intrinsic()?;
        write_json_atomic(
            &self.storage_root,
            REGISTRY_PATH,
            registry,
            BackupPolicy::Single,
        )?;
        Ok(())
    }
}

#[derive(Debug, Default)]
struct ProfileScan {
    profiles: BTreeMap<AccountKey, ProfileManifest>,
    blocked_account_ids: HashSet<AccountId>,
    recovered_backups: Vec<AccountKey>,
    health_issues: Vec<ProfileHealthIssue>,
}

impl ProfileScan {
    fn issue(&mut self, path: PathBuf, kind: ProfileHealthKind) {
        self.health_issues.push(ProfileHealthIssue { path, kind });
    }
}

fn normalize_display_name(display_name: Option<&str>) -> Option<String> {
    display_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

fn commit_selected(
    registry: &mut AccountRegistry,
    account_key: AccountKey,
    opened_at: DateTime<Utc>,
) -> Result<(), AccountError> {
    let entry = registry
        .accounts
        .iter_mut()
        .find(|entry| entry.key == account_key)
        .ok_or(AccountError::AccountNotFound(account_key))?;
    entry.last_opened_at = Some(opened_at);
    registry.mode = RegistryMode::Selected;
    registry.selected_account_key = Some(account_key);
    registry.previous_account_key = None;
    registry.pending_transition = None;
    Ok(())
}

/// Build shortened key suffixes for entries that share a display name.
///
/// Returns a map from account key to a short suffix string (e.g. "a4db") for
/// entries whose display name collides with another entry. Entries with unique
/// or absent names are omitted.
pub fn shorten_duplicate_keys(entries: &[AccountRegistryEntry]) -> HashMap<AccountKey, String> {
    let mut by_name: HashMap<&str, Vec<AccountKey>> = HashMap::new();
    for entry in entries {
        if let Some(name) = entry.display_name() {
            by_name.entry(name).or_default().push(entry.key());
        }
    }

    let mut result = HashMap::new();
    for keys in by_name.values() {
        if keys.len() < 2 {
            continue;
        }
        // Widen the prefix until every key in the group is unique.
        let hex_strings: Vec<String> = keys
            .iter()
            .map(|k| k.as_uuid().as_simple().to_string())
            .collect();
        let mut width = 4;
        while width < 32 {
            let mut prefixes = HashSet::new();
            if hex_strings.iter().all(|h| prefixes.insert(&h[..width])) {
                break;
            }
            width += 4;
        }
        for (key, hex) in keys.iter().zip(hex_strings.iter()) {
            result.insert(*key, hex[..width].to_string());
        }
    }
    result
}

fn commit_discovering(registry: &mut AccountRegistry) {
    if registry.mode == RegistryMode::Selected {
        registry.previous_account_key = registry.selected_account_key;
    }
    registry.mode = RegistryMode::Discovering;
    registry.selected_account_key = None;
    registry.pending_transition = None;
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::account::credentials::CredentialStore;

    fn store() -> (TempDir, AccountRegistryStore) {
        let temp = tempfile::tempdir().unwrap();
        let root = StorageRoot::from_path(temp.path()).unwrap();
        (temp, AccountRegistryStore::new(root))
    }

    fn register(
        store: &AccountRegistryStore,
        account_id: &str,
        display_name: &str,
    ) -> AccountRegistryEntry {
        store
            .register_verified(
                AccountId::new(account_id).unwrap(),
                Some(display_name),
                Utc::now(),
            )
            .unwrap()
    }

    fn register_no_name(store: &AccountRegistryStore, account_id: &str) -> AccountRegistryEntry {
        store
            .register_verified(AccountId::new(account_id).unwrap(), None, Utc::now())
            .unwrap()
    }

    fn write_manifest(root: &StorageRoot, key: AccountKey, account_id: &str) -> ProfileManifest {
        let paths = AccountPaths::new(root.clone(), key);
        root.create_directory(paths.relative_root()).unwrap();
        let manifest = ProfileManifest::new(key, AccountId::new(account_id).unwrap(), Utc::now());
        write_json_atomic(
            root,
            paths.relative_manifest(),
            &manifest,
            BackupPolicy::Single,
        )
        .unwrap();
        manifest
    }

    #[test]
    fn fresh_install_reconciles_to_empty_discovery_registry() {
        let (_temp, store) = store();

        let reconciled = store.reconcile().unwrap();

        assert_eq!(reconciled.registry, AccountRegistry::new());
    }

    #[test]
    fn create_migration_profile_refuses_nonempty_dir_without_manifest() {
        let (_temp, store) = store();
        let key = AccountKey::generate();
        let paths = AccountPaths::new(store.storage_root.clone(), key);
        // A foreign, non-empty profile directory with no manifest must never be
        // adopted by writing a manifest over it.
        store
            .storage_root
            .create_directory(paths.relative_root())
            .unwrap();
        fs::write(
            store
                .storage_root
                .path()
                .join(paths.relative_root())
                .join("foreign.txt"),
            b"not ours",
        )
        .unwrap();

        let error = store
            .create_migration_profile(key, AccountId::new("ACCT-1").unwrap(), Utc::now())
            .unwrap_err();
        assert!(matches!(error, AccountError::ProfileAlreadyExists(_)));
        // No manifest was written.
        assert!(!store
            .storage_root
            .path()
            .join(paths.relative_manifest())
            .exists());
    }

    #[test]
    fn create_migration_profile_permits_empty_dir_resume() {
        let (_temp, store) = store();
        let key = AccountKey::generate();
        let paths = AccountPaths::new(store.storage_root.clone(), key);
        // An exact-journal resume may find an empty, migration-owned directory.
        store
            .storage_root
            .create_directory(paths.relative_root())
            .unwrap();

        let manifest = store
            .create_migration_profile(key, AccountId::new("ACCT-1").unwrap(), Utc::now())
            .unwrap();
        assert_eq!(manifest.account_key(), key);
        // Idempotent on a second call with the same identity.
        let again = store
            .create_migration_profile(key, AccountId::new("ACCT-1").unwrap(), Utc::now())
            .unwrap();
        assert_eq!(again.account_id().as_str(), "ACCT-1");
    }

    #[test]
    fn registry_round_trips_through_atomic_document() {
        let (_temp, store) = store();
        let entry = register(&store, "account-a", "Player");

        let reconciled = store.reconcile().unwrap();

        assert_eq!(reconciled.registry.entry(entry.key()), Some(&entry));
    }

    #[test]
    fn credential_target_binds_updates_and_clears() {
        let (_temp, store) = store();
        let entry = register(&store, "account-a", "Player");
        assert_eq!(entry.credential_target(), None);

        let updated = store
            .set_credential_target(entry.key(), Some("RealmHound/account/opaque"))
            .unwrap();
        assert_eq!(
            updated.credential_target(),
            Some("RealmHound/account/opaque")
        );

        // The bound reference survives a reload and can be cleared again.
        let reloaded = store.reconcile().unwrap();
        assert_eq!(
            reloaded
                .registry
                .entry(entry.key())
                .unwrap()
                .credential_target(),
            Some("RealmHound/account/opaque")
        );
        let cleared = store.set_credential_target(entry.key(), None).unwrap();
        assert_eq!(cleared.credential_target(), None);
    }

    #[test]
    fn registry_json_without_credential_field_deserializes() {
        let (_temp, store) = store();
        register(&store, "account-a", "Player");
        let registry = store.reconcile().unwrap().registry;

        // Simulate a pre-credential registry document: strip the new field.
        let mut value = serde_json::to_value(&registry).unwrap();
        for account in value["accounts"].as_array_mut().unwrap() {
            account.as_object_mut().unwrap().remove("credential_target");
        }
        assert!(value["accounts"][0].get("credential_target").is_none());

        let restored: AccountRegistry = serde_json::from_value(value).unwrap();
        assert_eq!(restored.accounts()[0].credential_target(), None);
    }

    #[test]
    fn duplicate_display_names_are_supported() {
        let (_temp, store) = store();
        register(&store, "account-a", "Player");
        register(&store, "account-b", "Player");

        let reconciled = store.reconcile().unwrap();

        assert_eq!(
            reconciled
                .registry
                .accounts()
                .iter()
                .filter(|entry| entry.display_name() == Some("Player"))
                .count(),
            2
        );
    }

    #[test]
    fn display_name_update_preserves_account_key() {
        let (_temp, store) = store();
        let original = register(&store, "account-a", "Before");

        let updated = store
            .update_verified(original.key(), Some("After"), Utc::now())
            .unwrap();

        assert_eq!(updated.key(), original.key());
        assert_eq!(updated.display_name(), Some("After"));
    }

    #[test]
    fn registration_reuses_normalized_account_id() {
        let (_temp, store) = store();
        let original = register(&store, "account-a", "Before");

        let updated = store
            .register_verified(
                AccountId::new(" account-a ").unwrap(),
                Some("After"),
                Utc::now(),
            )
            .unwrap();

        assert_eq!(updated.key(), original.key());
    }

    #[test]
    fn manifest_is_separate_from_registry() {
        let (temp, store) = store();
        let entry = register(&store, "account-a", "Player");

        assert!(temp.path().join("accounts.json").is_file());
        assert!(temp
            .path()
            .join("accounts")
            .join(entry.key().to_string())
            .join("profile.json")
            .is_file());
    }

    #[test]
    fn update_last_client_launch_persists_atomically() {
        let (_temp, store) = store();
        let entry = register(&store, "account-a", "Player");
        assert_eq!(entry.last_client_launch_unix(), 0);

        let updated = store
            .update_last_client_launch(entry.key(), 1_725_000_000)
            .unwrap();
        assert_eq!(updated.last_client_launch_unix(), 1_725_000_000);

        // The value survives a reload of the persisted registry.
        let reloaded = store
            .reconcile()
            .unwrap()
            .registry
            .entry(entry.key())
            .unwrap()
            .last_client_launch_unix();
        assert_eq!(reloaded, 1_725_000_000);
    }

    #[test]
    fn update_last_client_launch_rejects_unknown_account() {
        let (_temp, store) = store();
        register(&store, "account-a", "Player");
        let missing = AccountKey::generate();
        assert!(matches!(
            store.update_last_client_launch(missing, 1),
            Err(AccountError::AccountNotFound(_))
        ));
    }

    #[test]
    fn orphan_manifest_is_adopted_without_display_metadata() {
        let (temp, store) = store();
        let key = AccountKey::generate();
        let root = StorageRoot::from_path(temp.path()).unwrap();
        let paths = AccountPaths::new(root.clone(), key);
        root.create_directory(paths.relative_root()).unwrap();
        let manifest = ProfileManifest::new(key, AccountId::new("orphan").unwrap(), Utc::now());
        write_json_atomic(
            &root,
            paths.relative_manifest(),
            &manifest,
            BackupPolicy::Single,
        )
        .unwrap();

        let reconciled = store.reconcile().unwrap();

        assert_eq!(reconciled.registry.entry(key).unwrap().display_name(), None);
        assert_eq!(reconciled.report.adopted_account_keys, vec![key]);
    }

    #[test]
    fn unrelated_damaged_profile_does_not_block_orphan_adoption() {
        let (temp, store) = store();
        let key = AccountKey::generate();
        let root = StorageRoot::from_path(temp.path()).unwrap();
        let paths = AccountPaths::new(root.clone(), key);
        root.create_directory(paths.relative_root()).unwrap();
        write_json_atomic(
            &root,
            paths.relative_manifest(),
            &ProfileManifest::new(key, AccountId::new("valid").unwrap(), Utc::now()),
            BackupPolicy::Single,
        )
        .unwrap();
        let damaged_key = AccountKey::generate();
        root.create_directory(Path::new("accounts").join(damaged_key.to_string()))
            .unwrap();
        fs::write(
            temp.path()
                .join("accounts")
                .join(damaged_key.to_string())
                .join("profile.json"),
            b"{",
        )
        .unwrap();

        let reconciled = store.reconcile().unwrap();

        assert!(reconciled.registry.entry(key).is_some());
        assert_eq!(reconciled.report.health_issues.len(), 1);
    }

    #[test]
    fn selected_orphan_is_adopted_before_reference_validation() {
        let (temp, store) = store();
        let entry = register(&store, "account-a", "Player");
        store.select(entry.key(), Utc::now()).unwrap();
        let registry_path = temp.path().join("accounts.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&registry_path).unwrap()).unwrap();
        value["accounts"] = json!([]);
        fs::write(&registry_path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

        let reconciled = store.reconcile().unwrap();

        assert_eq!(
            reconciled.registry.selected_account_key(),
            Some(entry.key())
        );
        assert!(reconciled.registry.entry(entry.key()).is_some());
    }

    #[test]
    fn pending_select_recovers_last_committed_selection() {
        let (_temp, store) = store();
        let first = register(&store, "account-a", "A");
        let second = register(&store, "account-b", "B");
        store.select(first.key(), Utc::now()).unwrap();
        store.begin_select(second.key(), Utc::now()).unwrap();

        let reconciled = store.reconcile().unwrap();

        assert_eq!(
            reconciled.registry.selected_account_key(),
            Some(first.key())
        );
        assert!(reconciled.registry.pending_transition().is_none());
        assert_eq!(
            reconciled
                .report
                .cleared_pending_transition
                .unwrap()
                .target_account_key(),
            Some(second.key())
        );
    }

    #[test]
    fn committed_select_replaces_pending_state() {
        let (_temp, store) = store();
        let first = register(&store, "account-a", "A");
        let second = register(&store, "account-b", "B");
        store.select(first.key(), Utc::now()).unwrap();
        store.begin_select(second.key(), Utc::now()).unwrap();

        let registry = store.commit_select(second.key(), Utc::now()).unwrap();

        assert_eq!(registry.selected_account_key(), Some(second.key()));
        assert!(registry.pending_transition().is_none());
    }

    #[test]
    fn discovery_cancel_restores_previous_selection() {
        let (_temp, store) = store();
        let entry = register(&store, "account-a", "A");
        store.select(entry.key(), Utc::now()).unwrap();
        store.begin_discovery(Utc::now()).unwrap();
        store.commit_discovery().unwrap();

        let registry = store.cancel_discovery(Utc::now()).unwrap();

        assert_eq!(registry.selected_account_key(), Some(entry.key()));
        assert_eq!(registry.mode(), RegistryMode::Selected);
    }

    #[test]
    fn discovery_cancel_without_previous_stays_discovering() {
        let (_temp, store) = store();

        let registry = store.cancel_discovery(Utc::now()).unwrap();

        assert_eq!(registry.mode(), RegistryMode::Discovering);
        assert_eq!(registry.selected_account_key(), None);
    }

    #[test]
    fn invalid_pending_transition_shape_is_rejected() {
        let registry = AccountRegistry {
            pending_transition: Some(PendingTransition {
                target_mode: RegistryMode::Discovering,
                target_account_key: Some(AccountKey::generate()),
                reason: TransitionReason::AccountDiscovery,
                started_at: Utc::now(),
            }),
            ..AccountRegistry::new()
        };

        assert!(registry.validate_intrinsic().is_err());
    }

    #[test]
    fn selected_pending_transition_requires_target_key() {
        let registry = AccountRegistry {
            pending_transition: Some(PendingTransition {
                target_mode: RegistryMode::Selected,
                target_account_key: None,
                reason: TransitionReason::AccountSwitch,
                started_at: Utc::now(),
            }),
            ..AccountRegistry::new()
        };

        assert!(registry.validate_intrinsic().is_err());
    }

    #[test]
    fn future_registry_version_is_rejected() {
        let (temp, store) = store();
        fs::write(
            temp.path().join("accounts.json"),
            br#"{"version":2,"mode":"discovering","selected_account_key":null,"previous_account_key":null,"pending_transition":null,"accounts":[]}"#,
        )
        .unwrap();

        assert!(matches!(
            store.reconcile(),
            Err(AccountError::UnsupportedRegistryVersion(2))
        ));
    }

    #[test]
    fn active_profile_blocks_registry_removal() {
        let (_temp, store) = store();
        let entry = register(&store, "account-a", "A");

        assert!(matches!(
            store.remove_entry(entry.key()),
            Err(AccountError::ActiveProfileExists(key)) if key == entry.key()
        ));
    }

    #[test]
    fn selected_absent_profile_removal_enters_discovery() {
        let (temp, store) = store();
        let entry = register(&store, "account-a", "A");
        store.select(entry.key(), Utc::now()).unwrap();
        let active = temp.path().join("accounts").join(entry.key().to_string());
        let archived = temp.path().join("archives").join(entry.key().to_string());
        fs::create_dir_all(archived.parent().unwrap()).unwrap();
        fs::rename(&active, &archived).unwrap();

        let registry = store.remove_entry(entry.key()).unwrap();

        assert_eq!(registry.mode(), RegistryMode::Discovering);
        assert!(registry.accounts().is_empty());
        assert!(archived.is_dir());
    }

    #[test]
    fn archives_are_excluded_from_orphan_adoption() {
        let (temp, store) = store();
        let key = AccountKey::generate();
        let archive = temp.path().join("archives").join(key.to_string());
        fs::create_dir_all(&archive).unwrap();
        fs::write(
            archive.join("profile.json"),
            serde_json::to_vec_pretty(&ProfileManifest::new(
                key,
                AccountId::new("archived").unwrap(),
                Utc::now(),
            ))
            .unwrap(),
        )
        .unwrap();

        let reconciled = store.reconcile().unwrap();

        assert!(reconciled.registry.accounts().is_empty());
    }

    #[test]
    fn duplicate_profile_account_id_blocks_registration() {
        let (temp, store) = store();
        let root = StorageRoot::from_path(temp.path()).unwrap();
        write_manifest(&root, AccountKey::generate(), "duplicate");
        write_manifest(&root, AccountKey::generate(), "duplicate");

        let error = store
            .register_verified(
                AccountId::new("duplicate").unwrap(),
                Some("Player"),
                Utc::now(),
            )
            .unwrap_err();

        assert!(matches!(error, AccountError::AmbiguousAccountId(_)));
    }

    #[test]
    fn registry_manifest_disagreement_blocks_registration() {
        let (temp, store) = store();
        let entry = register(&store, "registry-id", "Player");
        let root = StorageRoot::from_path(temp.path()).unwrap();
        let paths = AccountPaths::new(root.clone(), entry.key());
        write_json_atomic(
            &root,
            paths.relative_manifest(),
            &ProfileManifest::new(
                entry.key(),
                AccountId::new("manifest-id").unwrap(),
                entry.created_at(),
            ),
            BackupPolicy::Single,
        )
        .unwrap();

        let error = store
            .register_verified(
                AccountId::new("registry-id").unwrap(),
                Some("Renamed"),
                Utc::now(),
            )
            .unwrap_err();

        assert!(matches!(error, AccountError::AmbiguousAccountId(_)));
    }

    #[test]
    fn future_profile_is_reported_without_adoption() {
        let (temp, store) = store();
        let root = StorageRoot::from_path(temp.path()).unwrap();
        let key = AccountKey::generate();
        let paths = AccountPaths::new(root.clone(), key);
        root.create_directory(paths.relative_root()).unwrap();
        let mut value = serde_json::to_value(ProfileManifest::new(
            key,
            AccountId::new("future").unwrap(),
            Utc::now(),
        ))
        .unwrap();
        value["version"] = json!(2);
        write_json_atomic(
            &root,
            paths.relative_manifest(),
            &value,
            BackupPolicy::Single,
        )
        .unwrap();

        let reconciled = store.reconcile().unwrap();

        assert!(reconciled.registry.entry(key).is_none());
        assert!(matches!(
            reconciled.report.health_issues[0].kind,
            ProfileHealthKind::UnsupportedManifestVersion(2)
        ));
    }

    #[test]
    fn invalid_profile_directory_name_is_reported() {
        let (temp, store) = store();
        fs::create_dir_all(temp.path().join("accounts").join("not-an-account-key")).unwrap();

        let reconciled = store.reconcile().unwrap();

        assert!(matches!(
            reconciled.report.health_issues[0].kind,
            ProfileHealthKind::InvalidDirectoryKey
        ));
    }

    #[test]
    fn manifest_directory_key_mismatch_is_not_adopted() {
        let (temp, store) = store();
        let root = StorageRoot::from_path(temp.path()).unwrap();
        let directory_key = AccountKey::generate();
        let manifest_key = AccountKey::generate();
        let paths = AccountPaths::new(root.clone(), directory_key);
        root.create_directory(paths.relative_root()).unwrap();
        write_json_atomic(
            &root,
            paths.relative_manifest(),
            &ProfileManifest::new(
                manifest_key,
                AccountId::new("mismatch").unwrap(),
                Utc::now(),
            ),
            BackupPolicy::Single,
        )
        .unwrap();

        let reconciled = store.reconcile().unwrap();

        assert!(reconciled.registry.accounts().is_empty());
        assert!(matches!(
            reconciled.report.health_issues[0].kind,
            ProfileHealthKind::ManifestKeyMismatch { .. }
        ));
    }

    #[test]
    fn orphan_identity_conflict_is_reported_without_rebinding() {
        let (temp, store) = store();
        let entry = register(&store, "shared-id", "Registered");
        let root = StorageRoot::from_path(temp.path()).unwrap();
        let orphan_key = AccountKey::generate();
        write_manifest(&root, orphan_key, "shared-id");

        let reconciled = store.reconcile().unwrap();

        assert_eq!(
            reconciled
                .registry
                .find_by_account_id(&AccountId::new("shared-id").unwrap())
                .unwrap()
                .key(),
            entry.key()
        );
        assert!(reconciled.report.health_issues.iter().any(|issue| {
            matches!(issue.kind, ProfileHealthKind::DuplicateProfileAccountId(_))
        }));
    }

    #[test]
    fn recovered_manifest_backup_remains_authoritative() {
        let (temp, store) = store();
        let entry = register(&store, "account-a", "A");
        let root = StorageRoot::from_path(temp.path()).unwrap();
        let paths = AccountPaths::new(root.clone(), entry.key());
        let manifest: ProfileManifest = match load_json(&root, paths.relative_manifest()).unwrap() {
            LoadOutcome::Primary(manifest) => manifest,
            _ => panic!("expected primary manifest"),
        };
        write_json_atomic(
            &root,
            paths.relative_manifest(),
            &manifest,
            BackupPolicy::Single,
        )
        .unwrap();
        fs::write(paths.manifest().unwrap(), b"{").unwrap();

        let reconciled = store.reconcile().unwrap();

        assert_eq!(
            reconciled.report.recovered_manifest_backups,
            vec![entry.key()]
        );
        assert!(reconciled.registry.entry(entry.key()).is_some());
    }

    #[test]
    fn pending_recovery_and_orphan_adoption_share_one_result() {
        let (temp, store) = store();
        let selected = register(&store, "selected", "Selected");
        store.select(selected.key(), Utc::now()).unwrap();
        store.begin_discovery(Utc::now()).unwrap();
        let root = StorageRoot::from_path(temp.path()).unwrap();
        let orphan_key = AccountKey::generate();
        write_manifest(&root, orphan_key, "orphan");

        let reconciled = store.reconcile().unwrap();

        assert_eq!(
            reconciled.registry.selected_account_key(),
            Some(selected.key())
        );
        assert_eq!(reconciled.report.adopted_account_keys, vec![orphan_key]);
        assert!(reconciled.report.cleared_pending_transition.is_some());
    }

    #[test]
    fn missing_selected_profile_is_reported_without_replacement() {
        let (temp, store) = store();
        let entry = register(&store, "account-a", "A");
        store.select(entry.key(), Utc::now()).unwrap();
        fs::remove_file(
            temp.path()
                .join("accounts")
                .join(entry.key().to_string())
                .join("profile.json"),
        )
        .unwrap();

        let reconciled = store.reconcile().unwrap();

        assert_eq!(
            reconciled.registry.selected_account_key(),
            Some(entry.key())
        );
        assert!(reconciled.report.health_issues.iter().any(|issue| matches!(
            issue.kind,
            ProfileHealthKind::MissingRegisteredProfile(key) if key == entry.key()
        )));
    }

    #[test]
    fn shorten_duplicate_keys_unique_names() {
        let (_temp, store) = store();
        let a = register(&store, "account-a", "Alice");
        let b = register(&store, "account-b", "Bob");
        let registry = store.reconcile().unwrap().registry;
        let shortened = shorten_duplicate_keys(registry.accounts());
        assert!(shortened.is_empty(), "unique names need no suffix");
        // Verify entries are there
        assert!(registry.entry(a.key()).is_some());
        assert!(registry.entry(b.key()).is_some());
    }

    #[test]
    fn shorten_duplicate_keys_same_name() {
        let (_temp, store) = store();
        let a = register(&store, "account-a", "Player");
        let b = register(&store, "account-b", "Player");
        let registry = store.reconcile().unwrap().registry;
        let shortened = shorten_duplicate_keys(registry.accounts());
        assert_eq!(shortened.len(), 2);
        let suffix_a = &shortened[&a.key()];
        let suffix_b = &shortened[&b.key()];
        assert_ne!(suffix_a, suffix_b, "suffixes must differ");
        assert!(suffix_a.len() >= 4);
    }

    #[test]
    fn shorten_duplicate_keys_none_names_ignored() {
        let (_temp, store) = store();
        register_no_name(&store, "account-a");
        register_no_name(&store, "account-b");
        let registry = store.reconcile().unwrap().registry;
        let shortened = shorten_duplicate_keys(registry.accounts());
        assert!(shortened.is_empty(), "None names don't collide");
    }

    #[test]
    fn commit_discovery_fails_without_begin() {
        let (_temp, store) = store();
        let entry = register(&store, "account-a", "A");
        store.select(entry.key(), Utc::now()).unwrap();

        let result = store.commit_discovery();
        assert!(result.is_err());
    }

    #[test]
    fn register_verified_during_discovering_does_not_change_mode() {
        let (_temp, store) = store();
        let entry = register(&store, "account-a", "A");
        store.select(entry.key(), Utc::now()).unwrap();
        store.begin_discovery(Utc::now()).unwrap();
        store.commit_discovery().unwrap();

        let registered = store
            .register_verified(AccountId::new("account-b").unwrap(), Some("B"), Utc::now())
            .unwrap();

        let registry = store.reconcile().unwrap().registry;
        assert_eq!(registry.mode(), RegistryMode::Discovering);
        assert!(registry.entry(registered.key()).is_some());
    }

    #[test]
    fn full_discovery_flow_register_and_select() {
        let (_temp, store) = store();
        let original = register(&store, "account-a", "A");
        store.select(original.key(), Utc::now()).unwrap();

        store.begin_discovery(Utc::now()).unwrap();
        store.commit_discovery().unwrap();

        let registry = store.reconcile().unwrap().registry;
        assert_eq!(registry.mode(), RegistryMode::Discovering);
        assert_eq!(registry.previous_account_key(), Some(original.key()));

        let new_entry = store
            .register_verified(AccountId::new("account-b").unwrap(), Some("B"), Utc::now())
            .unwrap();

        store.select(new_entry.key(), Utc::now()).unwrap();

        let final_reg = store.reconcile().unwrap().registry;
        assert_eq!(final_reg.mode(), RegistryMode::Selected);
        assert_eq!(final_reg.selected_account_key(), Some(new_entry.key()));
        assert_eq!(final_reg.previous_account_key(), None);
    }

    #[test]
    fn delete_account_removes_profile_registry_and_credential() {
        let (_temp, store) = store();
        let creds = super::super::InMemoryCredentialStore::new();

        let entry = register(&store, "acc-1", "Alice");
        let key = entry.key();

        // Bind a credential.
        let target = super::super::credential_target(key);
        creds
            .write(&target, &super::super::SavedCredential::new("tok", None))
            .unwrap();

        // Confirm profile directory exists before deletion.
        let paths = AccountPaths::new(store.storage_root.clone(), key);
        assert!(store
            .storage_root
            .directory_exists(paths.relative_root())
            .unwrap());

        // Create a sibling lock file like ProfileLock would.
        let lock_relative =
            super::super::migration::lock::target_lock_relative(&paths.relative_root());
        let lock_path = store.storage_root.resolve(&lock_relative).unwrap();
        fs::write(&lock_path, b"").unwrap();
        assert!(lock_path.exists());

        let updated = store.delete_account(key, &creds).unwrap();

        assert!(updated.accounts().is_empty());
        assert!(!store
            .storage_root
            .directory_exists(paths.relative_root())
            .unwrap());
        assert!(creds.read(&target).unwrap().is_none());
        assert!(!lock_path.exists());
    }

    #[test]
    fn delete_selected_account_enters_discovery() {
        let (_temp, store) = store();
        let creds = super::super::InMemoryCredentialStore::new();

        let entry = register(&store, "acc-1", "Alice");
        store.select(entry.key(), Utc::now()).unwrap();

        let updated = store.delete_account(entry.key(), &creds).unwrap();

        assert_eq!(updated.mode(), RegistryMode::Discovering);
        assert_eq!(updated.selected_account_key(), None);
    }

    #[test]
    fn delete_nonexistent_account_returns_error() {
        let (_temp, store) = store();
        let creds = super::super::InMemoryCredentialStore::new();
        let key = AccountKey::generate();

        assert!(store.delete_account(key, &creds).is_err());
    }
}

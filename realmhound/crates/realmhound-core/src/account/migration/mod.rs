//! Recoverable flat-layout account migration.
//!
//! This module owns the one-time migration from the legacy single-account flat
//! layout into an isolated account profile (known account) or a validated
//! quarantine awaiting attribution (unknown account). It is built to be safe to
//! interrupt and resume at any point: files are copied and validated before any
//! original is moved, an atomic versioned [`journal`] records progress across
//! five known states, and OS-backed [`lock`]s prevent concurrent runs.
//!
//! Migration never resolves a real `%LOCALAPPDATA%`/`%APPDATA%`; callers supply
//! an explicit local [`StorageRoot`] and an optional explicit Roaming root. It
//! never runs an application database writer/initializer, never copies the
//! plaintext token into any profile/quarantine/backup, and never automatically
//! deletes a backup.

pub mod backup;
pub mod inventory;
pub mod journal;
pub mod lock;
pub mod settings_view;
pub mod sqlite;
pub mod validate;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use thiserror::Error;

use inventory::{flat_layout_inventory, InventoryItem, SourceClass, SourceLocation, Validator};
use journal::{
    account_identity_hash, inventory_fingerprint, load_journal, save_journal, CredentialOutcome,
    MigrationJournal, MigrationKind, MigrationPhase, RecoveryStatus,
};
use lock::MigrationLock;
use settings_view::read_legacy_account_settings;

use crate::account::{
    AccountError, AccountId, AccountKey, AccountPaths, AccountRegistryStore, CredentialError,
    CredentialStore, MigrationKnownBinding, ProfileManifest, SavedCredential,
};
use crate::storage::{
    load_json, write_json_atomic_at, BackupPolicy, LoadOutcome, StorageError, StorageRoot,
};
use crate::vault::{parse_legacy_character_cache, parse_legacy_vault, AccountData};

/// Root-relative quarantine directory for unknown-account data.
pub const QUARANTINE_DIR: &str = "quarantine/legacy-unassigned";

/// Errors produced by flat-layout migration. No variant carries token contents.
#[derive(Debug, Error)]
pub enum MigrationError {
    /// A validated storage operation failed.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// A registry or profile operation failed.
    #[error(transparent)]
    Account(#[from] AccountError),
    /// Reading a legacy settings file failed.
    #[error("failed to read legacy settings `{path}`: {source}")]
    SettingsRead {
        /// Settings path.
        path: PathBuf,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },
    /// A legacy settings file could not be parsed.
    #[error("legacy settings `{path}` are not valid JSON: {source}")]
    SettingsParse {
        /// Settings path.
        path: PathBuf,
        /// Parse error.
        #[source]
        source: serde_json::Error,
    },
    /// A saved account ID in settings was empty or invalid.
    #[error("legacy settings `{path}` contain an invalid account id: {source}")]
    SettingsAccountId {
        /// Settings path.
        path: PathBuf,
        /// Identity error.
        #[source]
        source: crate::account::AccountIdError,
    },
    /// A lock file could not be opened or created.
    #[error("failed to prepare migration lock `{path}`: {source}")]
    LockIo {
        /// Lock path.
        path: PathBuf,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },
    /// Another owner already holds a migration lock.
    #[error("migration lock `{path}` is held by another owner")]
    LockContended {
        /// Lock path.
        path: PathBuf,
    },
    /// A JSON source failed to parse during validation.
    #[error("source is not valid JSON: {source}")]
    JsonInvalid {
        /// Parse error.
        #[source]
        source: serde_json::Error,
    },
    /// A `char_list.xml` source failed the syntactic XML check.
    #[error("char_list.xml is not well-formed: {detail}")]
    XmlInvalid {
        /// Explanation.
        detail: String,
    },
    /// A RealmShark `dungeon.stats` source failed the syntactic check.
    #[error("dungeon.stats is not a valid RealmShark document: {source}")]
    RealmSharkInvalid {
        /// Parse error.
        #[source]
        source: serde_json::Error,
    },
    /// A copy IO operation failed.
    #[error("migration copy failed for `{path}`: {source}")]
    CopyIo {
        /// Affected path.
        path: PathBuf,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },
    /// A copied file/tree did not match its source content.
    #[error("migration copy of `{path}` did not match its source")]
    CopyMismatch {
        /// Affected destination path.
        path: PathBuf,
    },
    /// A symbolic link or Windows reparse point was encountered.
    #[error("symbolic links and reparse points are not allowed: `{path}`")]
    LinkNotAllowed {
        /// Offending path.
        path: PathBuf,
    },
    /// A SQLite operation failed.
    #[error("sqlite migration for `{label}` failed: {detail}")]
    Sqlite {
        /// Database label.
        label: &'static str,
        /// Explanation.
        detail: String,
    },
    /// A SQLite destination already exists.
    #[error("sqlite destination already exists: `{path}`")]
    DatabaseDestinationExists {
        /// Destination path.
        path: PathBuf,
    },
    /// A SQLite destination has no valid parent directory.
    #[error("sqlite destination is invalid: `{path}`")]
    DatabaseInvalidDestination {
        /// Destination path.
        path: PathBuf,
    },
    /// The WAL checkpoint reported a busy database.
    #[error("sqlite checkpoint for `{label}` was busy")]
    DatabaseCheckpointBusy {
        /// Database label.
        label: &'static str,
    },
    /// The WAL checkpoint did not fully consolidate.
    #[error("sqlite checkpoint for `{label}` incomplete (log {log}, checkpointed {checkpointed})")]
    DatabaseCheckpointIncomplete {
        /// Database label.
        label: &'static str,
        /// Total WAL frames.
        log: i64,
        /// Frames checkpointed.
        checkpointed: i64,
    },
    /// A non-empty WAL remained after checkpoint.
    #[error("sqlite WAL for `{label}` was not truncated")]
    DatabaseWalRemains {
        /// Database label.
        label: &'static str,
    },
    /// `quick_check` reported an integrity problem.
    #[error("sqlite integrity check for `{label}` failed: {detail}")]
    DatabaseIntegrity {
        /// Database label.
        label: &'static str,
        /// Reported detail.
        detail: String,
    },
    /// The database `user_version` exceeds the supported version.
    #[error("sqlite `{label}` version {found} exceeds supported version {supported}")]
    DatabaseVersionUnsupported {
        /// Database label.
        label: &'static str,
        /// Found version.
        found: i32,
        /// Supported version.
        supported: i32,
    },
    /// A required table is missing from the database.
    #[error("sqlite `{label}` is missing required table `{table}`")]
    DatabaseMissingTable {
        /// Database label.
        label: &'static str,
        /// Missing table name.
        table: String,
    },
    /// The journal document version is unsupported.
    #[error("unsupported migration journal version {version}")]
    JournalVersionUnsupported {
        /// Found version.
        version: u32,
    },
    /// The journal disagrees with the current run.
    #[error("migration journal mismatch: {detail}")]
    JournalMismatch {
        /// Explanation.
        detail: String,
    },
    /// The journal document is corrupt.
    #[error("migration journal is corrupt: {detail}")]
    JournalCorrupt {
        /// Explanation.
        detail: String,
    },
    /// Backup deletion was requested without explicit confirmation.
    #[error("migration backup deletion requires explicit confirmation")]
    DeletionNotConfirmed,
    /// Backup deletion targeted a path outside the backup root.
    #[error("migration backup deletion path is not contained in the backup root")]
    DeletionNotContained,
    /// Backup deletion was requested before the retention threshold.
    #[error("migration backup is not yet eligible for deletion")]
    DeletionNotEligible,
    /// The legacy plaintext access token file could not be parsed.
    ///
    /// The malformed file is left in place and the phase does not advance so the
    /// import can be retried. No variant ever carries token contents.
    #[error("legacy access token file is malformed")]
    TokenMalformed,
    /// A filesystem existence probe failed for a reason other than absence
    /// (for example a permission or metadata error), so absence cannot be
    /// assumed.
    #[error("could not determine whether `{path}` exists: {source}")]
    PathProbe {
        /// Probed path.
        path: PathBuf,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },
    /// A required migration invariant did not hold (never token-bearing).
    #[error("migration invariant violated: {detail}")]
    Invariant {
        /// Explanation.
        detail: String,
    },
    /// Test-only simulated interruption at a persisted boundary.
    #[cfg(test)]
    #[error("migration interrupted at `{at}`")]
    Interrupted {
        /// Boundary label.
        at: String,
    },
}

/// Structured, redacted result of a migration attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// A known account migrated (or resumed) to a selected profile.
    ReadyKnown {
        /// The selected account key.
        account_key: AccountKey,
        /// Root-relative timestamped backup directory.
        backup_directory: String,
        /// Recovery status for follow-up actions.
        recovery: RecoveryStatus,
    },
    /// An unknown account was quarantined and awaits attribution.
    AwaitingAttribution {
        /// Root-relative timestamped backup directory.
        backup_directory: String,
    },
    /// A previously completed migration required no work.
    AlreadyComplete {
        /// The selected account key, if known.
        account_key: Option<AccountKey>,
    },
    /// Migration could not proceed; no migration source was changed.
    Blocked {
        /// Actionable, redacted reason.
        reason: String,
    },
}

/// Post-discovery attribution evidence. A token is never evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttributionEvidence {
    /// A source embeds an account id that matches the verified account.
    StrongMatch {
        /// Owner id embedded in quarantined data.
        embedded_account_id: AccountId,
        /// Verified account id observed after discovery.
        verified_account_id: AccountId,
    },
    /// The user explicitly confirmed the quarantined data belongs to them.
    ExplicitConfirmation,
    /// No strong evidence and no confirmation.
    Unconfirmed,
}

/// Decision for attaching quarantined data to a discovered account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributionDecision {
    /// Attach the quarantined data to the account.
    Attach,
    /// Require explicit user confirmation before attaching.
    RequireConfirmation,
}

/// Decide whether quarantined data may attach to a discovered account.
///
/// Strong matching embedded identity or explicit confirmation attaches; anything
/// else requires confirmation. A token is deliberately not an input.
pub fn decide_attribution(evidence: &AttributionEvidence) -> AttributionDecision {
    match evidence {
        AttributionEvidence::StrongMatch {
            embedded_account_id,
            verified_account_id,
        } if embedded_account_id == verified_account_id => AttributionDecision::Attach,
        AttributionEvidence::ExplicitConfirmation => AttributionDecision::Attach,
        _ => AttributionDecision::RequireConfirmation,
    }
}

/// A recoverable flat-layout migration bound to explicit roots.
pub struct FlatLayoutMigration {
    local: StorageRoot,
    roaming: Option<StorageRoot>,
    registry: AccountRegistryStore,
    credentials: Arc<dyn CredentialStore>,
    /// Test-only armed interruption points, keyed by boundary label.
    #[cfg(test)]
    failpoints: std::sync::Mutex<std::collections::HashSet<String>>,
}

impl FlatLayoutMigration {
    /// Create a migration over an explicit local root and optional Roaming root.
    pub fn new(
        local: StorageRoot,
        roaming: Option<StorageRoot>,
        credentials: Arc<dyn CredentialStore>,
    ) -> Self {
        let registry = AccountRegistryStore::new(local.clone());
        Self {
            local,
            roaming,
            registry,
            credentials,
            #[cfg(test)]
            failpoints: std::sync::Mutex::new(std::collections::HashSet::new()),
        }
    }

    /// Arm a test-only interruption at a persisted boundary. The boundary fires
    /// once its label has been recorded (see [`Self::check_failpoint`]).
    #[cfg(test)]
    pub(crate) fn arm_failpoint(&self, at: impl Into<String>) {
        self.failpoints.lock().unwrap().insert(at.into());
    }

    /// Return a simulated interruption error when a boundary is armed.
    #[cfg(test)]
    fn check_failpoint(&self, at: &str) -> Result<(), MigrationError> {
        if self.failpoints.lock().unwrap().contains(at) {
            return Err(MigrationError::Interrupted { at: at.to_string() });
        }
        Ok(())
    }

    /// No-op in non-test builds; interruption boundaries have zero cost.
    #[cfg(not(test))]
    #[inline]
    fn check_failpoint(&self, _at: &str) -> Result<(), MigrationError> {
        Ok(())
    }

    /// Cheap, lock-free peek at whether migration still has work to do.
    ///
    /// Reads only the journal to decide, so it never mutates state or acquires
    /// the root lock; the authoritative migration in [`Self::run_at`] runs
    /// regardless. A `Complete` journal means nothing to do; a missing or
    /// unreadable journal, or any earlier phase, errs toward pending so the
    /// caller can surface progress.
    pub fn pending(&self) -> bool {
        match load_journal(&self.local) {
            Ok(Some(journal)) => journal.phase != MigrationPhase::Complete,
            Ok(None) => true,
            Err(_) => true,
        }
    }

    /// Run or resume the migration using the current time.
    pub fn run(&self) -> Result<MigrationOutcome, MigrationError> {
        self.run_at(Utc::now())
    }

    /// Run or resume the migration at an explicit time (for tests).
    pub fn run_at(&self, now: DateTime<Utc>) -> Result<MigrationOutcome, MigrationError> {
        let _root_lock = match MigrationLock::acquire_root(&self.local) {
            Ok(lock) => lock,
            Err(MigrationError::LockContended { path }) => {
                return Ok(MigrationOutcome::Blocked {
                    reason: format!(
                        "another RealmHound instance owns migration ({})",
                        path.display()
                    ),
                })
            }
            Err(other) => return Err(other),
        };

        // Load the journal first. After the registry has been committed the
        // migration is self-describing and must not depend on mutable settings.
        let existing = load_journal(&self.local)?;
        if let Some(journal) = existing.as_ref() {
            if !phase_before(journal.phase, MigrationPhase::RegistryCommitted) {
                if journal.phase != MigrationPhase::Complete {
                    tracing::info!(
                        "[MIGRATION] Resuming migration after registry commit (phase={:?})",
                        journal.phase
                    );
                }
                return self.resume_after_commit(journal.clone(), now);
            }
        }

        // Precommit: settings are authoritative and strictly required.
        let settings_path = self.local.ensure_no_links("settings.json")?;
        let settings = read_legacy_account_settings(&settings_path)?;
        let known_identity = settings.as_ref().and_then(|s| s.account_id.clone());
        let kind = if known_identity.is_some() {
            MigrationKind::Known
        } else {
            MigrationKind::Unknown
        };

        let has_roaming = self.roaming.is_some();
        let items = flat_layout_inventory(has_roaming);
        let item_ids: Vec<&str> = items.iter().map(|item| item.id).collect();
        let fingerprint = inventory_fingerprint(kind, &item_ids);

        let resuming = existing.is_some();
        let mut journal = match existing {
            Some(existing) => {
                self.verify_resumable(
                    &existing,
                    kind,
                    &fingerprint,
                    known_identity.as_ref(),
                    &item_ids,
                )?;
                existing
            }
            None => {
                // A brand-new journal must not adopt foreign quarantine content.
                if let Some(reason) = self.preexisting_quarantine_conflict()? {
                    return Ok(MigrationOutcome::Blocked { reason });
                }
                let (target_key, identity_hash) = match kind {
                    MigrationKind::Known => (
                        Some(AccountKey::generate()),
                        known_identity.as_ref().map(account_identity_hash),
                    ),
                    MigrationKind::Unknown => (None, None),
                };
                let backup_directory = timestamped_backup_dir(now);
                let journal = MigrationJournal::prepared(
                    kind,
                    fingerprint,
                    target_key,
                    identity_hash,
                    backup_directory,
                    &item_ids,
                    now,
                );
                save_journal(&self.local, &journal)?;
                self.check_failpoint("prepared")?;
                journal
            }
        };

        tracing::info!(
            "[MIGRATION] {} flat-layout migration (kind={:?}, phase={:?}, {} sources)",
            if resuming { "Resuming" } else { "Starting" },
            journal.kind,
            journal.phase,
            item_ids.len()
        );

        match kind {
            MigrationKind::Known => {
                let settings = settings.ok_or_else(|| MigrationError::Invariant {
                    detail: "known migration lost its settings".to_string(),
                })?;
                self.run_known(&mut journal, &items, &settings, now)
            }
            MigrationKind::Unknown => self.run_unknown(&mut journal, &items, now),
        }
    }

    /// Detect a foreign, non-empty quarantine directory before a new journal is
    /// created. Once the Prepared journal owns the quarantine, partial exact
    /// destinations are migration-owned and retryable, so this check applies
    /// only when no journal yet exists.
    fn preexisting_quarantine_conflict(&self) -> Result<Option<String>, MigrationError> {
        let quarantine = Path::new(QUARANTINE_DIR);
        match self.local.directory_exists(quarantine) {
            Ok(false) => Ok(None),
            Ok(true) => {
                if self.local.read_directory(quarantine)?.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(format!(
                        "quarantine `{QUARANTINE_DIR}` already contains data; \
                         resolve it before migrating"
                    )))
                }
            }
            // A collision where the quarantine path is not a directory.
            Err(error) => Err(MigrationError::Storage(error)),
        }
    }

    fn verify_resumable(
        &self,
        journal: &MigrationJournal,
        kind: MigrationKind,
        fingerprint: &str,
        known_identity: Option<&AccountId>,
        item_ids: &[&str],
    ) -> Result<(), MigrationError> {
        if journal.kind != kind {
            return Err(MigrationError::JournalMismatch {
                detail: "migration kind changed between runs".to_string(),
            });
        }
        if journal.inventory_fingerprint != fingerprint {
            return Err(MigrationError::JournalMismatch {
                detail: "inventory fingerprint changed between runs".to_string(),
            });
        }
        // The journal's item ids must exactly equal the current inventory.
        journal.verify_item_ids_exhaustive(item_ids)?;
        // A known target binding must not change identity between runs. The raw
        // account id is never stored; a non-reversible hash is re-derived from
        // settings on every precommit resume and compared.
        if kind == MigrationKind::Known {
            let current = known_identity.map(account_identity_hash);
            if journal.known_identity_hash != current {
                return Err(MigrationError::JournalMismatch {
                    detail: "settings account identity changed between runs".to_string(),
                });
            }
        }
        Ok(())
    }

    fn run_known(
        &self,
        journal: &mut MigrationJournal,
        items: &[InventoryItem],
        settings: &settings_view::LegacyAccountSettings,
        now: DateTime<Utc>,
    ) -> Result<MigrationOutcome, MigrationError> {
        let account_id = settings
            .account_id
            .clone()
            .ok_or_else(|| MigrationError::Invariant {
                detail: "known migration has no account id".to_string(),
            })?;
        let account_key = journal
            .target_account_key
            .ok_or_else(|| MigrationError::Invariant {
                detail: "known journal has no target key".to_string(),
            })?;

        if journal.phase == MigrationPhase::Complete {
            return Ok(MigrationOutcome::AlreadyComplete {
                account_key: Some(account_key),
            });
        }

        let _target_lock = MigrationLock::acquire_target(
            &self.local,
            &AccountPaths::new(self.local.clone(), account_key).relative_root(),
        )?;

        // Prepared: ensure the profile + manifest exist under the exact key.
        self.registry.create_migration_profile(
            account_key,
            account_id.clone(),
            journal.backup.created_at,
        )?;

        let paths = AccountPaths::new(self.local.clone(), account_key);

        if phase_before(journal.phase, MigrationPhase::RegistryCommitted) {
            // Never trust journal `validated` flags on resume: revalidate each
            // completed destination against its digest/schema and clear any that
            // are missing or corrupt so the copy pass recreates them. The
            // destination is migration-owned because the Prepared journal exists.
            self.revalidate_known_destinations(journal, items, &paths, &account_id)?;
            tracing::info!("[MIGRATION] Copying account data and databases into the profile");
            self.copy_known_sources(journal, items, &paths, &account_id)?;
            if phase_before(journal.phase, MigrationPhase::CopiesValidated) {
                journal.phase = MigrationPhase::CopiesValidated;
                save_journal(&self.local, journal)?;
                self.check_failpoint("copies-validated")?;
            }
        }

        // Credential import happens before registry commit so the credential
        // target can be recorded atomically with the selection.
        let credential_target = self.import_known_token(journal)?;

        if phase_before(journal.phase, MigrationPhase::RegistryCommitted) {
            let binding = MigrationKnownBinding {
                account_key,
                account_id: account_id.clone(),
                display_name: settings.account_name.clone(),
                verified_at: now,
                last_client_seen_unix: settings.last_client_seen_unix,
                last_client_launch_unix: settings.last_client_launch_unix,
                credential_target: credential_target.clone(),
            };
            tracing::info!("[MIGRATION] Committing account selection to the registry");
            self.registry.commit_migration_known(&binding)?;
            journal.phase = MigrationPhase::RegistryCommitted;
            journal.recovery = RecoveryStatus::ResumableAfterCommit;
            save_journal(&self.local, journal)?;
            self.check_failpoint("registry-committed")?;
        }

        self.finish_known_after_commit(journal, items, account_key)
    }

    /// Shared post-commit tail for known migration: back up non-credential
    /// originals, remove the plaintext token, and complete. Reached both on a
    /// first run after commit and on a settings-independent resume.
    fn finish_known_after_commit(
        &self,
        journal: &mut MigrationJournal,
        items: &[InventoryItem],
        account_key: AccountKey,
    ) -> Result<MigrationOutcome, MigrationError> {
        if phase_before(journal.phase, MigrationPhase::OriginalsBackedUp) {
            tracing::info!("[MIGRATION] Backing up and removing legacy originals");
            self.back_up_originals(journal, items)?;
            self.remove_plaintext_token(journal)?;
            journal.phase = MigrationPhase::OriginalsBackedUp;
            save_journal(&self.local, journal)?;
            self.check_failpoint("originals-backed-up")?;
        }

        journal.phase = MigrationPhase::Complete;
        journal.recovery = RecoveryStatus::Healthy;
        save_journal(&self.local, journal)?;

        tracing::info!("[MIGRATION] Flat-layout migration complete (account_key={account_key})");
        Ok(MigrationOutcome::ReadyKnown {
            account_key,
            backup_directory: journal.backup_directory.clone(),
            recovery: journal.recovery,
        })
    }

    fn run_unknown(
        &self,
        journal: &mut MigrationJournal,
        items: &[InventoryItem],
        _now: DateTime<Utc>,
    ) -> Result<MigrationOutcome, MigrationError> {
        if journal.phase == MigrationPhase::AwaitingAttribution {
            return Ok(MigrationOutcome::AwaitingAttribution {
                backup_directory: journal.backup_directory.clone(),
            });
        }

        let _quarantine_lock =
            MigrationLock::acquire_target(&self.local, Path::new(QUARANTINE_DIR))?;

        if phase_before(journal.phase, MigrationPhase::RegistryCommitted) {
            tracing::info!("[MIGRATION] Quarantining unattributed legacy data");
            self.revalidate_unknown_destinations(journal, items)?;
            self.copy_unknown_sources(journal, items)?;
            if phase_before(journal.phase, MigrationPhase::CopiesValidated) {
                journal.phase = MigrationPhase::CopiesValidated;
                save_journal(&self.local, journal)?;
                self.check_failpoint("copies-validated")?;
            }
        }

        if phase_before(journal.phase, MigrationPhase::RegistryCommitted) {
            self.registry.commit_migration_discovery()?;
            journal.phase = MigrationPhase::RegistryCommitted;
            journal.recovery = RecoveryStatus::ResumableAfterCommit;
            save_journal(&self.local, journal)?;
            self.check_failpoint("registry-committed")?;
        }

        self.finish_unknown_after_commit(journal, items)
    }

    /// Shared post-commit tail for unknown migration.
    fn finish_unknown_after_commit(
        &self,
        journal: &mut MigrationJournal,
        items: &[InventoryItem],
    ) -> Result<MigrationOutcome, MigrationError> {
        if phase_before(journal.phase, MigrationPhase::OriginalsBackedUp) {
            self.back_up_originals(journal, items)?;
            journal.phase = MigrationPhase::OriginalsBackedUp;
            save_journal(&self.local, journal)?;
            self.check_failpoint("originals-backed-up")?;
        }

        journal.phase = MigrationPhase::AwaitingAttribution;
        journal.recovery = RecoveryStatus::Healthy;
        save_journal(&self.local, journal)?;

        Ok(MigrationOutcome::AwaitingAttribution {
            backup_directory: journal.backup_directory.clone(),
        })
    }

    /// Resume a migration whose registry commit already succeeded without
    /// parsing mutable/corrupt settings. Ownership and identity are derived from
    /// the validated journal, profile manifest, and registry.
    fn resume_after_commit(
        &self,
        mut journal: MigrationJournal,
        _now: DateTime<Utc>,
    ) -> Result<MigrationOutcome, MigrationError> {
        let items = flat_layout_inventory(self.roaming.is_some());
        let item_ids: Vec<&str> = items.iter().map(|item| item.id).collect();
        journal.verify_item_ids_exhaustive(&item_ids)?;

        match journal.kind {
            MigrationKind::Known => {
                let account_key =
                    journal
                        .target_account_key
                        .ok_or_else(|| MigrationError::Invariant {
                            detail: "committed known journal has no target key".to_string(),
                        })?;
                if journal.phase == MigrationPhase::Complete {
                    return Ok(MigrationOutcome::AlreadyComplete {
                        account_key: Some(account_key),
                    });
                }
                // Prove destination ownership through the validated manifest,
                // not settings, which may now be corrupt or altered.
                let paths = AccountPaths::new(self.local.clone(), account_key);
                match load_json::<ProfileManifest>(&self.local, &paths.relative_manifest())
                    .map_err(MigrationError::Storage)?
                {
                    LoadOutcome::Primary(manifest) | LoadOutcome::RecoveredBackup(manifest) => {
                        if manifest.account_key() != account_key {
                            return Err(MigrationError::JournalMismatch {
                                detail: "committed profile manifest key mismatch".to_string(),
                            });
                        }
                    }
                    LoadOutcome::Missing => {
                        return Err(MigrationError::JournalMismatch {
                            detail: "committed profile manifest is missing".to_string(),
                        })
                    }
                }
                let _target_lock =
                    MigrationLock::acquire_target(&self.local, &paths.relative_root())?;
                self.finish_known_after_commit(&mut journal, &items, account_key)
            }
            MigrationKind::Unknown => {
                if journal.phase == MigrationPhase::AwaitingAttribution {
                    return Ok(MigrationOutcome::AwaitingAttribution {
                        backup_directory: journal.backup_directory.clone(),
                    });
                }
                let _quarantine_lock =
                    MigrationLock::acquire_target(&self.local, Path::new(QUARANTINE_DIR))?;
                self.finish_unknown_after_commit(&mut journal, &items)
            }
        }
    }

    fn copy_known_sources(
        &self,
        journal: &mut MigrationJournal,
        items: &[InventoryItem],
        paths: &AccountPaths,
        settings_account_id: &AccountId,
    ) -> Result<(), MigrationError> {
        // Independently detect contradictions in the four identity-bearing
        // sources so only contradictory sources are quarantined.
        let account_data_contradicts =
            self.source_contradicts("account_data.json", settings_account_id)?;
        let local_cache_contradicts =
            self.source_contradicts("characters_cache.json", settings_account_id)?;
        let roaming_cache_contradicts =
            self.source_contradicts("roaming_characters_cache.json", settings_account_id)?;
        let char_list_contradicts = self.char_list_contradicts(settings_account_id)?;

        for item in items {
            match item.class {
                SourceClass::Attributable
                | SourceClass::Ambiguous
                | SourceClass::ActiveDatabase
                | SourceClass::Obsolete => {}
                _ => continue,
            }
            if item_done(journal, item.id) {
                continue;
            }

            // Backup-only sources are copied into the fixed backup directory and
            // validated by hash/tree equality; they are never rename-moved.
            if item.class == SourceClass::Obsolete {
                self.copy_backup_only_source(journal, item)?;
                self.check_failpoint(&format!("copy:{}", item.id))?;
                continue;
            }

            let contradicts = match item.id {
                "account_data.json" => account_data_contradicts,
                "characters_cache.json" => local_cache_contradicts,
                "roaming_characters_cache.json" => roaming_cache_contradicts,
                "char_list.xml" => char_list_contradicts,
                _ => false,
            };

            if item.id == "account_data.json" {
                // Canonical account_data is rebuilt with documented precedence.
                self.build_profile_account_data(
                    paths,
                    account_data_contradicts,
                    local_cache_contradicts,
                    roaming_cache_contradicts,
                )?;
                // A contradictory canonical source is also quarantined intact.
                if account_data_contradicts {
                    self.quarantine_identity_source(journal, item)?;
                }
                mark_validated(journal, item.id, None);
                save_journal(&self.local, journal)?;
                self.check_failpoint("copy:account_data.json")?;
                continue;
            }
            // The ambiguous character caches are consumed into account_data and
            // preserved only in the backup, unless they contradict settings, in
            // which case only the contradictory source is quarantined. A present
            // source is typed-validated; parse errors are never silently ignored.
            if matches!(
                item.id,
                "characters_cache.json" | "roaming_characters_cache.json"
            ) {
                self.validate_present_typed(item.id)?;
                if contradicts {
                    self.quarantine_identity_source(journal, item)?;
                }
                if item.id == "roaming_characters_cache.json" {
                    // Known migration preserves the imported Roaming cache in a
                    // distinct backup path; the external original is never moved.
                    self.preserve_roaming_cache_in_backup(journal, item)?;
                }
                mark_validated(journal, item.id, None);
                save_journal(&self.local, journal)?;
                self.check_failpoint(&format!("copy:{}", item.id))?;
                continue;
            }
            if item.id == "live_vault.json" {
                self.validate_present_typed(item.id)?;
                mark_validated(journal, item.id, None);
                save_journal(&self.local, journal)?;
                self.check_failpoint("copy:live_vault.json")?;
                continue;
            }

            let source = self.absolute_source(item)?;
            if !self.path_exists(&source)? {
                mark_validated(journal, item.id, None);
                save_journal(&self.local, journal)?;
                self.check_failpoint(&format!("copy:{}", item.id))?;
                continue;
            }

            let destination = if contradicts {
                self.quarantine_destination(item)?
            } else {
                self.profile_destination(item, paths)?
            };
            self.copy_one(journal, item, &source, &destination)?;
            save_journal(&self.local, journal)?;
            self.check_failpoint(&format!("copy:{}", item.id))?;
        }
        Ok(())
    }

    fn copy_unknown_sources(
        &self,
        journal: &mut MigrationJournal,
        items: &[InventoryItem],
    ) -> Result<(), MigrationError> {
        for item in items {
            match item.class {
                SourceClass::Attributable
                | SourceClass::Ambiguous
                | SourceClass::ActiveDatabase
                | SourceClass::Obsolete => {}
                _ => continue,
            }
            if item_done(journal, item.id) {
                continue;
            }
            if item.class == SourceClass::Obsolete {
                self.copy_backup_only_source(journal, item)?;
                self.check_failpoint(&format!("copy:{}", item.id))?;
                continue;
            }
            let source = self.absolute_source(item)?;
            if !self.path_exists(&source)? {
                mark_validated(journal, item.id, None);
                save_journal(&self.local, journal)?;
                self.check_failpoint(&format!("copy:{}", item.id))?;
                continue;
            }
            let destination = self.quarantine_destination(item)?;
            self.copy_one(journal, item, &source, &destination)?;
            save_journal(&self.local, journal)?;
            self.check_failpoint(&format!("copy:{}", item.id))?;
        }
        Ok(())
    }

    /// Copy a backup-only source into the fixed timestamped backup directory,
    /// proving hash/tree equality before its journal record is validated.
    fn copy_backup_only_source(
        &self,
        journal: &mut MigrationJournal,
        item: &InventoryItem,
    ) -> Result<(), MigrationError> {
        let source = self.absolute_source(item)?;
        if !self.path_exists(&source)? {
            mark_validated(journal, item.id, None);
            save_journal(&self.local, journal)?;
            return Ok(());
        }
        let destination = self.backup_destination(&journal.backup_directory, &item.source)?;
        self.copy_one(journal, item, &source, &destination)?;
        save_journal(&self.local, journal)?;
        Ok(())
    }

    /// Preserve the imported Roaming characters cache in a distinct backup path
    /// (`<backup>/roaming/characters_cache.json`) via validated copy. The
    /// external Roaming original is never moved.
    fn preserve_roaming_cache_in_backup(
        &self,
        journal: &mut MigrationJournal,
        item: &InventoryItem,
    ) -> Result<(), MigrationError> {
        let source = self.absolute_source(item)?;
        if !self.path_exists(&source)? {
            return Ok(());
        }
        let relative = PathBuf::from("roaming").join(&item.source);
        let destination = self.backup_destination(&journal.backup_directory, &relative)?;
        if self.path_exists(&destination)? {
            validate::remove_tree_no_links(&destination)?;
        }
        let digest = validate::copy_and_validate_file(&source, &destination, Validator::Json)?;
        if let Some(record) = journal.item_mut(item.id) {
            record.progress.digest = Some(hex::encode(digest));
        }
        Ok(())
    }

    /// Resolve a validated destination path inside the timestamped backup.
    fn backup_destination(
        &self,
        backup_directory: &str,
        relative: &Path,
    ) -> Result<PathBuf, MigrationError> {
        let combined = PathBuf::from(backup_directory).join(relative);
        Ok(self.local.ensure_no_links(&combined)?)
    }

    /// Typed validation of a present ambiguous source. Absent sources are Ok.
    fn validate_present_typed(&self, item_id: &str) -> Result<(), MigrationError> {
        let bytes = match item_id {
            "roaming_characters_cache.json" => self.read_roaming_characters_cache()?,
            other => self.read_optional(other)?,
        };
        let Some(bytes) = bytes else {
            return Ok(());
        };
        let text = String::from_utf8_lossy(&bytes);
        match item_id {
            "characters_cache.json" | "roaming_characters_cache.json" => {
                parse_legacy_character_cache(&text)
                    .map(|_| ())
                    .map_err(|source| MigrationError::JsonInvalid { source })
            }
            "live_vault.json" => parse_legacy_vault(&text)
                .map(|_| ())
                .map_err(|source| MigrationError::JsonInvalid { source }),
            _ => Ok(()),
        }
    }

    fn copy_one(
        &self,
        journal: &mut MigrationJournal,
        item: &InventoryItem,
        source: &Path,
        destination: &Path,
    ) -> Result<(), MigrationError> {
        match item.validator {
            Validator::Sqlite => {
                let spec = match item.id {
                    "loot_history.db" => sqlite::DbSpec::loot(),
                    "combat_history.db" => sqlite::DbSpec::combat(),
                    _ => sqlite::DbSpec::loot(),
                };
                if self.path_exists(destination)? {
                    // A resumed run must revalidate: discard the prior temp
                    // output by removing the destination and re-migrating.
                    let _ = std::fs::remove_file(destination);
                }
                sqlite::migrate_database(source, destination, &spec)?;
                mark_validated(journal, item.id, None);
            }
            Validator::OpaqueTree => {
                // An unjournaled partial destination tree from an interrupted
                // copy is migration-owned: remove it safely (refusing links and
                // reparse points) and recopy.
                if self.path_exists(destination)? {
                    validate::remove_tree_no_links(destination)?;
                }
                let manifest = validate::copy_and_validate_tree(source, destination)?;
                let digest = tree_digest(&manifest);
                mark_validated(journal, item.id, Some(digest));
            }
            Validator::None => {
                mark_validated(journal, item.id, None);
            }
            other => {
                if self.path_exists(destination)? {
                    let _ = std::fs::remove_file(destination);
                }
                let digest = validate::copy_and_validate_file(source, destination, other)?;
                mark_validated(journal, item.id, Some(hex::encode(digest)));
            }
        }
        Ok(())
    }

    /// Revalidate each already-completed known destination against its recorded
    /// digest/schema, clearing the journal flag of any that is missing or
    /// corrupt so the copy pass recreates it. The destination is migration-owned
    /// because the Prepared journal exists. Never trusts the `validated` flag.
    fn revalidate_known_destinations(
        &self,
        journal: &mut MigrationJournal,
        items: &[InventoryItem],
        paths: &AccountPaths,
        account_id: &AccountId,
    ) -> Result<(), MigrationError> {
        let char_list_contradicts = self.char_list_contradicts(account_id)?;
        let mut to_clear: Vec<&str> = Vec::new();
        let mut account_data_ok = true;
        for item in items {
            let Some(record) = journal.item(item.id) else {
                continue;
            };
            if !record.progress.validated {
                continue;
            }
            let intact = match (item.class, item.id) {
                (SourceClass::Obsolete, _) => {
                    let dest = self.backup_destination(&journal.backup_directory, &item.source)?;
                    self.destination_intact(item, &dest, record)?
                }
                (_, "account_data.json") => {
                    account_data_ok = self.profile_account_data_valid(paths)?;
                    account_data_ok
                }
                // Consumed into account_data; validity binds to it.
                (_, "characters_cache.json") | (_, "live_vault.json") => true,
                (_, "roaming_characters_cache.json") => {
                    let relative = PathBuf::from("roaming").join(&item.source);
                    let dest = self.backup_destination(&journal.backup_directory, &relative)?;
                    self.destination_intact(item, &dest, record)?
                }
                (_, "char_list.xml") => {
                    let dest = if char_list_contradicts {
                        self.quarantine_destination(item)?
                    } else {
                        self.profile_destination(item, paths)?
                    };
                    self.destination_intact(item, &dest, record)?
                }
                (SourceClass::Attributable | SourceClass::ActiveDatabase, _) => {
                    let dest = self.profile_destination(item, paths)?;
                    self.destination_intact(item, &dest, record)?
                }
                _ => true,
            };
            if !intact {
                to_clear.push(item.id);
            }
        }
        // The integrated cache/vault import binds to a validated profile
        // account_data; if it is missing or corrupt, clear it so the copy pass
        // rebuilds it (re-consuming the caches).
        if !account_data_ok && !to_clear.contains(&"account_data.json") {
            to_clear.push("account_data.json");
        }
        for id in to_clear {
            clear_item(journal, id);
        }
        Ok(())
    }

    /// Revalidate each already-completed unknown (quarantine/backup) destination.
    fn revalidate_unknown_destinations(
        &self,
        journal: &mut MigrationJournal,
        items: &[InventoryItem],
    ) -> Result<(), MigrationError> {
        let mut to_clear: Vec<&str> = Vec::new();
        for item in items {
            let Some(record) = journal.item(item.id) else {
                continue;
            };
            if !record.progress.validated {
                continue;
            }
            let dest = if item.class == SourceClass::Obsolete {
                self.backup_destination(&journal.backup_directory, &item.source)?
            } else {
                self.quarantine_destination(item)?
            };
            if !self.destination_intact(item, &dest, record)? {
                to_clear.push(item.id);
            }
        }
        for id in to_clear {
            clear_item(journal, id);
        }
        Ok(())
    }

    /// Whether a completed destination still matches its recorded digest/schema.
    fn destination_intact(
        &self,
        item: &InventoryItem,
        dest: &Path,
        record: &journal::ItemRecord,
    ) -> Result<bool, MigrationError> {
        match item.validator {
            Validator::Sqlite => {
                let Some(spec) = sqlite::spec_for(item.id) else {
                    return Ok(true);
                };
                Ok(sqlite::validate_database_file(dest, &spec).is_ok())
            }
            Validator::OpaqueTree => {
                if !self.path_exists(dest)? {
                    return Ok(false);
                }
                match &record.progress.digest {
                    Some(expected) => {
                        let actual =
                            validate::tree_manifest_digest(&validate::tree_manifest(dest)?);
                        Ok(&actual == expected)
                    }
                    None => Ok(true),
                }
            }
            Validator::None => Ok(true),
            _ => {
                if !self.path_exists(dest)? {
                    return Ok(false);
                }
                match &record.progress.digest {
                    Some(expected) => Ok(&validate::hash_file_hex(dest)? == expected),
                    None => Ok(true),
                }
            }
        }
    }

    /// Whether the profile `account_data.json` exists and parses as `AccountData`.
    fn profile_account_data_valid(&self, paths: &AccountPaths) -> Result<bool, MigrationError> {
        let dest = paths.account_data()?;
        if !self.path_exists(&dest)? {
            return Ok(false);
        }
        let bytes = std::fs::read(&dest).map_err(|source| MigrationError::CopyIo {
            path: dest.clone(),
            source,
        })?;
        Ok(serde_json::from_slice::<AccountData>(&bytes).is_ok())
    }

    fn build_profile_account_data(
        &self,
        paths: &AccountPaths,
        account_data_contradicts: bool,
        local_cache_contradicts: bool,
        roaming_cache_contradicts: bool,
    ) -> Result<(), MigrationError> {
        let destination = paths.account_data()?;
        // Precedence: canonical account_data, otherwise local then Roaming
        // characters cache; canonical vault, otherwise live_vault.
        let canonical = self.read_optional("account_data.json")?;
        if let Some(bytes) = canonical.filter(|_| !account_data_contradicts) {
            let data: AccountData = serde_json::from_slice(&bytes)
                .map_err(|source| MigrationError::JsonInvalid { source })?;
            write_json_atomic_at(&destination, &data, BackupPolicy::Single)?;
            return Ok(());
        }

        let mut data = AccountData::new();
        let chars = if !local_cache_contradicts {
            self.read_optional("characters_cache.json")?
        } else {
            None
        };
        let chars = match chars {
            Some(bytes) => Some(bytes),
            None if !roaming_cache_contradicts => self.read_roaming_characters_cache()?,
            None => None,
        };
        if let Some(bytes) = chars {
            let text = String::from_utf8_lossy(&bytes);
            let cache = parse_legacy_character_cache(&text)
                .map_err(|source| MigrationError::JsonInvalid { source })?;
            data.characters = cache;
            data.last_api_update = data.characters.last_updated;
        }
        if let Some(bytes) = self.read_optional("live_vault.json")? {
            let text = String::from_utf8_lossy(&bytes);
            let (regular, seasonal) = parse_legacy_vault(&text)
                .map_err(|source| MigrationError::JsonInvalid { source })?;
            data.regular_vault = regular;
            data.seasonal_vault = seasonal;
        }
        write_json_atomic_at(&destination, &data, BackupPolicy::Single)?;
        Ok(())
    }

    fn quarantine_identity_source(
        &self,
        journal: &mut MigrationJournal,
        item: &InventoryItem,
    ) -> Result<(), MigrationError> {
        let source = self.absolute_source(item)?;
        if !self.path_exists(&source)? {
            return Ok(());
        }
        let destination = self.quarantine_destination(item)?;
        if self.path_exists(&destination)? {
            let _ = std::fs::remove_file(&destination);
        }
        let digest = validate::copy_and_validate_file(&source, &destination, Validator::Json)?;
        if let Some(record) = journal.item_mut(item.id) {
            record.progress.digest = Some(hex::encode(digest));
        }
        Ok(())
    }

    fn import_known_token(
        &self,
        journal: &mut MigrationJournal,
    ) -> Result<Option<String>, MigrationError> {
        let key = journal
            .target_account_key
            .ok_or_else(|| MigrationError::Invariant {
                detail: "known token import has no target key".to_string(),
            })?;

        if !matches!(journal.credential_outcome, CredentialOutcome::NotApplicable) {
            // Already attempted; re-deriving the recorded target from the key is
            // sufficient because commit is idempotent and re-imports.
            return Ok(match journal.credential_outcome {
                CredentialOutcome::Imported | CredentialOutcome::Removed => {
                    Some(crate::account::credential_target(key))
                }
                _ => None,
            });
        }

        let token_path = self.local.ensure_no_links("access_token.txt")?;
        let raw = match std::fs::read_to_string(&token_path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                journal.credential_outcome = CredentialOutcome::NotApplicable;
                save_journal(&self.local, journal)?;
                return Ok(None);
            }
            Err(source) => {
                return Err(MigrationError::CopyIo {
                    path: token_path,
                    source,
                })
            }
        };

        // Strict, reusable parse seam: preserve the captured time, accept a
        // legacy bare token, and treat malformed JSON as a retryable error that
        // leaves the file in place. The token contents are never logged.
        let saved = match crate::api::parse_saved_token_strict(&raw) {
            crate::api::TokenParse::Valid(saved) => saved,
            crate::api::TokenParse::Empty => {
                journal.credential_outcome = CredentialOutcome::NotApplicable;
                save_journal(&self.local, journal)?;
                return Ok(None);
            }
            crate::api::TokenParse::Malformed => return Err(MigrationError::TokenMalformed),
        };

        let target = crate::account::credential_target(key);
        let credential = SavedCredential::new(saved.token, saved.captured_at);
        match self.credentials.write(&target, &credential) {
            Ok(()) => {
                journal.credential_outcome = CredentialOutcome::Imported;
                save_journal(&self.local, journal)?;
                Ok(Some(target))
            }
            Err(CredentialError::Unavailable)
            | Err(CredentialError::Backend { .. })
            | Err(CredentialError::MalformedPayload)
            | Err(CredentialError::InvalidTarget) => {
                // Secure-store failure degrades to a missing token; migration
                // continues and the token file is still removed after commit.
                journal.credential_outcome = CredentialOutcome::ImportFailed;
                save_journal(&self.local, journal)?;
                Ok(None)
            }
        }
    }

    fn remove_plaintext_token(&self, journal: &mut MigrationJournal) -> Result<(), MigrationError> {
        let token_path = self.local.ensure_no_links("access_token.txt")?;
        // Only an already-absent token is ignored. Any other failure (for
        // example a permission or metadata error) is retryable and must not
        // advance the phase, so the plaintext is never silently left behind.
        match std::fs::remove_file(&token_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(MigrationError::CopyIo {
                    path: token_path,
                    source,
                })
            }
        }
        if matches!(journal.credential_outcome, CredentialOutcome::Imported) {
            journal.credential_outcome = CredentialOutcome::Removed;
            save_journal(&self.local, journal)?;
        }
        Ok(())
    }

    fn back_up_originals(
        &self,
        journal: &mut MigrationJournal,
        items: &[InventoryItem],
    ) -> Result<(), MigrationError> {
        let backup_dir = journal.backup_directory.clone();
        for item in items {
            if item.class == SourceClass::Global || item.class == SourceClass::Credential {
                continue;
            }
            // Fail closed: a missing journal record for an inventory item is
            // corruption, never a silent "already backed up".
            if journal.item_backed_up(item.id)? {
                continue;
            }

            if item.class == SourceClass::Obsolete {
                // Backup-only sources were copied+validated into the backup
                // during CopiesValidated. Revalidate that copy, then delete the
                // original only after the backup is proven intact.
                self.delete_backup_only_original(journal, item, &backup_dir)?;
                mark_backed_up(journal, item.id);
                save_journal(&self.local, journal)?;
                self.check_failpoint(&format!("backup:{}", item.id))?;
                continue;
            }

            if item.location == SourceLocation::Roaming {
                // The external Roaming original is never moved; the imported copy
                // already lives in a distinct backup path.
                mark_backed_up(journal, item.id);
                save_journal(&self.local, journal)?;
                self.check_failpoint(&format!("backup:{}", item.id))?;
                continue;
            }
            self.move_original_to_backup(&item.source, &backup_dir)?;
            mark_backed_up(journal, item.id);
            save_journal(&self.local, journal)?;
            self.check_failpoint(&format!("backup:{}", item.id))?;
        }
        Ok(())
    }

    /// Delete a backup-only original after revalidating its already-produced
    /// backup copy against the journaled digest/tree.
    fn delete_backup_only_original(
        &self,
        journal: &MigrationJournal,
        item: &InventoryItem,
        backup_dir: &str,
    ) -> Result<(), MigrationError> {
        let source = self.local.ensure_no_links(&item.source)?;
        if !self.path_exists(&source)? {
            return Ok(());
        }
        let backup = self.backup_destination(backup_dir, &item.source)?;
        // Revalidate the backup copy before removing anything.
        let recorded = journal
            .item(item.id)
            .and_then(|r| r.progress.digest.clone());
        if let Some(expected) = recorded {
            if !self.path_exists(&backup)? {
                return Err(MigrationError::CopyMismatch { path: backup });
            }
            let actual = if item.validator == Validator::OpaqueTree {
                validate::tree_manifest_digest(&validate::tree_manifest(&backup)?)
            } else {
                validate::hash_file_hex(&backup)?
            };
            if actual != expected {
                return Err(MigrationError::CopyMismatch { path: backup });
            }
        } else if !self.path_exists(&backup)? {
            // A source existed but no validated backup copy was produced.
            return Err(MigrationError::CopyMismatch { path: backup });
        }
        // The backup is proven durable; remove the original (refusing links).
        validate::remove_tree_no_links(&source)
    }

    fn move_original_to_backup(
        &self,
        source_relative: &Path,
        backup_dir: &str,
    ) -> Result<(), MigrationError> {
        let source = self.local.ensure_no_links(source_relative)?;
        if !self.path_exists(&source)? {
            return Ok(());
        }
        let destination_relative = PathBuf::from(backup_dir).join(source_relative);
        let destination = self.local.ensure_no_links(&destination_relative)?;
        move_path(&source, &destination)?;

        // SQLite sidecars travel with their database (migration-owned move
        // because generic storage moves reject SQLite files).
        if source.extension().map(|ext| ext == "db").unwrap_or(false) {
            for suffix in ["-wal", "-shm", "-journal"] {
                let sidecar = append_suffix(&source, suffix);
                if self.path_exists(&sidecar)? {
                    let sidecar_dest = append_suffix(&destination, suffix);
                    move_path(&sidecar, &sidecar_dest)?;
                }
            }
        }
        Ok(())
    }

    fn source_contradicts(
        &self,
        item_id: &str,
        settings_account_id: &AccountId,
    ) -> Result<bool, MigrationError> {
        let bytes = match item_id {
            "roaming_characters_cache.json" => self.read_roaming_characters_cache()?,
            other => self.read_optional(other)?,
        };
        let Some(bytes) = bytes else {
            return Ok(false);
        };
        Ok(embedded_account_id_json(&bytes)
            .map(|embedded| embedded != settings_account_id.as_str())
            .unwrap_or(false))
    }

    fn char_list_contradicts(
        &self,
        settings_account_id: &AccountId,
    ) -> Result<bool, MigrationError> {
        let Some(bytes) = self.read_optional("char_list.xml")? else {
            return Ok(false);
        };
        Ok(embedded_account_id_xml(&bytes)
            .map(|embedded| embedded != settings_account_id.as_str())
            .unwrap_or(false))
    }

    fn read_optional(&self, relative: &str) -> Result<Option<Vec<u8>>, MigrationError> {
        let path = self.local.ensure_no_links(relative)?;
        read_optional_bytes(&path)
    }

    fn read_roaming_characters_cache(&self) -> Result<Option<Vec<u8>>, MigrationError> {
        let Some(roaming) = &self.roaming else {
            return Ok(None);
        };
        let path = roaming.ensure_no_links("characters_cache.json")?;
        read_optional_bytes(&path)
    }

    fn absolute_source(&self, item: &InventoryItem) -> Result<PathBuf, MigrationError> {
        match item.location {
            SourceLocation::Local => Ok(self.local.ensure_no_links(&item.source)?),
            SourceLocation::Roaming => {
                let roaming = self
                    .roaming
                    .as_ref()
                    .ok_or_else(|| MigrationError::Invariant {
                        detail: "roaming item requires a roaming root".to_string(),
                    })?;
                Ok(roaming.ensure_no_links(&item.source)?)
            }
        }
    }

    /// Existence probe that distinguishes true absence from a permission or
    /// metadata error. Never assumes absence on an unexpected error.
    fn path_exists(&self, path: &Path) -> Result<bool, MigrationError> {
        match std::fs::symlink_metadata(path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(MigrationError::PathProbe {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    fn profile_destination(
        &self,
        item: &InventoryItem,
        paths: &AccountPaths,
    ) -> Result<PathBuf, MigrationError> {
        let path = match item.id {
            "quests.json" => paths.quests()?,
            "char_list.xml" => paths.character_list_cache()?,
            "imports/dungeon.stats" => paths.realmshark_import()?,
            "logs/chat" => paths.chat_logs()?,
            "event_notification_log.csv" => paths.event_notifications()?,
            "watchlist_detections.log" => paths.watchlist_detections()?,
            "loot_history.db" => paths.loot_database()?,
            "combat_history.db" => paths.combat_database()?,
            "account_data.json" => paths.account_data()?,
            _ => {
                return Err(MigrationError::JournalMismatch {
                    detail: format!("no profile destination for `{}`", item.id),
                })
            }
        };
        Ok(path)
    }

    fn quarantine_destination(&self, item: &InventoryItem) -> Result<PathBuf, MigrationError> {
        let leaf = match item.id {
            "roaming_characters_cache.json" => PathBuf::from("roaming_characters_cache.json"),
            _ => item.source.clone(),
        };
        let relative = PathBuf::from(QUARANTINE_DIR).join(leaf);
        Ok(self.local.ensure_no_links(&relative)?)
    }
}

fn timestamped_backup_dir(now: DateTime<Utc>) -> String {
    format!("migration-backups/{}", now.format("%Y-%m-%dT%H%M%SZ"))
}

fn phase_before(current: MigrationPhase, target: MigrationPhase) -> bool {
    phase_rank(current) < phase_rank(target)
}

fn phase_rank(phase: MigrationPhase) -> u8 {
    match phase {
        MigrationPhase::Prepared => 0,
        MigrationPhase::CopiesValidated => 1,
        MigrationPhase::RegistryCommitted => 2,
        MigrationPhase::OriginalsBackedUp => 3,
        MigrationPhase::Complete | MigrationPhase::AwaitingAttribution => 4,
    }
}

fn item_done(journal: &MigrationJournal, id: &str) -> bool {
    journal
        .item(id)
        .map(|record| record.progress.validated)
        .unwrap_or(false)
}

fn clear_item(journal: &mut MigrationJournal, id: &str) {
    if let Some(record) = journal.item_mut(id) {
        record.progress.copied = false;
        record.progress.validated = false;
        record.progress.digest = None;
    }
}

fn mark_validated(journal: &mut MigrationJournal, id: &str, digest: Option<String>) {
    if let Some(record) = journal.item_mut(id) {
        record.progress.copied = true;
        record.progress.validated = true;
        if digest.is_some() {
            record.progress.digest = digest;
        }
    }
}

fn mark_backed_up(journal: &mut MigrationJournal, id: &str) {
    if let Some(record) = journal.item_mut(id) {
        record.progress.backed_up = true;
    }
}

fn tree_digest(manifest: &validate::TreeManifest) -> String {
    validate::tree_manifest_digest(manifest)
}

fn read_optional_bytes(path: &Path) -> Result<Option<Vec<u8>>, MigrationError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(MigrationError::CopyIo {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn move_path(source: &Path, destination: &Path) -> Result<(), MigrationError> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|source_err| MigrationError::CopyIo {
            path: parent.to_path_buf(),
            source: source_err,
        })?;
    }
    std::fs::rename(source, destination).map_err(|source_err| MigrationError::CopyIo {
        path: destination.to_path_buf(),
        source: source_err,
    })
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn embedded_account_id_json(bytes: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    for key in ["account_id", "accountId", "AccountId"] {
        if let Some(found) = find_string_field(&value, key) {
            return Some(found);
        }
    }
    None
}

fn find_string_field(value: &serde_json::Value, key: &str) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(found)) = map.get(key) {
                let trimmed = found.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
            map.values().find_map(|child| find_string_field(child, key))
        }
        serde_json::Value::Array(items) => {
            items.iter().find_map(|child| find_string_field(child, key))
        }
        _ => None,
    }
}

fn embedded_account_id_xml(bytes: &[u8]) -> Option<String> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let text = std::str::from_utf8(bytes).ok()?;
    let mut reader = Reader::from_str(text);
    let mut in_account_id = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => {
                if start.name().as_ref() == b"AccountId" {
                    in_account_id = true;
                }
            }
            Ok(Event::Text(text)) if in_account_id => {
                let decoded = text.unescape().ok()?;
                let trimmed = decoded.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
                in_account_id = false;
            }
            Ok(Event::End(_)) => in_account_id = false,
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    None
}

#[cfg(test)]
mod tests;

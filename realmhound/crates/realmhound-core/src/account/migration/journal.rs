//! Atomic, versioned migration journal at `migrations/flat-layout-v1.json`.
//!
//! The journal makes flat-layout migration resumable. It records the five known
//! migration states (plus the unknown-account `AwaitingAttribution` terminal
//! state), per-item copy/validation/backup progress, the fixed timestamped
//! backup directory, a redacted credential outcome, backup retention metadata,
//! and a recovery status. It is written with the shared atomic-document writer
//! and fails closed on an unsupported version or a changed/conflicting binding.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::MigrationError;
use crate::account::{AccountId, AccountKey};
use crate::storage::{
    load_json, write_json_atomic, BackupPolicy, LoadOutcome, StorageError, StorageRoot,
};

/// Supported journal document version.
pub const JOURNAL_VERSION: u32 = 1;
/// Journal migration label.
pub const JOURNAL_MIGRATION: &str = "flat-layout-v1";
/// Root-relative journal path.
pub const JOURNAL_PATH: &str = "migrations/flat-layout-v1.json";

/// Whether the migration targets a known account or is unknown/discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationKind {
    /// Settings held a saved account ID.
    Known,
    /// No reliable account ID; data is quarantined for later attribution.
    Unknown,
}

/// The persisted migration phase. Known migration advances through the five
/// states; unknown migration ends at [`MigrationPhase::AwaitingAttribution`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MigrationPhase {
    /// Journal and profile/quarantine prepared; nothing validated yet.
    Prepared,
    /// Every owned copy has been produced and validated.
    CopiesValidated,
    /// Registry selection/discovery has been atomically committed.
    RegistryCommitted,
    /// Non-credential originals have been relocated into the backup.
    OriginalsBackedUp,
    /// Known migration finished.
    Complete,
    /// Unknown migration finished its initial pass and awaits attribution.
    AwaitingAttribution,
}

/// Per-item copy, validation, and backup progress.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemProgress {
    /// The owned copy has been produced.
    #[serde(default)]
    pub copied: bool,
    /// The owned copy has been validated durable.
    #[serde(default)]
    pub validated: bool,
    /// The original has been relocated into the timestamped backup.
    #[serde(default)]
    pub backed_up: bool,
    /// Hex content digest or tree-manifest digest, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

/// A journaled inventory item and its progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemRecord {
    /// Inventory item id.
    pub id: String,
    /// Progress flags for this item.
    #[serde(default)]
    pub progress: ItemProgress,
}

/// Redacted outcome of the plaintext-token import. Never carries the token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialOutcome {
    /// No token was present, or unknown migration (never imports a token).
    NotApplicable,
    /// A token was imported into secure storage.
    Imported,
    /// Secure store was unavailable; treated as a missing token.
    ImportFailed,
    /// The plaintext token file was removed after profile+registry commit.
    Removed,
}

/// Backup retention metadata tracked for the confirmed-deletion policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupMetadata {
    /// When the timestamped backup directory was created.
    pub created_at: DateTime<Utc>,
    /// Validated launches of the migrated profile.
    #[serde(default)]
    pub validated_launches: u32,
}

/// High-level recovery status surfaced to callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryStatus {
    /// Migration is progressing or complete; no manual recovery needed.
    Healthy,
    /// Migration was interrupted; originals remain authoritative.
    ResumableBeforeCommit,
    /// Migration committed the registry; resume backup/token cleanup.
    ResumableAfterCommit,
}

/// The full migration journal document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationJournal {
    /// Document version; unsupported versions fail closed on load.
    pub version: u32,
    /// Migration label.
    pub migration: String,
    /// Known vs unknown migration.
    pub kind: MigrationKind,
    /// Current phase.
    pub phase: MigrationPhase,
    /// Fingerprint binding the journal to a stable inventory shape.
    pub inventory_fingerprint: String,
    /// Target account key, present for known migration only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_account_key: Option<AccountKey>,
    /// Non-reversible hash of the initial normalized `AccountId`, present for
    /// known migration only. The raw account ID is never stored; every precommit
    /// resume re-derives this hash from settings and fails closed on a change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_identity_hash: Option<String>,
    /// Fixed, timestamped backup directory (root-relative).
    pub backup_directory: String,
    /// Per-item progress.
    #[serde(default)]
    pub items: Vec<ItemRecord>,
    /// Redacted credential outcome.
    pub credential_outcome: CredentialOutcome,
    /// Backup retention metadata.
    pub backup: BackupMetadata,
    /// Recovery status.
    pub recovery: RecoveryStatus,
}

impl MigrationJournal {
    /// Create a fresh Prepared journal.
    #[allow(clippy::too_many_arguments)]
    pub fn prepared(
        kind: MigrationKind,
        inventory_fingerprint: String,
        target_account_key: Option<AccountKey>,
        known_identity_hash: Option<String>,
        backup_directory: String,
        item_ids: &[&str],
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            version: JOURNAL_VERSION,
            migration: JOURNAL_MIGRATION.to_string(),
            kind,
            phase: MigrationPhase::Prepared,
            inventory_fingerprint,
            target_account_key,
            known_identity_hash,
            backup_directory,
            items: item_ids
                .iter()
                .map(|id| ItemRecord {
                    id: (*id).to_string(),
                    progress: ItemProgress::default(),
                })
                .collect(),
            credential_outcome: CredentialOutcome::NotApplicable,
            backup: BackupMetadata {
                created_at,
                validated_launches: 0,
            },
            recovery: RecoveryStatus::ResumableBeforeCommit,
        }
    }

    /// Find a mutable item record by id.
    pub fn item_mut(&mut self, id: &str) -> Option<&mut ItemRecord> {
        self.items.iter_mut().find(|record| record.id == id)
    }

    /// Find an item record by id.
    pub fn item(&self, id: &str) -> Option<&ItemRecord> {
        self.items.iter().find(|record| record.id == id)
    }

    /// Whether an item's original has been relocated into the backup.
    ///
    /// Fails closed: a missing record for a queried inventory id is journal
    /// corruption, never a silent "already backed up".
    pub fn item_backed_up(&self, id: &str) -> Result<bool, MigrationError> {
        match self.item(id) {
            Some(record) => Ok(record.progress.backed_up),
            None => Err(MigrationError::JournalCorrupt {
                detail: format!("journal is missing a record for inventory item `{id}`"),
            }),
        }
    }

    /// Verify the journal's item ids are exactly the current inventory ids with
    /// no missing, duplicate, or extra entries.
    pub fn verify_item_ids_exhaustive(&self, item_ids: &[&str]) -> Result<(), MigrationError> {
        use std::collections::BTreeSet;
        let expected: BTreeSet<&str> = item_ids.iter().copied().collect();
        if expected.len() != item_ids.len() {
            return Err(MigrationError::JournalMismatch {
                detail: "inventory contains duplicate item ids".to_string(),
            });
        }
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for record in &self.items {
            if !seen.insert(record.id.as_str()) {
                return Err(MigrationError::JournalCorrupt {
                    detail: format!("journal contains a duplicate item id `{}`", record.id),
                });
            }
            if !expected.contains(record.id.as_str()) {
                return Err(MigrationError::JournalMismatch {
                    detail: format!("journal has an unexpected item id `{}`", record.id),
                });
            }
        }
        for id in &expected {
            if !seen.contains(id) {
                return Err(MigrationError::JournalMismatch {
                    detail: format!("journal is missing item id `{id}`"),
                });
            }
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), MigrationError> {
        if self.version != JOURNAL_VERSION {
            return Err(MigrationError::JournalVersionUnsupported {
                version: self.version,
            });
        }
        if self.migration != JOURNAL_MIGRATION {
            return Err(MigrationError::JournalMismatch {
                detail: format!("unexpected migration label `{}`", self.migration),
            });
        }
        match (self.kind, self.target_account_key) {
            (MigrationKind::Known, None) => {
                return Err(MigrationError::JournalMismatch {
                    detail: "known migration journal is missing its target key".to_string(),
                })
            }
            (MigrationKind::Unknown, Some(_)) => {
                return Err(MigrationError::JournalMismatch {
                    detail: "unknown migration journal must not bind a target key".to_string(),
                })
            }
            _ => {}
        }
        match (self.kind, self.known_identity_hash.as_ref()) {
            (MigrationKind::Known, None) => Err(MigrationError::JournalMismatch {
                detail: "known migration journal is missing its identity binding".to_string(),
            }),
            (MigrationKind::Unknown, Some(_)) => Err(MigrationError::JournalMismatch {
                detail: "unknown migration journal must not bind an identity".to_string(),
            }),
            _ => Ok(()),
        }
    }
}

/// Load and validate the migration journal, if present.
pub fn load_journal(root: &StorageRoot) -> Result<Option<MigrationJournal>, MigrationError> {
    let outcome = load_json::<MigrationJournal>(root, JOURNAL_PATH).map_err(map_storage)?;
    match outcome {
        LoadOutcome::Primary(journal) | LoadOutcome::RecoveredBackup(journal) => {
            journal.validate()?;
            Ok(Some(journal))
        }
        LoadOutcome::Missing => Ok(None),
    }
}

/// Atomically persist the migration journal, keeping one rolling backup.
pub fn save_journal(root: &StorageRoot, journal: &MigrationJournal) -> Result<(), MigrationError> {
    journal.validate()?;
    write_json_atomic(root, JOURNAL_PATH, journal, BackupPolicy::Single).map_err(map_storage)
}

fn map_storage(error: StorageError) -> MigrationError {
    match error {
        StorageError::InvalidDocument { .. } | StorageError::InvalidDocuments { .. } => {
            MigrationError::JournalCorrupt {
                detail: error.to_string(),
            }
        }
        other => MigrationError::Storage(other),
    }
}

/// Non-reversible binding hash of a normalized account identity.
///
/// Domain-separated SHA-256 of the normalized `AccountId`. The raw account ID is
/// never persisted in the journal; only this hash is stored and re-derived to
/// detect a settings identity change on precommit resume.
pub fn account_identity_hash(account_id: &AccountId) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"realmhound/flat-layout-v1/account-id\0");
    hasher.update(account_id.as_str().as_bytes());
    hex::encode(hasher.finalize())
}

/// Fingerprint binding the journal to a stable inventory shape and target.
pub fn inventory_fingerprint(kind: MigrationKind, item_ids: &[&str]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(match kind {
        MigrationKind::Known => b"known".as_slice(),
        MigrationKind::Unknown => b"unknown".as_slice(),
    });
    let mut ids: Vec<&str> = item_ids.to_vec();
    ids.sort_unstable();
    for id in ids {
        hasher.update([0u8]);
        hasher.update(id.as_bytes());
    }
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_root() -> (tempfile::TempDir, StorageRoot) {
        let temp = tempfile::tempdir().unwrap();
        let root = StorageRoot::from_path(temp.path()).unwrap();
        (temp, root)
    }

    #[test]
    fn round_trips_through_atomic_document() {
        let (_temp, root) = make_root();
        let key = AccountKey::generate();
        let journal = MigrationJournal::prepared(
            MigrationKind::Known,
            inventory_fingerprint(MigrationKind::Known, &["account_data.json"]),
            Some(key),
            Some(account_identity_hash(&AccountId::new("ACCT-1").unwrap())),
            "migration-backups/2026-08-22T200000Z".to_string(),
            &["account_data.json"],
            Utc::now(),
        );
        save_journal(&root, &journal).unwrap();

        let loaded = load_journal(&root).unwrap().unwrap();
        assert_eq!(loaded, journal);
    }

    #[test]
    fn missing_journal_is_none() {
        let (_temp, root) = make_root();
        assert!(load_journal(&root).unwrap().is_none());
    }

    #[test]
    fn unsupported_version_fails_closed() {
        let (_temp, root) = make_root();
        let mut journal = MigrationJournal::prepared(
            MigrationKind::Unknown,
            inventory_fingerprint(MigrationKind::Unknown, &[]),
            None,
            None,
            "migration-backups/x".to_string(),
            &[],
            Utc::now(),
        );
        // Persist a valid journal, then corrupt only the version on disk.
        save_journal(&root, &journal).unwrap();
        journal.version = 99;
        write_json_atomic(
            &root,
            JOURNAL_PATH,
            &serde_json::json!({
                "version": 99,
                "migration": JOURNAL_MIGRATION,
                "kind": "unknown",
                "phase": "prepared",
                "inventory_fingerprint": "x",
                "backup_directory": "b",
                "credential_outcome": "not_applicable",
                "backup": {"created_at": Utc::now(), "validated_launches": 0},
                "recovery": "resumable_before_commit"
            }),
            BackupPolicy::None,
        )
        .unwrap();

        let error = load_journal(&root).unwrap_err();
        assert!(matches!(
            error,
            MigrationError::JournalVersionUnsupported { version: 99 }
        ));
    }

    #[test]
    fn known_journal_requires_target_key() {
        let (_temp, root) = make_root();
        let journal = serde_json::json!({
            "version": 1,
            "migration": JOURNAL_MIGRATION,
            "kind": "known",
            "phase": "prepared",
            "inventory_fingerprint": "x",
            "backup_directory": "b",
            "credential_outcome": "not_applicable",
            "backup": {"created_at": Utc::now(), "validated_launches": 0},
            "recovery": "resumable_before_commit"
        });
        write_json_atomic(&root, JOURNAL_PATH, &journal, BackupPolicy::None).unwrap();

        let error = load_journal(&root).unwrap_err();
        assert!(matches!(error, MigrationError::JournalMismatch { .. }));
    }

    #[test]
    fn fingerprint_is_order_independent() {
        let a = inventory_fingerprint(MigrationKind::Known, &["a", "b"]);
        let b = inventory_fingerprint(MigrationKind::Known, &["b", "a"]);
        assert_eq!(a, b);
        let c = inventory_fingerprint(MigrationKind::Unknown, &["a", "b"]);
        assert_ne!(a, c);
    }

    fn known_journal(ids: &[&str]) -> MigrationJournal {
        MigrationJournal::prepared(
            MigrationKind::Known,
            inventory_fingerprint(MigrationKind::Known, ids),
            Some(AccountKey::generate()),
            Some(account_identity_hash(&AccountId::new("ACCT-1").unwrap())),
            "migration-backups/x".to_string(),
            ids,
            Utc::now(),
        )
    }

    #[test]
    fn known_journal_requires_identity_binding() {
        let (_temp, root) = make_root();
        let journal = serde_json::json!({
            "version": 1,
            "migration": JOURNAL_MIGRATION,
            "kind": "known",
            "phase": "prepared",
            "inventory_fingerprint": "x",
            "target_account_key": AccountKey::generate().to_string(),
            "backup_directory": "b",
            "credential_outcome": "not_applicable",
            "backup": {"created_at": Utc::now(), "validated_launches": 0},
            "recovery": "resumable_before_commit"
        });
        write_json_atomic(&root, JOURNAL_PATH, &journal, BackupPolicy::None).unwrap();
        let error = load_journal(&root).unwrap_err();
        assert!(matches!(error, MigrationError::JournalMismatch { .. }));
    }

    #[test]
    fn exhaustive_item_ids_detect_missing_extra_and_duplicate() {
        let journal = known_journal(&["a", "b", "c"]);
        assert!(journal.verify_item_ids_exhaustive(&["a", "b", "c"]).is_ok());
        // Missing one.
        assert!(journal
            .verify_item_ids_exhaustive(&["a", "b", "c", "d"])
            .is_err());
        // Extra one in the journal.
        assert!(journal.verify_item_ids_exhaustive(&["a", "b"]).is_err());

        // A journal carrying a duplicate id fails closed.
        let mut dup = known_journal(&["a", "b"]);
        dup.items.push(ItemRecord {
            id: "a".to_string(),
            progress: ItemProgress::default(),
        });
        assert!(dup.verify_item_ids_exhaustive(&["a", "b"]).is_err());
    }

    #[test]
    fn item_backed_up_fails_closed_on_missing_record() {
        let journal = known_journal(&["a"]);
        assert_eq!(journal.item_backed_up("a").unwrap(), false);
        assert!(matches!(
            journal.item_backed_up("missing").unwrap_err(),
            MigrationError::JournalCorrupt { .. }
        ));
    }
}

//! Migration-backup retention and confirmed deletion.
//!
//! This governs only the timestamped flat-layout migration backup, never a
//! profile, archive, or account. A backup is eligible for deletion only after it
//! is at least 30 days old and the migrated profile has completed at least three
//! validated launches. Deletion is never automatic: it requires explicit
//! confirmation and enforces exact containment with no links.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};

use super::inventory::{flat_layout_inventory, InventoryItem, SourceLocation, Validator};
use super::journal::{
    load_journal, save_journal, BackupMetadata, MigrationJournal, MigrationKind, MigrationPhase,
};
use super::{sqlite, validate, MigrationError};
use crate::account::AccountKey;
use crate::storage::StorageRoot;

/// Minimum backup age before deletion is offered.
pub const RETENTION_MIN_AGE_DAYS: i64 = 30;
/// Minimum validated launches before deletion is offered.
pub const RETENTION_MIN_LAUNCHES: u32 = 3;
/// Root-relative parent directory that contains migration backups.
pub const BACKUP_ROOT: &str = "migration-backups";

/// Explicit confirmation required to delete a migration backup.
///
/// The value cannot be constructed implicitly; callers must pass `confirmed:
/// true` and the exact backup directory, so no code path deletes a backup by
/// accident.
#[derive(Debug, Clone)]
pub struct DeletionConfirmation {
    /// Must be `true`.
    pub confirmed: bool,
    /// The exact root-relative backup directory the user confirmed.
    pub backup_directory: String,
}

/// Whether a backup is eligible for deletion given its metadata and the time.
pub fn is_eligible_for_deletion(backup: &BackupMetadata, now: DateTime<Utc>) -> bool {
    let age_ok = now - backup.created_at >= Duration::days(RETENTION_MIN_AGE_DAYS);
    let launches_ok = backup.validated_launches >= RETENTION_MIN_LAUNCHES;
    age_ok && launches_ok
}

/// Record one validated launch of a selected migrated profile in the journal.
///
/// Selected-profile startup calls this once, only after the complete profile has
/// opened successfully, so failed startups never increment retention metadata.
/// It records a launch only when the journal describes a completed known
/// migration whose target key matches the launched profile; a missing,
/// unrelated, unknown, or still-incomplete journal is a no-op `Ok(())` so
/// startups of non-migrated or other profiles never touch it. The increment is
/// bounded and saturating, and persisted atomically so a crash cannot
/// double-count.
pub fn record_validated_launch(
    root: &StorageRoot,
    account_key: AccountKey,
) -> Result<(), MigrationError> {
    let mut journal = match load_journal(root)? {
        Some(journal) => journal,
        None => return Ok(()),
    };
    let matches_completed_known = journal.kind == MigrationKind::Known
        && journal.phase == MigrationPhase::Complete
        && journal.target_account_key == Some(account_key);
    if !matches_completed_known {
        return Ok(());
    }
    bump_validated_launches(&mut journal);
    save_journal(root, &journal)
}

/// Bounded, saturating increment of the journal's validated-launch counter.
fn bump_validated_launches(journal: &mut MigrationJournal) {
    journal.backup.validated_launches = journal.backup.validated_launches.saturating_add(1);
}
/// Validate a migration backup directory before any restoration.
///
/// Confirms the path is contained under [`BACKUP_ROOT`], contains no links or
/// reparse points, and exists as a directory. This is the explicit gate a
/// recovery flow calls before restoring; migration never rolls back
/// automatically.
pub fn validate_backup_for_restore(
    root: &StorageRoot,
    backup_directory: &str,
) -> Result<PathBuf, MigrationError> {
    let relative = PathBuf::from(backup_directory);
    let mut components = relative.iter();
    let first = components
        .next()
        .and_then(|c| c.to_str())
        .ok_or(MigrationError::DeletionNotContained)?;
    if first != BACKUP_ROOT || components.next().is_none() {
        return Err(MigrationError::DeletionNotContained);
    }
    let absolute = root
        .ensure_no_links(&relative)
        .map_err(MigrationError::Storage)?;
    let metadata = fs::symlink_metadata(&absolute).map_err(|source| MigrationError::CopyIo {
        path: absolute.clone(),
        source,
    })?;
    if is_link_or_reparse(&metadata) {
        return Err(MigrationError::LinkNotAllowed { path: absolute });
    }
    if !metadata.is_dir() {
        return Err(MigrationError::DeletionNotContained);
    }
    Ok(absolute)
}

/// Explicitly restore files from a validated migration backup into the storage
/// root, never overwriting an existing destination.
///
/// This is a non-destructive recovery seam bound to the migration [`journal`]. It
/// validates the backup directory containment and then validates each backed-up
/// item's content against the journaled digests and schemas (not merely that the
/// directory exists) before filling in files that are currently absent, so it
/// can never destroy live data nor restore corrupt backup content. Returns the
/// root-relative paths restored.
pub fn restore_backup(
    root: &StorageRoot,
    journal: &MigrationJournal,
) -> Result<Vec<PathBuf>, MigrationError> {
    let backup_root = validate_backup_for_restore(root, &journal.backup_directory)?;
    validate_backup_contents(&backup_root, journal)?;
    let mut restored = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(relative) = stack.pop() {
        let absolute = backup_root.join(&relative);
        let metadata =
            fs::symlink_metadata(&absolute).map_err(|source| MigrationError::CopyIo {
                path: absolute.clone(),
                source,
            })?;
        if is_link_or_reparse(&metadata) {
            return Err(MigrationError::LinkNotAllowed { path: absolute });
        }
        if metadata.is_dir() {
            for entry in fs::read_dir(&absolute).map_err(|source| MigrationError::CopyIo {
                path: absolute.clone(),
                source,
            })? {
                let entry = entry.map_err(|source| MigrationError::CopyIo {
                    path: absolute.clone(),
                    source,
                })?;
                stack.push(relative.join(entry.file_name()));
            }
        } else if metadata.is_file() {
            // The `roaming/` subtree preserves the external Roaming original and
            // is never restored into the local root.
            if relative.starts_with("roaming") {
                continue;
            }
            let destination = root
                .ensure_no_links(&relative)
                .map_err(MigrationError::Storage)?;
            if destination.exists() {
                continue;
            }
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|source| MigrationError::CopyIo {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            fs::copy(&absolute, &destination).map_err(|source| MigrationError::CopyIo {
                path: destination.clone(),
                source,
            })?;
            restored.push(relative.clone());
        }
    }
    Ok(restored)
}

/// Backup path of a backed-up inventory item below the timestamped backup root.
///
/// Local originals are relocated to `<backup>/<source>`; the Roaming cache copy
/// is preserved under a distinct `<backup>/roaming/<source>` subtree.
fn item_backup_path(backup_root: &Path, item: &InventoryItem) -> PathBuf {
    match item.location {
        SourceLocation::Local => backup_root.join(&item.source),
        SourceLocation::Roaming => backup_root.join("roaming").join(&item.source),
    }
}

/// Validate the content of every backed-up item present in the backup against
/// the journaled digests and schemas. Missing items are permissible (their live
/// original may never have existed); present-but-corrupt content fails closed.
fn validate_backup_contents(
    backup_root: &Path,
    journal: &MigrationJournal,
) -> Result<(), MigrationError> {
    let has_roaming = journal.item("roaming_characters_cache.json").is_some();
    for item in flat_layout_inventory(has_roaming) {
        let Some(record) = journal.item(item.id) else {
            continue;
        };
        if !record.progress.backed_up {
            continue;
        }
        let path = item_backup_path(backup_root, &item);
        match fs::symlink_metadata(&path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => return Err(MigrationError::CopyIo { path, source }),
        }

        if let Some(expected) = &record.progress.digest {
            if item.validator == Validator::OpaqueTree {
                let manifest = validate::tree_manifest(&path)?;
                if &validate::tree_manifest_digest(&manifest) != expected {
                    return Err(MigrationError::CopyMismatch { path });
                }
            } else {
                if validate::hash_file_hex(&path)? != *expected {
                    return Err(MigrationError::CopyMismatch { path });
                }
            }
            continue;
        }

        // No recorded digest: validate structurally by validator kind.
        match item.validator {
            Validator::Sqlite => {
                if let Some(spec) = sqlite::spec_for(item.id) {
                    sqlite::validate_database_file(&path, &spec)?;
                }
            }
            Validator::Json | Validator::Xml | Validator::RealmShark => {
                let bytes = fs::read(&path).map_err(|source| MigrationError::CopyIo {
                    path: path.clone(),
                    source,
                })?;
                validate::validate_document(item.validator, &bytes)?;
            }
            Validator::OpaqueFile | Validator::OpaqueTree | Validator::None => {}
        }
    }
    Ok(())
}

/// Delete a migration backup after explicit confirmation and safety checks.
///
/// Bound to the migration [`journal`]: the authoritative backup directory and
/// retention metadata are taken from the journal, so metadata from one backup
/// can never delete another. Fails closed unless: confirmation is present and
/// matches the journal's exact backup directory, the journal metadata is
/// eligible, the path is contained under [`BACKUP_ROOT`], and no path component
/// is a link or reparse point.
pub fn delete_backup(
    root: &StorageRoot,
    journal: &MigrationJournal,
    confirmation: &DeletionConfirmation,
    now: DateTime<Utc>,
) -> Result<(), MigrationError> {
    if !confirmation.confirmed {
        return Err(MigrationError::DeletionNotConfirmed);
    }
    // The confirmation must name the journal's exact backup directory; the
    // journaled path is authoritative and never taken from the confirmation.
    if confirmation.backup_directory != journal.backup_directory {
        return Err(MigrationError::DeletionNotContained);
    }
    let relative = PathBuf::from(&journal.backup_directory);
    let mut components = relative.iter();
    let first = components
        .next()
        .and_then(|c| c.to_str())
        .ok_or(MigrationError::DeletionNotContained)?;
    if first != BACKUP_ROOT || components.next().is_none() {
        return Err(MigrationError::DeletionNotContained);
    }
    if !is_eligible_for_deletion(&journal.backup, now) {
        return Err(MigrationError::DeletionNotEligible);
    }

    let absolute = root
        .ensure_no_links(&relative)
        .map_err(MigrationError::Storage)?;
    remove_tree_no_links(&absolute)?;
    Ok(())
}

fn remove_tree_no_links(path: &Path) -> Result<(), MigrationError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(MigrationError::CopyIo {
                path: path.to_path_buf(),
                source: error,
            })
        }
    };
    if is_link_or_reparse(&metadata) {
        return Err(MigrationError::LinkNotAllowed {
            path: path.to_path_buf(),
        });
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(|source| MigrationError::CopyIo {
            path: path.to_path_buf(),
            source,
        })? {
            let entry = entry.map_err(|source| MigrationError::CopyIo {
                path: path.to_path_buf(),
                source,
            })?;
            remove_tree_no_links(&entry.path())?;
        }
        fs::remove_dir(path).map_err(|source| MigrationError::CopyIo {
            path: path.to_path_buf(),
            source,
        })?;
    } else {
        fs::remove_file(path).map_err(|source| MigrationError::CopyIo {
            path: path.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

#[cfg(windows)]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::migration::journal::{
        inventory_fingerprint, ItemProgress, MigrationJournal, MigrationKind,
    };

    fn root() -> (tempfile::TempDir, StorageRoot) {
        let temp = tempfile::tempdir().unwrap();
        let root = StorageRoot::from_path(temp.path()).unwrap();
        (temp, root)
    }

    fn metadata(created_days_ago: i64, launches: u32) -> BackupMetadata {
        BackupMetadata {
            created_at: Utc::now() - Duration::days(created_days_ago),
            validated_launches: launches,
        }
    }

    fn unknown_journal(dir: &str, ids: &[&str]) -> MigrationJournal {
        MigrationJournal::prepared(
            MigrationKind::Unknown,
            inventory_fingerprint(MigrationKind::Unknown, ids),
            None,
            None,
            dir.to_string(),
            ids,
            Utc::now(),
        )
    }

    fn completed_known_journal(dir: &str, key: AccountKey, ids: &[&str]) -> MigrationJournal {
        use crate::account::migration::journal::account_identity_hash;
        use crate::account::AccountId;
        let mut journal = MigrationJournal::prepared(
            MigrationKind::Known,
            inventory_fingerprint(MigrationKind::Known, ids),
            Some(key),
            Some(account_identity_hash(&AccountId::new("ACCT-1").unwrap())),
            dir.to_string(),
            ids,
            Utc::now(),
        );
        journal.phase = MigrationPhase::Complete;
        journal
    }

    fn mark_backed_up(journal: &mut MigrationJournal, id: &str, progress: ItemProgress) {
        if let Some(record) = journal.item_mut(id) {
            record.progress = progress;
        }
    }

    #[test]
    fn eligibility_requires_age_and_launches() {
        let now = Utc::now();
        assert!(!is_eligible_for_deletion(&metadata(10, 5), now));
        assert!(!is_eligible_for_deletion(&metadata(40, 1), now));
        assert!(is_eligible_for_deletion(&metadata(40, 3), now));
    }

    #[test]
    fn record_validated_launch_counts_only_matching_completed_known_journal() {
        let (_temp, root) = root();
        let key = AccountKey::generate();
        let journal = completed_known_journal("migration-backups/x", key, &[]);
        save_journal(&root, &journal).unwrap();
        assert_eq!(journal.backup.validated_launches, 0);

        record_validated_launch(&root, key).unwrap();
        record_validated_launch(&root, key).unwrap();

        let reloaded = load_journal(&root).unwrap().unwrap();
        assert_eq!(reloaded.backup.validated_launches, 2);
    }

    #[test]
    fn record_validated_launch_no_ops_for_unrelated_or_unknown_journals() {
        let (_temp, root) = root();
        let key = AccountKey::generate();

        // No journal at all: a non-migrated install is a silent no-op.
        record_validated_launch(&root, key).unwrap();
        assert!(load_journal(&root).unwrap().is_none());

        // An unknown-account journal is never counted.
        save_journal(&root, &unknown_journal("migration-backups/x", &[])).unwrap();
        record_validated_launch(&root, key).unwrap();
        assert_eq!(
            load_journal(&root)
                .unwrap()
                .unwrap()
                .backup
                .validated_launches,
            0
        );

        // A completed known journal for a different profile is not counted.
        let other = completed_known_journal("migration-backups/y", AccountKey::generate(), &[]);
        save_journal(&root, &other).unwrap();
        record_validated_launch(&root, key).unwrap();
        assert_eq!(
            load_journal(&root)
                .unwrap()
                .unwrap()
                .backup
                .validated_launches,
            0
        );

        // A still-incomplete known journal for the right key is not counted yet.
        let mut incomplete = completed_known_journal("migration-backups/z", key, &[]);
        incomplete.phase = MigrationPhase::RegistryCommitted;
        save_journal(&root, &incomplete).unwrap();
        record_validated_launch(&root, key).unwrap();
        assert_eq!(
            load_journal(&root)
                .unwrap()
                .unwrap()
                .backup
                .validated_launches,
            0
        );
    }

    #[test]
    fn delete_requires_confirmation() {
        let (_temp, root) = root();
        let dir = "migration-backups/2026-01-01T000000Z";
        root.create_directory(dir).unwrap();
        let mut journal = unknown_journal(dir, &[]);
        journal.backup = metadata(40, 3);
        let error = delete_backup(
            &root,
            &journal,
            &DeletionConfirmation {
                confirmed: false,
                backup_directory: dir.to_string(),
            },
            Utc::now(),
        )
        .unwrap_err();
        assert!(matches!(error, MigrationError::DeletionNotConfirmed));
        assert!(root.directory_exists(dir).unwrap());
    }

    #[test]
    fn delete_rejects_ineligible_backup() {
        let (_temp, root) = root();
        let dir = "migration-backups/2026-01-01T000000Z";
        root.create_directory(dir).unwrap();
        let mut journal = unknown_journal(dir, &[]);
        journal.backup = metadata(10, 3);
        let error = delete_backup(
            &root,
            &journal,
            &DeletionConfirmation {
                confirmed: true,
                backup_directory: dir.to_string(),
            },
            Utc::now(),
        )
        .unwrap_err();
        assert!(matches!(error, MigrationError::DeletionNotEligible));
        assert!(root.directory_exists(dir).unwrap());
    }

    #[test]
    fn delete_rejects_confirmation_for_a_different_backup() {
        // Metadata/confirmation from one backup must never delete another.
        let (_temp, root) = root();
        let dir = "migration-backups/2026-01-01T000000Z";
        root.create_directory(dir).unwrap();
        let mut journal = unknown_journal(dir, &[]);
        journal.backup = metadata(40, 3);
        let error = delete_backup(
            &root,
            &journal,
            &DeletionConfirmation {
                confirmed: true,
                backup_directory: "migration-backups/2020-01-01T000000Z".to_string(),
            },
            Utc::now(),
        )
        .unwrap_err();
        assert!(matches!(error, MigrationError::DeletionNotContained));
        assert!(root.directory_exists(dir).unwrap());
    }

    #[test]
    fn delete_rejects_uncontained_path() {
        let (_temp, root) = root();
        let mut journal = unknown_journal("accounts/secret", &[]);
        journal.backup = metadata(40, 3);
        let error = delete_backup(
            &root,
            &journal,
            &DeletionConfirmation {
                confirmed: true,
                backup_directory: "accounts/secret".to_string(),
            },
            Utc::now(),
        )
        .unwrap_err();
        assert!(matches!(error, MigrationError::DeletionNotContained));
    }

    #[test]
    fn delete_removes_confirmed_eligible_backup() {
        let (_temp, root) = root();
        let dir = "migration-backups/2026-01-01T000000Z";
        root.create_directory(dir).unwrap();
        fs::write(root.path().join(dir).join("account_data.json"), b"x").unwrap();
        let mut journal = unknown_journal(dir, &[]);
        journal.backup = metadata(40, 3);

        delete_backup(
            &root,
            &journal,
            &DeletionConfirmation {
                confirmed: true,
                backup_directory: dir.to_string(),
            },
            Utc::now(),
        )
        .unwrap();

        assert!(!root.directory_exists(dir).unwrap());
    }

    #[test]
    fn restore_backup_fills_absent_files_without_overwriting() {
        let (_temp, root) = root();
        let dir = "migration-backups/2026-01-01T000000Z";
        root.create_directory(dir).unwrap();
        fs::write(
            root.path().join(dir).join("account_data.json"),
            br#"{"a":1}"#,
        )
        .unwrap();
        fs::write(
            root.path().join(dir).join("quests.json"),
            br#"{"quests":[]}"#,
        )
        .unwrap();
        // A live file already exists and must not be overwritten.
        fs::write(root.path().join("account_data.json"), b"live").unwrap();

        let mut journal = unknown_journal(dir, &["account_data.json", "quests.json"]);
        let quests_digest =
            validate::hash_file_hex(&root.path().join(dir).join("quests.json")).unwrap();
        mark_backed_up(
            &mut journal,
            "account_data.json",
            ItemProgress {
                copied: true,
                validated: true,
                backed_up: true,
                digest: None,
            },
        );
        mark_backed_up(
            &mut journal,
            "quests.json",
            ItemProgress {
                copied: true,
                validated: true,
                backed_up: true,
                digest: Some(quests_digest),
            },
        );

        let restored = restore_backup(&root, &journal).unwrap();

        assert_eq!(
            fs::read(root.path().join("account_data.json")).unwrap(),
            b"live"
        );
        assert_eq!(
            fs::read(root.path().join("quests.json")).unwrap(),
            br#"{"quests":[]}"#
        );
        assert!(restored.contains(&PathBuf::from("quests.json")));
        assert!(!restored.contains(&PathBuf::from("account_data.json")));
    }

    #[test]
    fn restore_rejects_backup_whose_content_fails_journaled_digest() {
        let (_temp, root) = root();
        let dir = "migration-backups/2026-01-01T000000Z";
        root.create_directory(dir).unwrap();
        fs::write(root.path().join(dir).join("quests.json"), b"tampered").unwrap();

        let mut journal = unknown_journal(dir, &["quests.json"]);
        mark_backed_up(
            &mut journal,
            "quests.json",
            ItemProgress {
                copied: true,
                validated: true,
                backed_up: true,
                digest: Some("00".repeat(32)),
            },
        );

        let error = restore_backup(&root, &journal).unwrap_err();
        assert!(matches!(error, MigrationError::CopyMismatch { .. }));
        // Nothing was restored because validation failed before copying.
        assert!(!root.path().join("quests.json").exists());
    }

    #[test]
    fn restore_rejects_uncontained_backup() {
        let (_temp, root) = root();
        let journal = unknown_journal("accounts/secret", &[]);
        let error = restore_backup(&root, &journal).unwrap_err();
        assert!(matches!(error, MigrationError::DeletionNotContained));
    }
}

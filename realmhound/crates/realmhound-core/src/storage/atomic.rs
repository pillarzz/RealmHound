use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Serialize;

use super::{StorageError, StorageRoot};

/// Backup behavior used when replacing an existing document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupPolicy {
    /// Replace the primary document without retaining its prior contents.
    None,
    /// Retain one `<filename>.bak` sibling containing the prior document.
    Single,
}

/// Result of loading a primary JSON document and its optional backup.
#[derive(Debug, PartialEq, Eq)]
pub enum LoadOutcome<T> {
    /// The primary document was loaded.
    Primary(T),
    /// The primary was missing or malformed and the rolling backup was loaded.
    RecoveredBackup(T),
    /// Neither the primary nor its backup exists.
    Missing,
}

/// Serialize and atomically replace one root-relative JSON document.
pub fn write_json_atomic<T: Serialize>(
    root: &StorageRoot,
    relative: impl AsRef<Path>,
    value: &T,
    backup_policy: BackupPolicy,
) -> Result<(), StorageError> {
    let relative = relative.as_ref();
    // Validate + link-check + create the parent directory, then delegate to the
    // absolute-path core so both variants share one atomic implementation.
    let destination = root.prepare_parent(relative)?;
    write_json_atomic_at(&destination, value, backup_policy)
}

/// Serialize and atomically replace one JSON document at an explicit absolute
/// path, keeping one rolling `<filename>.bak` sibling per [`BackupPolicy`].
///
/// Shared atomic persistence for callers that hold a resolved absolute path
/// (per-account profile files and flat-layout compatibility files) rather than
/// a [`StorageRoot`]-relative path. The caller is responsible for path safety;
/// this creates parent directories and never follows the destination link.
pub fn write_json_atomic_at<T: Serialize>(
    path: &Path,
    value: &T,
    backup_policy: BackupPolicy,
) -> Result<(), StorageError> {
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|source| StorageError::SerializeDocument {
            path: path.to_path_buf(),
            source,
        })?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| StorageError::io("create storage directory", parent, error))?;
    }

    let (mut temporary, temporary_path) = create_unique_temporary(path)?;
    let mut guard = TemporaryGuard::new(temporary_path.clone());

    temporary
        .write_all(&bytes)
        .map_err(|error| StorageError::io("write temporary document", &temporary_path, error))?;
    temporary
        .sync_all()
        .map_err(|error| StorageError::io("flush temporary document", &temporary_path, error))?;
    drop(temporary);

    match backup_policy {
        BackupPolicy::None => {
            remove_backup_if_present(path)?;
            fs::rename(&temporary_path, path)
                .map_err(|error| StorageError::io("replace document", path, error))?;
        }
        BackupPolicy::Single => {
            replace_with_backup(&temporary_path, path)?;
        }
    }

    guard.disarm();
    Ok(())
}

/// Load a root-relative JSON document, recovering its rolling backup when the
/// primary is missing or malformed.
pub fn load_json<T: DeserializeOwned>(
    root: &StorageRoot,
    relative: impl AsRef<Path>,
) -> Result<LoadOutcome<T>, StorageError> {
    let relative = relative.as_ref();
    let primary = root.ensure_no_links(relative)?;
    let backup_relative = sibling_with_suffix(relative, ".bak")?;
    // Preserve the link-check on the backup sibling before reading it.
    root.ensure_no_links(backup_relative)?;
    load_json_at(&primary)
}

/// Load a JSON document from an explicit absolute path, recovering its rolling
/// `<filename>.bak` sibling when the primary is missing or malformed. The
/// absolute-path counterpart to [`load_json`].
pub fn load_json_at<T: DeserializeOwned>(path: &Path) -> Result<LoadOutcome<T>, StorageError> {
    let backup = sibling_with_suffix(path, ".bak")?;
    let primary_result = read_candidate(path)?;

    match primary_result {
        Candidate::Loaded(value) => Ok(LoadOutcome::Primary(value)),
        Candidate::Missing => match read_candidate(&backup)? {
            Candidate::Loaded(value) => Ok(LoadOutcome::RecoveredBackup(value)),
            Candidate::Missing => Ok(LoadOutcome::Missing),
            Candidate::Invalid(source) => Err(StorageError::InvalidDocument {
                path: backup,
                source,
            }),
        },
        Candidate::Invalid(primary_error) => match read_candidate(&backup)? {
            Candidate::Loaded(value) => Ok(LoadOutcome::RecoveredBackup(value)),
            Candidate::Missing => Err(StorageError::InvalidDocument {
                path: path.to_path_buf(),
                source: primary_error,
            }),
            Candidate::Invalid(backup_error) => Err(StorageError::InvalidDocuments {
                primary: path.to_path_buf(),
                primary_error: primary_error.to_string(),
                backup,
                backup_error: backup_error.to_string(),
            }),
        },
    }
}

enum Candidate<T> {
    Loaded(T),
    Missing,
    Invalid(serde_json::Error),
}

fn read_candidate<T: DeserializeOwned>(path: &Path) -> Result<Candidate<T>, StorageError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Candidate::Missing),
        Err(error) => return Err(StorageError::io("read document", path, error)),
    };

    match serde_json::from_slice(&bytes) {
        Ok(value) => Ok(Candidate::Loaded(value)),
        Err(error) => Ok(Candidate::Invalid(error)),
    }
}

fn create_unique_temporary(destination: &Path) -> Result<(File, PathBuf), StorageError> {
    for _ in 0..32 {
        let suffix = format!(".tmp-{}-{:016x}", std::process::id(), rand::random::<u64>());
        let path = sibling_with_suffix(destination, &suffix)?;
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((file, path)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(StorageError::io("create temporary document", &path, error));
            }
        }
    }

    Err(StorageError::io(
        "create unique temporary document",
        destination,
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "temporary name collision limit reached",
        ),
    ))
}

fn replace_with_backup(temporary: &Path, destination: &Path) -> Result<(), StorageError> {
    match fs::symlink_metadata(destination) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            remove_backup_if_present(destination)?;
            return fs::rename(temporary, destination)
                .map_err(|error| StorageError::io("persist document", destination, error));
        }
        Ok(_) => {}
        Err(error) => {
            return Err(StorageError::io(
                "inspect primary document",
                destination,
                error,
            ));
        }
    }

    let backup = backup_path(destination)?;
    remove_existing_backup(&backup)?;
    fs::rename(destination, &backup)
        .map_err(|error| StorageError::io("rotate document backup", destination, error))?;

    if let Err(source) = fs::rename(temporary, destination) {
        let rollback = match fs::rename(&backup, destination) {
            Ok(()) => "restored the previous primary".to_string(),
            Err(error) => format!(
                "previous primary remains at `{}` because restoration failed: {}",
                backup.display(),
                error
            ),
        };
        return Err(StorageError::Replacement {
            path: destination.to_path_buf(),
            source,
            rollback,
        });
    }

    Ok(())
}

fn remove_backup_if_present(destination: &Path) -> Result<(), StorageError> {
    remove_existing_backup(&backup_path(destination)?)
}

fn remove_existing_backup(backup: &Path) -> Result<(), StorageError> {
    match fs::symlink_metadata(&backup) {
        Ok(metadata) if metadata.is_dir() => {
            return Err(StorageError::io(
                "remove prior document backup",
                &backup,
                io::Error::new(io::ErrorKind::IsADirectory, "backup path is a directory"),
            ));
        }
        Ok(_) => fs::remove_file(&backup)
            .map_err(|error| StorageError::io("remove prior document backup", &backup, error))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(StorageError::io(
                "inspect prior document backup",
                &backup,
                error,
            ));
        }
    }
    Ok(())
}

fn backup_path(path: &Path) -> Result<PathBuf, StorageError> {
    sibling_with_suffix(path, ".bak")
}

fn sibling_with_suffix(path: &Path, suffix: &str) -> Result<PathBuf, StorageError> {
    let file_name = path.file_name().ok_or(StorageError::MissingFileName)?;
    let mut sibling = OsString::from(file_name);
    sibling.push(suffix);
    Ok(path.with_file_name(sibling))
}

struct TemporaryGuard {
    path: PathBuf,
    armed: bool,
}

impl TemporaryGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TemporaryGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde::{Deserialize, Serialize};
    use tempfile::TempDir;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Document {
        value: u32,
    }

    fn root() -> (TempDir, StorageRoot) {
        let temp = tempfile::tempdir().unwrap();
        let root = StorageRoot::from_path(temp.path().to_path_buf()).unwrap();
        (temp, root)
    }

    #[test]
    fn write_and_load_round_trip_uses_primary() {
        let (_temp, root) = root();
        let expected = Document { value: 7 };
        write_json_atomic(&root, "registry.json", &expected, BackupPolicy::Single).unwrap();

        let loaded = load_json::<Document>(&root, "registry.json").unwrap();

        assert_eq!(loaded, LoadOutcome::Primary(expected));
    }

    #[test]
    fn repeated_write_keeps_one_previous_document() {
        let (_temp, root) = root();
        write_json_atomic(
            &root,
            "registry.json",
            &Document { value: 1 },
            BackupPolicy::Single,
        )
        .unwrap();
        write_json_atomic(
            &root,
            "registry.json",
            &Document { value: 2 },
            BackupPolicy::Single,
        )
        .unwrap();

        let backup = fs::read(root.path().join("registry.json.bak")).unwrap();

        assert_eq!(
            serde_json::from_slice::<Document>(&backup).unwrap(),
            Document { value: 1 }
        );
    }

    #[test]
    fn missing_primary_recovers_backup() {
        let (_temp, root) = root();
        write_json_atomic(
            &root,
            "registry.json",
            &Document { value: 3 },
            BackupPolicy::Single,
        )
        .unwrap();
        fs::rename(
            root.path().join("registry.json"),
            root.path().join("registry.json.bak"),
        )
        .unwrap();

        let loaded = load_json::<Document>(&root, "registry.json").unwrap();

        assert_eq!(loaded, LoadOutcome::RecoveredBackup(Document { value: 3 }));
    }

    #[test]
    fn malformed_primary_recovers_valid_backup() {
        let (_temp, root) = root();
        write_json_atomic(
            &root,
            "registry.json",
            &Document { value: 4 },
            BackupPolicy::Single,
        )
        .unwrap();
        write_json_atomic(
            &root,
            "registry.json",
            &Document { value: 5 },
            BackupPolicy::Single,
        )
        .unwrap();
        fs::write(root.path().join("registry.json"), b"{").unwrap();

        let loaded = load_json::<Document>(&root, "registry.json").unwrap();

        assert_eq!(loaded, LoadOutcome::RecoveredBackup(Document { value: 4 }));
    }

    #[test]
    fn malformed_primary_without_backup_returns_error() {
        let (_temp, root) = root();
        fs::write(root.path().join("registry.json"), b"{").unwrap();

        let error = load_json::<Document>(&root, "registry.json").unwrap_err();

        assert!(matches!(error, StorageError::InvalidDocument { .. }));
    }

    #[test]
    fn malformed_primary_and_backup_return_combined_error() {
        let (_temp, root) = root();
        fs::write(root.path().join("registry.json"), b"{").unwrap();
        fs::write(root.path().join("registry.json.bak"), b"[").unwrap();

        let error = load_json::<Document>(&root, "registry.json").unwrap_err();

        assert!(matches!(error, StorageError::InvalidDocuments { .. }));
    }

    #[test]
    fn directory_read_error_is_not_treated_as_missing() {
        let (_temp, root) = root();
        fs::create_dir(root.path().join("registry.json")).unwrap();

        let error = load_json::<Document>(&root, "registry.json").unwrap_err();

        assert!(matches!(error, StorageError::Io { .. }));
    }

    #[test]
    fn traversal_is_rejected_for_document_operations() {
        let (_temp, root) = root();

        let error = write_json_atomic(
            &root,
            "../registry.json",
            &Document { value: 8 },
            BackupPolicy::Single,
        )
        .unwrap_err();

        assert!(matches!(error, StorageError::InvalidRelativePath(_)));
    }

    #[test]
    fn no_backup_policy_replaces_primary_and_removes_old_backup() {
        let (_temp, root) = root();
        write_json_atomic(
            &root,
            "registry.json",
            &Document { value: 9 },
            BackupPolicy::Single,
        )
        .unwrap();
        write_json_atomic(
            &root,
            "registry.json",
            &Document { value: 10 },
            BackupPolicy::Single,
        )
        .unwrap();

        write_json_atomic(
            &root,
            "registry.json",
            &Document { value: 11 },
            BackupPolicy::None,
        )
        .unwrap();

        assert!(!root.path().join("registry.json.bak").exists());
        assert_eq!(
            load_json(&root, "registry.json").unwrap(),
            LoadOutcome::Primary(Document { value: 11 })
        );
    }

    #[test]
    fn writing_missing_primary_removes_stale_backup() {
        let (_temp, root) = root();
        write_json_atomic(
            &root,
            "registry.json",
            &Document { value: 12 },
            BackupPolicy::Single,
        )
        .unwrap();
        write_json_atomic(
            &root,
            "registry.json",
            &Document { value: 13 },
            BackupPolicy::Single,
        )
        .unwrap();
        fs::remove_file(root.path().join("registry.json")).unwrap();

        write_json_atomic(
            &root,
            "registry.json",
            &Document { value: 14 },
            BackupPolicy::Single,
        )
        .unwrap();

        assert!(!root.path().join("registry.json.bak").exists());
    }

    #[test]
    fn replacement_failure_preserves_primary_and_cleans_temporary() {
        let (_temp, root) = root();
        write_json_atomic(
            &root,
            "registry.json",
            &Document { value: 6 },
            BackupPolicy::Single,
        )
        .unwrap();
        fs::create_dir(root.path().join("registry.json.bak")).unwrap();

        let error = write_json_atomic(
            &root,
            "registry.json",
            &Document { value: 7 },
            BackupPolicy::Single,
        )
        .unwrap_err();

        assert!(matches!(error, StorageError::Io { .. }));
        let primary = serde_json::from_slice::<Document>(
            &fs::read(root.path().join("registry.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(primary, Document { value: 6 });
        let temporary_count = fs::read_dir(root.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .count();
        assert_eq!(temporary_count, 0);
    }
}

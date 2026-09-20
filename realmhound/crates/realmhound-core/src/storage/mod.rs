//! Validated storage roots and narrowly scoped file operations.

mod atomic;

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use thiserror::Error;

pub use atomic::{
    load_json, load_json_at, write_json_atomic, write_json_atomic_at, BackupPolicy, LoadOutcome,
};

/// Errors produced by validated storage and document operations.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("could not determine the local application data directory")]
    LocalDataUnavailable,
    #[error("storage root must be absolute: {0}")]
    RootNotAbsolute(PathBuf),
    #[error("storage path must be relative and cannot escape the root: {0}")]
    InvalidRelativePath(PathBuf),
    #[error("storage path cannot be empty")]
    MissingFileName,
    #[error(
        "symbolic links and Windows reparse points are not allowed below the storage root: {0}"
    )]
    LinkNotAllowed(PathBuf),
    #[error("storage source is not a regular file: {0}")]
    SourceNotFile(PathBuf),
    #[error("storage path is not a directory: {0}")]
    NotDirectory(PathBuf),
    #[error("storage destination already exists: {0}")]
    DestinationExists(PathBuf),
    #[error("SQLite files require the dedicated checkpoint and backup workflow: {0}")]
    SqliteRequiresBackup(PathBuf),
    #[error("failed to {operation} `{path}`: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid JSON document `{path}`: {source}")]
    InvalidDocument {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to serialize JSON document `{path}`: {source}")]
    SerializeDocument {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(
        "neither JSON document is valid (`{primary}`: {primary_error}; `{backup}`: {backup_error})"
    )]
    InvalidDocuments {
        primary: PathBuf,
        primary_error: String,
        backup: PathBuf,
        backup_error: String,
    },
    #[error("failed to replace `{path}`: {source}; rollback: {rollback}")]
    Replacement {
        path: PathBuf,
        #[source]
        source: io::Error,
        rollback: String,
    },
}

impl StorageError {
    pub(super) fn io(operation: &'static str, path: &Path, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.to_path_buf(),
            source,
        }
    }
}

/// Root directory used for all RealmHound storage operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageRoot {
    path: PathBuf,
}

impl StorageRoot {
    /// Resolve the default `%LOCALAPPDATA%\RealmHound` storage root.
    pub fn default_local() -> Result<Self, StorageError> {
        static DEFAULT_ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
        let path = DEFAULT_ROOT
            .get_or_init(|| dirs::data_local_dir().map(|path| path.join("RealmHound")))
            .clone()
            .ok_or(StorageError::LocalDataUnavailable)?;
        Self::from_path(path)
    }

    /// Create a storage root from an explicit absolute path.
    pub fn from_path(path: impl Into<PathBuf>) -> Result<Self, StorageError> {
        let path = path.into();
        if !path.is_absolute() {
            return Err(StorageError::RootNotAbsolute(path));
        }
        Ok(Self { path })
    }

    /// Return the configured root path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Resolve a root-relative path without permitting traversal.
    pub fn resolve(&self, relative: impl AsRef<Path>) -> Result<PathBuf, StorageError> {
        let relative = relative.as_ref();
        if relative.as_os_str().is_empty() {
            return Ok(self.path.clone());
        }

        let mut resolved = self.path.clone();
        for component in relative.components() {
            match component {
                Component::Normal(value) => resolved.push(value),
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(StorageError::InvalidRelativePath(relative.to_path_buf()));
                }
            }
        }
        Ok(resolved)
    }

    /// Verify that existing components below the root contain no symbolic
    /// links or Windows reparse points.
    pub fn ensure_no_links(&self, relative: impl AsRef<Path>) -> Result<PathBuf, StorageError> {
        let relative = relative.as_ref();
        let resolved = self.resolve(relative)?;
        let mut current = self.path.clone();

        for component in relative.components() {
            let Component::Normal(value) = component else {
                continue;
            };
            current.push(value);
            match fs::symlink_metadata(&current) {
                Ok(metadata) if is_link_or_reparse(&metadata) => {
                    return Err(StorageError::LinkNotAllowed(current));
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => break,
                Err(error) => {
                    return Err(StorageError::io("inspect storage path", &current, error));
                }
            }
        }

        Ok(resolved)
    }

    /// Return immediate root-relative children without traversing them.
    pub fn read_directory(&self, relative: impl AsRef<Path>) -> Result<Vec<PathBuf>, StorageError> {
        let relative = relative.as_ref();
        let directory = self.ensure_no_links(relative)?;
        let metadata = match fs::metadata(&directory) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(StorageError::io(
                    "inspect storage directory",
                    &directory,
                    error,
                ));
            }
        };
        if !metadata.is_dir() {
            return Err(StorageError::NotDirectory(directory));
        }

        let entries = fs::read_dir(&directory)
            .map_err(|error| StorageError::io("read storage directory", &directory, error))?;
        let mut children = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| {
                StorageError::io("read storage directory entry", &directory, error)
            })?;
            children.push(relative.join(entry.file_name()));
        }
        children.sort();
        Ok(children)
    }

    /// Return whether a validated root-relative directory exists.
    pub fn directory_exists(&self, relative: impl AsRef<Path>) -> Result<bool, StorageError> {
        let path = self.ensure_no_links(relative)?;
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_dir() => Ok(true),
            Ok(_) => Err(StorageError::NotDirectory(path)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(StorageError::io("inspect storage directory", &path, error)),
        }
    }

    /// Create a validated root-relative directory.
    pub fn create_directory(&self, relative: impl AsRef<Path>) -> Result<PathBuf, StorageError> {
        let relative = relative.as_ref();
        let path = self.ensure_no_links(relative)?;
        fs::create_dir_all(&path)
            .map_err(|error| StorageError::io("create storage directory", &path, error))?;
        self.ensure_no_links(relative)
    }

    /// Remove a validated directory only when it is empty.
    pub fn remove_empty_directory(&self, relative: impl AsRef<Path>) -> Result<(), StorageError> {
        let path = self.ensure_no_links(relative)?;
        match fs::remove_dir(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(StorageError::io(
                "remove empty storage directory",
                &path,
                error,
            )),
        }
    }

    /// Recursively remove a validated directory and all its contents.
    ///
    /// The path must be a non-empty relative path below the storage root with
    /// no symlinks or reparse points on the path to the directory itself.
    /// Already-absent directories are not an error.
    pub fn remove_directory_all(&self, relative: impl AsRef<Path>) -> Result<(), StorageError> {
        let relative = relative.as_ref();
        if relative.as_os_str().is_empty() {
            return Err(StorageError::InvalidRelativePath(relative.to_path_buf()));
        }
        let path = self.ensure_no_links(relative)?;
        match fs::remove_dir_all(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(StorageError::io(
                "remove storage directory tree",
                &path,
                error,
            )),
        }
    }

    /// Copy one regular non-SQLite file within the storage root.
    pub fn copy_file(
        &self,
        source: impl AsRef<Path>,
        destination: impl AsRef<Path>,
    ) -> Result<u64, StorageError> {
        let source = source.as_ref();
        let destination = destination.as_ref();
        reject_sqlite_path(source)?;
        reject_sqlite_path(destination)?;

        let source_path = self.existing_file(source)?;
        let destination_path = self.prepare_new_destination(destination)?;
        let mut source_file = File::open(&source_path)
            .map_err(|error| StorageError::io("open source file", &source_path, error))?;
        let mut destination_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination_path)
            .map_err(|error| {
                if error.kind() == io::ErrorKind::AlreadyExists {
                    StorageError::DestinationExists(destination_path.clone())
                } else {
                    StorageError::io("create destination file", &destination_path, error)
                }
            })?;

        let copied = io::copy(&mut source_file, &mut destination_file)
            .and_then(|bytes| destination_file.sync_all().map(|()| bytes));
        match copied {
            Ok(bytes) => Ok(bytes),
            Err(error) => {
                drop(destination_file);
                let _ = fs::remove_file(&destination_path);
                Err(StorageError::io("copy file", &destination_path, error))
            }
        }
    }

    /// Move one regular non-SQLite file within the storage root.
    pub fn move_file(
        &self,
        source: impl AsRef<Path>,
        destination: impl AsRef<Path>,
    ) -> Result<(), StorageError> {
        let source = source.as_ref();
        let destination = destination.as_ref();
        reject_sqlite_path(source)?;
        reject_sqlite_path(destination)?;

        let source_path = self.existing_file(source)?;
        let destination_path = self.prepare_new_destination(destination)?;
        match fs::hard_link(&source_path, &destination_path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Err(StorageError::DestinationExists(destination_path));
            }
            Err(_) => {
                self.copy_file(source, destination)?;
            }
        }

        if let Err(error) = fs::remove_file(&source_path) {
            let _ = fs::remove_file(&destination_path);
            return Err(StorageError::io("remove moved source", &source_path, error));
        }
        Ok(())
    }

    /// Move one regular file to a validated quarantine path.
    pub fn quarantine_file(
        &self,
        source: impl AsRef<Path>,
        quarantine_path: impl AsRef<Path>,
    ) -> Result<(), StorageError> {
        self.move_file(source, quarantine_path)
    }

    fn prepare_parent(&self, relative: &Path) -> Result<PathBuf, StorageError> {
        if relative.file_name().is_none() {
            return Err(StorageError::MissingFileName);
        }
        let destination = self.ensure_no_links(relative)?;
        let parent = destination.parent().ok_or(StorageError::MissingFileName)?;
        fs::create_dir_all(parent)
            .map_err(|error| StorageError::io("create storage directory", parent, error))?;

        if let Some(relative_parent) = relative.parent() {
            self.ensure_no_links(relative_parent)?;
        }
        Ok(destination)
    }

    fn existing_file(&self, relative: &Path) -> Result<PathBuf, StorageError> {
        let path = self.ensure_no_links(relative)?;
        let metadata = fs::metadata(&path)
            .map_err(|error| StorageError::io("inspect source file", &path, error))?;
        if !metadata.is_file() {
            return Err(StorageError::SourceNotFile(path));
        }
        Ok(path)
    }

    fn prepare_new_destination(&self, relative: &Path) -> Result<PathBuf, StorageError> {
        let path = self.prepare_parent(relative)?;
        match fs::symlink_metadata(&path) {
            Ok(_) => Err(StorageError::DestinationExists(path)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(path),
            Err(error) => Err(StorageError::io("inspect destination file", &path, error)),
        }
    }
}

fn reject_sqlite_path(path: &Path) -> Result<(), StorageError> {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Err(StorageError::MissingFileName);
    };
    let name = name.to_ascii_lowercase();
    if [".db", ".sqlite", ".sqlite3", "-wal", "-shm", "-journal"]
        .iter()
        .any(|suffix| name.ends_with(suffix))
    {
        return Err(StorageError::SqliteRequiresBackup(path.to_path_buf()));
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

    use tempfile::TempDir;

    fn root() -> (TempDir, StorageRoot) {
        let temp = tempfile::tempdir().unwrap();
        let root = StorageRoot::from_path(temp.path().to_path_buf()).unwrap();
        (temp, root)
    }

    #[test]
    fn resolve_accepts_nested_relative_path() {
        let (_temp, root) = root();

        let resolved = root
            .resolve(Path::new("accounts").join("profile.json"))
            .unwrap();

        assert_eq!(resolved, root.path().join("accounts").join("profile.json"));
    }

    #[test]
    fn resolve_rejects_parent_traversal() {
        let (_temp, root) = root();

        let error = root
            .resolve(Path::new("accounts").join("..").join("outside"))
            .unwrap_err();

        assert!(matches!(error, StorageError::InvalidRelativePath(_)));
    }

    #[test]
    fn resolve_rejects_absolute_path() {
        let (_temp, root) = root();
        let absolute = root.path().join("outside.json");

        let error = root.resolve(absolute).unwrap_err();

        assert!(matches!(error, StorageError::InvalidRelativePath(_)));
    }

    #[test]
    fn read_directory_returns_empty_for_missing_directory() {
        let (_temp, root) = root();

        assert!(root.read_directory("accounts").unwrap().is_empty());
    }

    #[test]
    fn read_directory_returns_only_immediate_paths() {
        let (_temp, root) = root();
        fs::create_dir_all(root.path().join("accounts").join("first").join("nested")).unwrap();
        fs::create_dir_all(root.path().join("accounts").join("second")).unwrap();

        let children = root.read_directory("accounts").unwrap();

        assert_eq!(
            children,
            vec![
                PathBuf::from("accounts").join("first"),
                PathBuf::from("accounts").join("second")
            ]
        );
    }

    #[test]
    fn remove_empty_directory_refuses_nonempty_directory() {
        let (_temp, root) = root();
        root.create_directory("accounts/profile").unwrap();
        fs::write(root.path().join("accounts/profile/data.json"), b"data").unwrap();

        assert!(root.remove_empty_directory("accounts/profile").is_err());
    }

    #[test]
    fn remove_directory_all_removes_nonempty_tree() {
        let (_temp, root) = root();
        root.create_directory("accounts/profile/databases").unwrap();
        fs::write(root.path().join("accounts/profile/data.json"), b"data").unwrap();
        fs::write(
            root.path().join("accounts/profile/databases/loot.db"),
            b"db",
        )
        .unwrap();

        root.remove_directory_all("accounts/profile").unwrap();

        assert!(!root.path().join("accounts/profile").exists());
        // Parent directory remains.
        assert!(root.path().join("accounts").exists());
    }

    #[test]
    fn remove_directory_all_is_idempotent_for_missing_directory() {
        let (_temp, root) = root();
        root.remove_directory_all("accounts/nonexistent").unwrap();
    }

    #[test]
    fn remove_directory_all_rejects_empty_relative_path() {
        let (_temp, root) = root();
        assert!(root.remove_directory_all("").is_err());
    }

    #[test]
    fn copy_file_copies_regular_file() {
        let (_temp, root) = root();
        fs::write(root.path().join("source.json"), b"source").unwrap();

        root.copy_file("source.json", "copies/destination.json")
            .unwrap();

        assert_eq!(
            fs::read(root.path().join("copies/destination.json")).unwrap(),
            b"source"
        );
    }

    #[test]
    fn move_file_rejects_existing_destination() {
        let (_temp, root) = root();
        fs::write(root.path().join("source.json"), b"source").unwrap();
        fs::write(root.path().join("destination.json"), b"destination").unwrap();

        let error = root
            .move_file("source.json", "destination.json")
            .unwrap_err();

        assert!(matches!(error, StorageError::DestinationExists(_)));
    }

    #[test]
    fn quarantine_file_moves_regular_file() {
        let (_temp, root) = root();
        fs::write(root.path().join("legacy.json"), b"legacy").unwrap();

        root.quarantine_file("legacy.json", "quarantine/legacy-unassigned/legacy.json")
            .unwrap();

        assert!(root
            .path()
            .join("quarantine/legacy-unassigned/legacy.json")
            .exists());
    }

    #[test]
    fn copy_file_rejects_sqlite_database() {
        let (_temp, root) = root();
        fs::write(root.path().join("history.db"), b"sqlite").unwrap();

        let error = root.copy_file("history.db", "copy.db").unwrap_err();

        assert!(matches!(error, StorageError::SqliteRequiresBackup(_)));
    }

    #[test]
    fn copy_file_rejects_escaping_destination() {
        let (_temp, root) = root();
        fs::write(root.path().join("source.json"), b"source").unwrap();

        let error = root
            .copy_file("source.json", "../outside.json")
            .unwrap_err();

        assert!(matches!(error, StorageError::InvalidRelativePath(_)));
    }

    #[cfg(windows)]
    #[test]
    fn ensure_no_links_rejects_windows_symlink_when_available() {
        use std::os::windows::fs::symlink_file;

        let (_temp, root) = root();
        fs::write(root.path().join("target.json"), b"target").unwrap();
        let link = root.path().join("link.json");
        if symlink_file(root.path().join("target.json"), &link).is_err() {
            return;
        }

        let error = root.ensure_no_links("link.json").unwrap_err();

        assert!(matches!(error, StorageError::LinkNotAllowed(_)));
    }
}

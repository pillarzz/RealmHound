//! OS-backed exclusive locks for flat-layout migration.
//!
//! Migration takes an advisory exclusive lock on a dedicated lock file under the
//! storage root using [`fs2`], plus a second lock scoped to the target profile
//! or quarantine directory. These are the authoritative guards; the process-wide
//! named mutex used elsewhere is only defense in depth. A failure to acquire a
//! lock is explicit and never silently proceeds, so no migration source is
//! changed while another owner holds the data.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use fs2::FileExt;

use super::MigrationError;
use crate::storage::StorageRoot;

/// Relative path of the exclusive migration root lock.
pub const ROOT_LOCK_PATH: &str = "migrations/flat-layout-v1.lock";

/// The authoritative sibling `.lock` path guarding a target directory.
///
/// Both migration and selected-profile startup lock a profile through this one
/// path so they contend on the exact same file (never a child inside the
/// directory), keeping migration and normal startup mutually exclusive.
pub(crate) fn target_lock_relative(target_relative: &Path) -> PathBuf {
    let mut relative = target_relative.as_os_str().to_os_string();
    relative.push(".lock");
    PathBuf::from(relative)
}

/// A held advisory lock. The lock is released when this guard is dropped.
#[derive(Debug)]
pub struct MigrationLock {
    file: File,
    path: PathBuf,
}

impl MigrationLock {
    /// Acquire the exclusive migration root lock, failing if another owner holds
    /// it rather than blocking.
    pub fn acquire_root(root: &StorageRoot) -> Result<Self, MigrationError> {
        let path = root
            .ensure_no_links(ROOT_LOCK_PATH)
            .map_err(MigrationError::Storage)?;
        Self::acquire_at(path)
    }

    /// Acquire an exclusive lock scoped to a target directory (profile or
    /// quarantine). The lock file is a sibling `.lock`, never following links.
    pub fn acquire_target(
        root: &StorageRoot,
        target_relative: &Path,
    ) -> Result<Self, MigrationError> {
        let path = root
            .ensure_no_links(target_lock_relative(target_relative))
            .map_err(MigrationError::Storage)?;
        Self::acquire_at(path)
    }

    fn acquire_at(path: PathBuf) -> Result<Self, MigrationError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| MigrationError::LockIo {
                path: path.clone(),
                source,
            })?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|source| MigrationError::LockIo {
                path: path.clone(),
                source,
            })?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Self { file, path }),
            Err(_) => Err(MigrationError::LockContended { path }),
        }
    }

    /// Return the path of the held lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for MigrationLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> (tempfile::TempDir, StorageRoot) {
        let temp = tempfile::tempdir().unwrap();
        let root = StorageRoot::from_path(temp.path()).unwrap();
        (temp, root)
    }

    #[test]
    fn root_lock_is_exclusive_while_held() {
        let (_temp, root) = root();
        let held = MigrationLock::acquire_root(&root).unwrap();

        let error = MigrationLock::acquire_root(&root).unwrap_err();
        assert!(matches!(error, MigrationError::LockContended { .. }));

        drop(held);
        // Re-acquirable once released.
        let _reacquired = MigrationLock::acquire_root(&root).unwrap();
    }

    #[test]
    fn target_lock_is_independent_of_root_lock() {
        let (_temp, root) = root();
        let _root_lock = MigrationLock::acquire_root(&root).unwrap();
        let target = Path::new("accounts").join("target");
        let _target_lock = MigrationLock::acquire_target(&root, &target).unwrap();
    }

    #[test]
    fn target_lock_is_exclusive_for_same_target() {
        let (_temp, root) = root();
        let target = Path::new("quarantine").join("legacy-unassigned");
        let held = MigrationLock::acquire_target(&root, &target).unwrap();
        let error = MigrationLock::acquire_target(&root, &target).unwrap_err();
        assert!(matches!(error, MigrationError::LockContended { .. }));
        drop(held);
    }
}

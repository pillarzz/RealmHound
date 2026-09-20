//! Exclusive process-lifetime lock for one account profile.
//!
//! Selected-profile startup acquires this lock before opening any account
//! snapshot, cache, quest, import, log, credential, or SQLite reader/writer, and
//! holds it for the lifetime of the process through the shared account context.
//! It is an OS-backed advisory exclusive lock on the profile's sibling
//! `accounts/<key>.lock` file -- the same authoritative target-lock path the
//! flat-layout migration uses -- so migration and normal startup contend on one
//! file and never open a profile at the same time. A failure to acquire is
//! explicit and never silently proceeds.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use thiserror::Error;

use super::migration::lock::target_lock_relative;
use super::{AccountKey, AccountPaths};
use crate::storage::{StorageError, StorageRoot};

/// A held exclusive profile lock. Released when dropped.
#[derive(Debug)]
pub struct ProfileLock {
    file: File,
    path: PathBuf,
    account_key: AccountKey,
}

impl ProfileLock {
    /// Acquire the exclusive lock for a profile, failing closed if another owner
    /// holds it rather than blocking.
    pub fn acquire(root: &StorageRoot, account_key: AccountKey) -> Result<Self, ProfileLockError> {
        let paths = AccountPaths::new(root.clone(), account_key);
        let relative = target_lock_relative(&paths.relative_root());
        let path = root
            .ensure_no_links(&relative)
            .map_err(ProfileLockError::Storage)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| ProfileLockError::Io {
                path: path.clone(),
                source,
            })?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|source| ProfileLockError::Io {
                path: path.clone(),
                source,
            })?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Self {
                file,
                path,
                account_key,
            }),
            Err(_) => Err(ProfileLockError::Contended { account_key }),
        }
    }

    /// Return the account key this lock guards.
    pub fn account_key(&self) -> AccountKey {
        self.account_key
    }

    /// Acquire the lock, waiting up to `timeout` for a current owner to release.
    /// Non-contention errors (storage/IO) return immediately.
    pub fn acquire_blocking(
        root: &StorageRoot,
        account_key: AccountKey,
        timeout: std::time::Duration,
    ) -> Result<Self, ProfileLockError> {
        let deadline = std::time::Instant::now() + timeout;
        let poll = std::time::Duration::from_millis(50);
        loop {
            match Self::acquire(root, account_key) {
                Err(ProfileLockError::Contended { .. }) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(poll);
                }
                other => return other,
            }
        }
    }

    /// Return the path of the held lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ProfileLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// Errors returned while acquiring a [`ProfileLock`]. No variant carries
/// account data or credentials.
#[derive(Debug, Error)]
pub enum ProfileLockError {
    /// The lock path could not be validated below the storage root.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// The lock file could not be opened or created.
    #[error("failed to prepare profile lock `{path}`: {source}")]
    Io {
        /// Lock path.
        path: PathBuf,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },
    /// Another owner already holds the profile lock.
    #[error("profile {account_key} is already locked by another instance")]
    Contended {
        /// The contended account key.
        account_key: AccountKey,
    },
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
    fn lock_is_exclusive_while_held() {
        let (_temp, root) = root();
        let key = AccountKey::generate();
        let held = ProfileLock::acquire(&root, key).unwrap();

        let error = ProfileLock::acquire(&root, key).unwrap_err();
        assert!(matches!(error, ProfileLockError::Contended { .. }));

        drop(held);
        let _reacquired = ProfileLock::acquire(&root, key).unwrap();
    }

    #[test]
    fn distinct_profiles_lock_independently() {
        let (_temp, root) = root();
        let first = AccountKey::generate();
        let second = AccountKey::generate();
        let _a = ProfileLock::acquire(&root, first).unwrap();
        let _b = ProfileLock::acquire(&root, second).unwrap();
    }

    #[test]
    fn acquire_blocking_waits_for_release() {
        use std::sync::mpsc;
        use std::time::Duration;

        let (temp, root) = root();
        let key = AccountKey::generate();
        let held = ProfileLock::acquire(&root, key).unwrap();

        // Release from another thread; the blocking acquire should wait, then succeed.
        let (tx, rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            drop(held);
            tx.send(()).unwrap();
        });

        let acquired = ProfileLock::acquire_blocking(&root, key, Duration::from_secs(5)).unwrap();
        rx.recv().unwrap();
        handle.join().unwrap();
        assert_eq!(acquired.account_key(), key);
        drop(temp);
    }

    #[test]
    fn acquire_blocking_times_out_while_held() {
        use std::time::Duration;

        let (_temp, root) = root();
        let key = AccountKey::generate();
        let _held = ProfileLock::acquire(&root, key).unwrap();

        let error =
            ProfileLock::acquire_blocking(&root, key, Duration::from_millis(150)).unwrap_err();
        assert!(matches!(error, ProfileLockError::Contended { .. }));
    }

    #[test]
    fn lock_file_is_the_authoritative_sibling_target_lock() {
        let (_temp, root) = root();
        let key = AccountKey::generate();
        let held = ProfileLock::acquire(&root, key).unwrap();
        // The profile lock lives at the sibling `accounts/<key>.lock`, matching
        // the migration target-lock path so the two contend on one file.
        let expected = root.path().join("accounts").join(format!("{key}.lock"));
        assert_eq!(held.path(), expected);
    }

    #[test]
    fn profile_lock_and_migration_target_lock_contend() {
        use crate::account::migration::lock::MigrationLock;
        use crate::account::migration::MigrationError;
        let (_temp, root) = root();
        let key = AccountKey::generate();
        let profile_relative = AccountPaths::new(root.clone(), key).relative_root();

        // Migration holding the profile target lock blocks selected startup.
        let migration_lock = MigrationLock::acquire_target(&root, &profile_relative).unwrap();
        let error = ProfileLock::acquire(&root, key).unwrap_err();
        assert!(matches!(error, ProfileLockError::Contended { .. }));
        drop(migration_lock);

        // And a held profile lock blocks a migration of the same profile.
        let profile_lock = ProfileLock::acquire(&root, key).unwrap();
        let error = MigrationLock::acquire_target(&root, &profile_relative).unwrap_err();
        assert!(matches!(error, MigrationError::LockContended { .. }));
        drop(profile_lock);
    }
}

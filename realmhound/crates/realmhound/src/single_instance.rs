//! Single-instance enforcement.
//!
//! Claims a system-wide token on startup. If another instance already holds it,
//! shows a message and exits. When launched as a relaunch replacement
//! (`--updated` or `--relaunch`), retries briefly to allow the old process to
//! exit.
//!
//! Windows uses a named mutex, which the kernel drops even when the process is
//! killed. Unix has no equivalent name space, so an advisory `flock` on a file
//! under the storage root stands in: that lock is likewise released by the
//! kernel on exit, so a crashed instance never leaves a stale claim behind.

/// Maximum time to wait for the old process to release the claim after a self-update.
const UPDATE_RETRY_DURATION: std::time::Duration = std::time::Duration::from_secs(5);
const UPDATE_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

#[cfg(windows)]
pub use windows_impl::SingleInstanceGuard;
#[cfg(windows)]
use windows_impl::{show_already_running_message, try_acquire};

#[cfg(not(windows))]
pub use unix_impl::SingleInstanceGuard;
#[cfg(not(windows))]
use unix_impl::{show_already_running_message, try_acquire};

/// Attempts to acquire the single-instance claim.
/// If another instance is already running, shows a message and exits.
pub fn acquire_or_exit() -> SingleInstanceGuard {
    let is_post_update = std::env::args().any(|a| a == "--updated" || a == "--relaunch");

    if is_post_update {
        // After a self-update, retry briefly while the old process exits.
        let deadline = std::time::Instant::now() + UPDATE_RETRY_DURATION;
        loop {
            if let Some(guard) = try_acquire() {
                return guard;
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(UPDATE_RETRY_INTERVAL);
        }
    } else if let Some(guard) = try_acquire() {
        return guard;
    }

    show_already_running_message();
    std::process::exit(0);
}

#[cfg(windows)]
mod windows_impl {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};

    const MUTEX_NAME: &str = "Global\\RealmHound_SingleInstance_v1";

    /// RAII guard that holds the mutex for the lifetime of the process.
    pub struct SingleInstanceGuard {
        handle: HANDLE,
    }

    impl Drop for SingleInstanceGuard {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }

    /// Try once to create/acquire the mutex. Returns the guard on success.
    pub fn try_acquire() -> Option<SingleInstanceGuard> {
        let wide_name: Vec<u16> = MUTEX_NAME
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let handle = unsafe { CreateMutexW(std::ptr::null(), 1, wide_name.as_ptr()) };

        if handle == 0 {
            // Mutex creation failed -- let the app start anyway
            return Some(SingleInstanceGuard { handle: 0 });
        }

        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe { CloseHandle(handle) };
            return None;
        }

        Some(SingleInstanceGuard { handle })
    }

    pub fn show_already_running_message() {
        let title: Vec<u16> = "RealmHound\0".encode_utf16().collect();
        let msg: Vec<u16> = "RealmHound is already running.\0".encode_utf16().collect();

        unsafe {
            MessageBoxW(0, msg.as_ptr(), title.as_ptr(), MB_OK | MB_ICONINFORMATION);
        }
    }
}

#[cfg(not(windows))]
mod unix_impl {
    use fs2::FileExt;
    use std::fs::File;
    use std::path::PathBuf;

    /// Lock file name, versioned like the Windows mutex so a later layout change
    /// can never be mistaken for a running instance.
    const LOCK_FILE_NAME: &str = "single_instance_v1.lock";

    /// RAII guard holding the advisory lock for the lifetime of the process.
    /// The kernel releases the lock when it closes the descriptor, so holding
    /// the open file is the whole mechanism -- including on an abnormal exit.
    pub struct SingleInstanceGuard {
        _file: Option<File>,
    }

    /// Where the lock file lives. The storage root is preferred so the claim is
    /// per-user and out of reach of temp-directory cleaners; the temp directory
    /// is the fallback for the same reasons `data_local_dir` can be unavailable.
    fn lock_path() -> PathBuf {
        let dir = dirs::data_local_dir()
            .map(|dir| dir.join("RealmHound"))
            .filter(|dir| std::fs::create_dir_all(dir).is_ok())
            .unwrap_or_else(std::env::temp_dir);
        dir.join(LOCK_FILE_NAME)
    }

    /// Try once to take the advisory lock. Returns the guard on success.
    pub fn try_acquire() -> Option<SingleInstanceGuard> {
        try_acquire_at(lock_path())
    }

    /// Claim the lock at an explicit path, so a test never contends with the
    /// real installation's claim.
    pub(super) fn try_acquire_at(path: PathBuf) -> Option<SingleInstanceGuard> {
        let Ok(file) = File::options()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
        else {
            // The lock file is unusable (read-only home, full disk). Refusing to
            // launch over a bookkeeping file would be worse than starting.
            return Some(SingleInstanceGuard { _file: None });
        };

        file.try_lock_exclusive()
            .ok()
            .map(|()| SingleInstanceGuard { _file: Some(file) })
    }

    pub fn show_already_running_message() {
        rfd::MessageDialog::new()
            .set_title("RealmHound")
            .set_description("RealmHound is already running.")
            .set_level(rfd::MessageLevel::Info)
            .show();
    }
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::unix_impl::try_acquire_at;

    #[test]
    fn a_second_claim_is_refused_while_the_first_is_held() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("single_instance.lock");

        let first = try_acquire_at(path.clone()).expect("first instance claims the lock");
        assert!(
            try_acquire_at(path.clone()).is_none(),
            "a second instance must not claim the lock"
        );
        drop(first);
        assert!(
            try_acquire_at(path).is_some(),
            "the lock must be reclaimable once released"
        );
    }
}

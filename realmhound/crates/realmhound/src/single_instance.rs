//! Single-instance enforcement.
//!
//! Claims a system-wide token on startup. If another instance already holds it,
//! shows a message and exits; if the lock file cannot be opened or locked at
//! all, reports that reason instead of starting unprotected. When launched as a
//! relaunch replacement (`--updated` or `--relaunch`), retries briefly to allow
//! the old process to exit.
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
use windows_impl::{show_already_running_message, show_lock_failure_message, try_acquire};

#[cfg(not(windows))]
pub use unix_impl::SingleInstanceGuard;
#[cfg(not(windows))]
use unix_impl::{show_already_running_message, show_lock_failure_message, try_acquire};

/// The outcome of one attempt to claim the single-instance token.
///
/// A failure to evaluate the claim is deliberately not folded into either of
/// the other two: starting anyway would leave a second launch unprotected, and
/// reporting "already running" would blame the wrong thing.
enum Claim {
    /// The claim is ours, held by the guard for the life of the process.
    Acquired(SingleInstanceGuard),
    /// Another RealmHound instance holds the claim.
    AlreadyRunning,
    /// The claim could not be evaluated -- permissions, filesystem, OS error.
    /// Only the lock-file platforms produce this; the Windows mutex starts the
    /// app anyway when it cannot be created.
    #[cfg_attr(windows, allow(dead_code))]
    Failed(String),
}

/// Attempts to acquire the single-instance claim.
/// If another instance is already running, or the claim cannot be evaluated at
/// all, shows the matching message and exits.
pub fn acquire_or_exit() -> SingleInstanceGuard {
    let is_post_update = std::env::args().any(|a| a == "--updated" || a == "--relaunch");

    let claim = if is_post_update {
        // After a self-update, retry briefly while the old process exits. Only
        // contention clears itself by waiting, so a real failure breaks out.
        let deadline = std::time::Instant::now() + UPDATE_RETRY_DURATION;
        loop {
            match try_acquire() {
                Claim::AlreadyRunning if std::time::Instant::now() < deadline => {
                    std::thread::sleep(UPDATE_RETRY_INTERVAL);
                }
                settled => break settled,
            }
        }
    } else {
        try_acquire()
    };

    match claim {
        Claim::Acquired(guard) => guard,
        Claim::AlreadyRunning => {
            show_already_running_message();
            std::process::exit(0);
        }
        Claim::Failed(details) => {
            show_lock_failure_message(&details);
            std::process::exit(1);
        }
    }
}

#[cfg(windows)]
mod windows_impl {
    use super::Claim;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK,
    };

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

    /// Try once to create/acquire the mutex.
    pub fn try_acquire() -> Claim {
        let wide_name: Vec<u16> = MUTEX_NAME
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let handle = unsafe { CreateMutexW(std::ptr::null(), 1, wide_name.as_ptr()) };

        if handle == 0 {
            // Mutex creation failed -- let the app start anyway
            return Claim::Acquired(SingleInstanceGuard { handle: 0 });
        }

        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe { CloseHandle(handle) };
            return Claim::AlreadyRunning;
        }

        Claim::Acquired(SingleInstanceGuard { handle })
    }

    pub fn show_already_running_message() {
        let title: Vec<u16> = "RealmHound\0".encode_utf16().collect();
        let msg: Vec<u16> = "RealmHound is already running.\0".encode_utf16().collect();

        unsafe {
            MessageBoxW(0, msg.as_ptr(), title.as_ptr(), MB_OK | MB_ICONINFORMATION);
        }
    }

    /// Unreachable on Windows -- the mutex path never reports `Failed` -- but the
    /// outcome is shared with the platforms whose claim can fail outright.
    pub fn show_lock_failure_message(details: &str) {
        let title: Vec<u16> = "RealmHound\0".encode_utf16().collect();
        let msg: Vec<u16> = format!(
            "RealmHound could not check whether another instance is running, \
             so it did not start.\n\n{details}\0"
        )
        .encode_utf16()
        .collect();

        unsafe {
            MessageBoxW(0, msg.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR);
        }
    }
}

#[cfg(not(windows))]
mod unix_impl {
    use super::Claim;
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
        _file: File,
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

    /// Whether the error is the kernel reporting the lock as held by someone
    /// else, as opposed to the lock attempt itself failing.
    fn is_contended(error: &std::io::Error) -> bool {
        error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
    }

    /// Try once to take the advisory lock.
    pub fn try_acquire() -> Claim {
        try_acquire_at(lock_path())
    }

    /// Claim the lock at an explicit path, so a test never contends with the
    /// real installation's claim.
    pub(super) fn try_acquire_at(path: PathBuf) -> Claim {
        let file = match File::options()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
        {
            Ok(file) => file,
            Err(e) => {
                return Claim::Failed(format!(
                    "The lock file {} could not be opened: {e}",
                    path.display()
                ))
            }
        };

        match file.try_lock_exclusive() {
            Ok(()) => Claim::Acquired(SingleInstanceGuard { _file: file }),
            Err(e) if is_contended(&e) => Claim::AlreadyRunning,
            Err(e) => Claim::Failed(format!(
                "The lock file {} could not be locked: {e}",
                path.display()
            )),
        }
    }

    pub fn show_already_running_message() {
        rfd::MessageDialog::new()
            .set_title("RealmHound")
            .set_description("RealmHound is already running.")
            .set_level(rfd::MessageLevel::Info)
            .show();
    }

    pub fn show_lock_failure_message(details: &str) {
        rfd::MessageDialog::new()
            .set_title("RealmHound")
            .set_description(format!(
                "RealmHound could not check whether another instance is running, \
                 so it did not start.\n\n{details}"
            ))
            .set_level(rfd::MessageLevel::Error)
            .show();
    }
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::unix_impl::try_acquire_at;
    use super::Claim;

    #[test]
    fn a_second_claim_is_refused_while_the_first_is_held() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("single_instance.lock");

        let first = try_acquire_at(path.clone());
        assert!(
            matches!(first, Claim::Acquired(_)),
            "first instance claims the lock"
        );
        assert!(
            matches!(try_acquire_at(path.clone()), Claim::AlreadyRunning),
            "a second instance must not claim the lock"
        );
        drop(first);
        assert!(
            matches!(try_acquire_at(path), Claim::Acquired(_)),
            "the lock must be reclaimable once released"
        );
    }

    #[test]
    fn an_unusable_lock_file_is_not_reported_as_another_instance() {
        let temp = tempfile::tempdir().expect("temp dir");
        // No such directory, so the lock file can never be opened.
        let path = temp.path().join("absent").join("single_instance.lock");

        assert!(
            matches!(try_acquire_at(path), Claim::Failed(_)),
            "an unopenable lock file must surface as a failure, not as a running instance"
        );
    }
}

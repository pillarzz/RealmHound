//! Single-instance enforcement using a Windows named mutex.
//!
//! Creates a system-wide mutex on startup. If another instance already holds it,
//! shows a message box and exits. When launched as a relaunch replacement
//! (`--updated` or `--relaunch`), retries briefly to allow the old process to
//! exit.

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};

const MUTEX_NAME: &str = "Global\\RealmHound_SingleInstance_v1";

/// Maximum time to wait for the old process to release the mutex after a self-update.
const UPDATE_RETRY_DURATION: std::time::Duration = std::time::Duration::from_secs(5);
const UPDATE_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

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

/// Attempts to acquire the single-instance mutex.
/// If another instance is already running, shows a message box and exits.
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

/// Try once to create/acquire the mutex. Returns the guard on success.
fn try_acquire() -> Option<SingleInstanceGuard> {
    let wide_name: Vec<u16> = MUTEX_NAME
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let handle = unsafe { CreateMutexW(std::ptr::null(), 1, wide_name.as_ptr()) };

    if handle == 0 {
        // Mutex creation failed — let the app start anyway
        return Some(SingleInstanceGuard { handle: 0 });
    }

    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe { CloseHandle(handle) };
        return None;
    }

    Some(SingleInstanceGuard { handle })
}

fn show_already_running_message() {
    let title: Vec<u16> = "RealmHound\0".encode_utf16().collect();
    let msg: Vec<u16> = "RealmHound is already running.\0".encode_utf16().collect();

    unsafe {
        MessageBoxW(0, msg.as_ptr(), title.as_ptr(), MB_OK | MB_ICONINFORMATION);
    }
}

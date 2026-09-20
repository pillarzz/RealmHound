//! Shared relaunch primitives: reason vocabulary, replacement spawn, and the
//! wait-for-old-process helper. Graceful shutdown is owned by the app.

use std::path::Path;
use std::process::{Child, Command};
use std::time::Duration;

use crate::account::AccountKey;

/// Command-line flag marking a process launched as a relaunch replacement.
pub const RELAUNCH_FLAG: &str = "--relaunch";
/// Command-line flag carrying the previous process id to wait on.
pub const WAIT_PID_FLAG: &str = "--wait-pid";

/// Why RealmHound is relaunching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelaunchReason {
    /// Applying a downloaded self-update.
    Updated,
    /// Switching to another known account profile.
    AccountSwitch(AccountKey),
    /// Entering discovery mode to identify and add a new account.
    AccountDiscovery,
    /// Restarting into a newly discovered account profile.
    AccountDiscovered(AccountKey),
    /// The active account was deleted; restart into the next profile or
    /// discovery mode.
    AccountDeletion,
}

/// Spawn the replacement, telling it to wait for `wait_pid` to exit before
/// acquiring the mutex and profile lock. The reason is not encoded in args: it
/// affects what the current process commits before exit, not how the new one starts.
pub fn spawn_replacement(exe: &Path, wait_pid: u32) -> std::io::Result<Child> {
    Command::new(exe)
        .arg(RELAUNCH_FLAG)
        .arg(WAIT_PID_FLAG)
        .arg(wait_pid.to_string())
        .spawn()
}

/// Wait up to `timeout` for `pid` to exit; returns immediately if already gone.
#[cfg(windows)]
pub fn wait_for_process_exit(pid: u32, timeout: Duration) {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };

    // Null handle means the process is already gone.
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if handle == 0 {
        return;
    }
    let millis = timeout.as_millis().min(u32::MAX as u128) as u32;
    let result = unsafe { WaitForSingleObject(handle, millis) };
    unsafe { CloseHandle(handle) };
    if result == WAIT_TIMEOUT {
        tracing::warn!("[RELAUNCH] timed out waiting for pid {pid} to exit");
    }
}

/// Off Windows the profile lock wait is the correctness guard; no process wait.
#[cfg(not(windows))]
pub fn wait_for_process_exit(_pid: u32, _timeout: Duration) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_replacement_surfaces_launch_failure() {
        let missing = std::path::Path::new("this-executable-does-not-exist-4f3a.exe");
        assert!(spawn_replacement(missing, 1234).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn wait_for_process_exit_returns_immediately_for_dead_pid() {
        let start = std::time::Instant::now();
        wait_for_process_exit(0xFFFF_FFFC, Duration::from_secs(30));
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}

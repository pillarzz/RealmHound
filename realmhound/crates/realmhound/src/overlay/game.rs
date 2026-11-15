//! Tracks the game client window that owns RealmHound's verified main
//! connection.
//!
//! Overlays are in-game furniture: they belong on screen exactly while the
//! game window that produced the tracked socket is visible and unminimized,
//! and either the linked game or RealmHound is foreground. Unknown attachment or
//! inspection failures hide them.

use std::time::{Duration, Instant};

use realmhound_core::stream::ConnectionKey;

use super::platform::{self, Process};

/// How often to retry a failed process/window lookup, in either direction.
const LOOKUP_INTERVAL: Duration = Duration::from_millis(500);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The game client associated with the current session.
///
/// The association is deliberately sticky: the main connection changes on
/// every area change, but the client process does not. A process handle is
/// retained until the process exits, and only then is the next connection
/// resolved into a new association.
#[derive(Default)]
pub(super) struct GameWindow {
    pid: Option<u32>,
    process: Option<Process>,
    window: Option<isize>,
    next_process: Option<Instant>,
    next_window: Option<Instant>,
    next_poll: Option<Instant>,
    last_error: Option<String>,
    active: bool,
}

impl GameWindow {
    /// Poll the attached client and return whether its game window is on screen
    /// and either the linked game or RealmHound is foreground right now.
    pub fn poll(&mut self, connection: Option<ConnectionKey>) -> bool {
        let now = Instant::now();
        if self.next_poll.is_some_and(|at| at > now) {
            return self.active;
        }
        self.next_poll = Some(now + POLL_INTERVAL);
        self.active = false;
        if let Some(process) = self.process.as_ref() {
            match process.is_alive() {
                Ok(true) => {}
                Ok(false) => self.detach(),
                Err(error) => {
                    // Unreadable evidence is not an exit: retain the verified
                    // association, but never show an uninspected window.
                    self.fail(error);
                    return false;
                }
            }
        }

        if self.process.is_none() {
            self.resolve_process(connection);
        }
        if let Some(pid) = self.pid {
            if self.window.is_none() {
                self.find_window(pid);
            }
        }

        let (Some(window), Some(pid)) = (self.window, self.pid) else {
            return false;
        };
        // Catch minimize/focus/exit transitions even without a connection event.
        match platform::game_window_active(window, pid) {
            Ok(Some(active)) => {
                self.active = active;
                self.last_error = None;
            }
            Ok(None) => {
                self.window = None;
                self.next_window = None;
                self.active = false;
            }
            Err(error) => self.fail(error),
        }
        self.active
    }

    fn resolve_process(&mut self, connection: Option<ConnectionKey>) {
        let Some(connection) = connection else {
            return;
        };
        if !due(&mut self.next_process) {
            return;
        }
        match crate::client_process::connection_process_id(&connection) {
            Ok(Some(pid)) => match Process::open(pid) {
                Ok(process) => {
                    self.pid = Some(pid);
                    self.process = Some(process);
                    self.window = None;
                    self.next_process = None;
                    self.next_window = None;
                    self.last_error = None;
                }
                Err(error) => self.fail(error),
            },
            Ok(None) => {}
            Err(error) => self.fail(error),
        }
    }

    fn find_window(&mut self, pid: u32) {
        if !due(&mut self.next_window) {
            return;
        }
        match platform::game_window(pid) {
            Ok(Some(window)) => {
                self.window = Some(window);
                self.next_window = None;
                self.last_error = None;
            }
            Ok(None) => {}
            Err(error) => self.fail(error),
        }
    }

    fn detach(&mut self) {
        self.pid = None;
        self.process = None;
        self.window = None;
        self.next_process = None;
        self.next_window = None;
        self.active = false;
    }

    fn fail(&mut self, error: String) {
        if self.last_error.as_deref() != Some(error.as_str()) {
            tracing::warn!("[OVERLAY] {error}");
            self.last_error = Some(error);
        }
        self.active = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polls_reuse_cached_visibility_until_the_native_check_is_due() {
        let mut game = GameWindow {
            active: true,
            next_poll: Some(Instant::now() + Duration::from_secs(60)),
            ..Default::default()
        };
        let deadline = game.next_poll;
        assert!(game.poll(None));
        assert_eq!(game.next_poll, deadline);
        let before = Instant::now();
        game.next_poll = Some(before - POLL_INTERVAL);
        assert!(!game.poll(None));
        assert!(game.next_poll.unwrap() >= before + POLL_INTERVAL);
    }

    #[test]
    fn unbound_polls_preserve_process_retry_deadline() {
        let mut game = GameWindow::default();
        assert!(due(&mut game.next_process));
        let deadline = game.next_process;
        let connection =
            ConnectionKey::new([127, 0, 0, 1].into(), 50_000, [127, 0, 0, 2].into(), 2050);
        for connection in [None, Some(connection), None] {
            assert!(!game.poll(connection));
            assert_eq!(game.next_process, deadline);
        }
        assert!(!due(&mut game.next_process));
        game.next_process = Some(Instant::now() - LOOKUP_INTERVAL);
        assert!(due(&mut game.next_process));
    }

    #[test]
    fn inspection_failure_hides_without_forgetting_attachment() {
        let mut game = GameWindow {
            pid: Some(123),
            window: Some(456),
            active: true,
            ..Default::default()
        };
        game.fail("inspection unavailable".to_string());
        assert!(!game.active);
        assert_eq!(game.pid, Some(123));
        assert_eq!(game.window, Some(456));
        game.detach();
        assert_eq!(game.pid, None);
        assert_eq!(game.window, None);
        assert!(!game.active);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn live_process_stays_linked_across_socket_changes() {
        let pid = std::process::id();
        let mut game = GameWindow {
            pid: Some(pid),
            process: Some(Process::open(pid).unwrap()),
            next_window: Some(Instant::now() + LOOKUP_INTERVAL),
            ..Default::default()
        };
        for port in [None, Some(50_000), Some(50_001)] {
            let connection = port.map(|port| {
                ConnectionKey::new([127, 0, 0, 1].into(), port, [127, 0, 0, 2].into(), 2050)
            });
            assert!(!game.poll(connection));
            assert_eq!(game.pid, Some(pid));
            assert!(game.process.is_some());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn exited_process_clears_the_sticky_link() {
        use std::process::{Command, Stdio};

        let mut child = Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let mut game = GameWindow {
            pid: Some(pid),
            process: Some(Process::open(pid).unwrap()),
            next_window: Some(Instant::now() + LOOKUP_INTERVAL),
            ..Default::default()
        };
        assert!(!game.poll(None));
        assert_eq!(game.pid, Some(pid));
        drop(child.stdin.take());
        child.wait().unwrap();
        game.next_poll = None;
        assert!(!game.poll(None));
        assert_eq!(game.pid, None);
        assert!(game.process.is_none());
    }
}

fn due(next: &mut Option<Instant>) -> bool {
    if next.is_some_and(|at| at > Instant::now()) {
        return false;
    }
    *next = Some(Instant::now() + LOOKUP_INTERVAL);
    true
}

//! Tracks the game client window that owns RealmHound's verified main
//! connection.
//!
//! Overlays are in-game furniture: they belong on screen exactly while the
//! game window that produced the tracked socket is visible and focused, and
//! hide when the user alt-tabs away, minimizes the client, or the client
//! exits.

use std::time::{Duration, Instant};

use realmhound_core::stream::ConnectionKey;

use super::platform::{self, Process};

/// How often to retry a failed process/window lookup, in either direction.
const LOOKUP_INTERVAL: Duration = Duration::from_millis(500);

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
    last_error: Option<String>,
    active: bool,
}

impl GameWindow {
    /// Poll the attached client and return whether its game window is on screen
    /// and focused right now.
    pub fn poll(&mut self, connection: Option<ConnectionKey>) -> bool {
        match self.process.as_ref().map_or(Ok(false), Process::is_alive) {
            Ok(true) => {}
            Ok(false) => self.detach(),
            Err(error) => {
                self.detach();
                self.fail(error);
            }
        }

        if self.process.is_none() {
            self.resolve_process(connection);
        }
        if let Some(pid) = self.pid {
            if self.window.is_none() {
                self.find_window(pid);
            }
        } else {
            self.active = false;
            return false;
        }

        let (Some(window), Some(pid)) = (self.window, self.pid) else {
            self.active = false;
            return false;
        };
        // Re-validating the cached handle is cheap and catches a client that
        // was minimized, covered, or closed without a connection event.
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

fn due(next: &mut Option<Instant>) -> bool {
    if next.is_some_and(|at| at > Instant::now()) {
        return false;
    }
    *next = Some(Instant::now() + LOOKUP_INTERVAL);
    true
}

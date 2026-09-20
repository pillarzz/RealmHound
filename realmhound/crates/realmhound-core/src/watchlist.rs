//! Watchlist management for party member checking.
//!
//! Handles loading, saving, and checking players against ban/watch lists.

use chrono::Utc;
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

/// Separator line between ban list and watch list in the file.
const WATCHLIST_SEPARATOR: &str = "On watchlist:";

/// Player status after checking against the watchlist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerStatus {
    /// Player is on the ban list
    Banned,
    /// Player is on the watch list
    Watched,
    /// Player is not on any list
    Normal,
}

/// Watchlist containing banned and watched player names.
#[derive(Debug, Clone, Default)]
pub struct Watchlist {
    /// Set of banned player names (case-sensitive)
    banned: HashSet<String>,
    /// Set of watched player names (case-sensitive)
    watched: HashSet<String>,
    /// Global entries file (explicit). `None` disables saving.
    list_path: Option<PathBuf>,
    /// Optional per-account detection log (explicit). `None` disables logging.
    detection_log: Option<PathBuf>,
}

impl Watchlist {
    /// Create a new empty watchlist with no bound paths.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load the watchlist entries from an explicit global path and bind an
    /// optional per-account detection log.
    ///
    /// Entries stay global; only the detection output is account-scoped.
    pub fn load_from(list_path: PathBuf, detection_log: Option<PathBuf>) -> Self {
        let mut watchlist = Self::load_from_file(&list_path).unwrap_or_default();
        watchlist.list_path = Some(list_path);
        watchlist.detection_log = detection_log;
        watchlist
    }

    /// Load watchlist entries from a specific file path.
    pub fn load_from_file(path: &PathBuf) -> Result<Self, std::io::Error> {
        let content = fs::read_to_string(path)?;
        Ok(Self::parse(&content))
    }

    /// Parse watchlist content from a string.
    pub fn parse(content: &str) -> Self {
        let mut banned = HashSet::new();
        let mut watched = HashSet::new();
        let mut in_watchlist_section = false;

        for line in content.lines() {
            let trimmed = line.trim();

            // Skip empty lines
            if trimmed.is_empty() {
                continue;
            }

            // Check for separator
            if trimmed == WATCHLIST_SEPARATOR {
                in_watchlist_section = true;
                continue;
            }

            // Add to appropriate set
            if in_watchlist_section {
                watched.insert(trimmed.to_string());
            } else {
                banned.insert(trimmed.to_string());
            }
        }

        Self {
            banned,
            watched,
            ..Self::default()
        }
    }

    /// Replace the banned/watched entries from text, preserving the bound
    /// global entries path and per-account detection log.
    pub fn replace_entries(&mut self, content: &str) {
        let parsed = Self::parse(content);
        self.banned = parsed.banned;
        self.watched = parsed.watched;
    }

    /// Save watchlist to its bound global entries path.
    pub fn save(&self) -> Result<(), std::io::Error> {
        if let Some(path) = &self.list_path {
            self.save_to_file(path)
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "watchlist has no bound entries path",
            ))
        }
    }

    /// Save watchlist to a specific file path.
    pub fn save_to_file(&self, path: &PathBuf) -> Result<(), std::io::Error> {
        // Ensure directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let content = self.to_string();
        fs::write(path, content)
    }

    /// Convert watchlist to string format.
    pub fn to_string(&self) -> String {
        let mut lines = Vec::new();

        // Add banned players (sorted for consistency)
        let mut banned_sorted: Vec<_> = self.banned.iter().collect();
        banned_sorted.sort();
        for name in banned_sorted {
            lines.push(name.clone());
        }

        // Add separator if there are watched players
        if !self.watched.is_empty() {
            lines.push(WATCHLIST_SEPARATOR.to_string());

            // Add watched players (sorted for consistency)
            let mut watched_sorted: Vec<_> = self.watched.iter().collect();
            watched_sorted.sort();
            for name in watched_sorted {
                lines.push(name.clone());
            }
        }

        lines.join("\n")
    }

    /// Check if a player is banned (case-sensitive).
    pub fn is_banned(&self, name: &str) -> bool {
        self.banned.contains(name)
    }

    /// Check if a player is watched (case-sensitive).
    pub fn is_watched(&self, name: &str) -> bool {
        self.watched.contains(name)
    }

    /// Get the status of a player (Banned takes precedence over Watched).
    pub fn check_player(&self, name: &str) -> PlayerStatus {
        if self.is_banned(name) {
            PlayerStatus::Banned
        } else if self.is_watched(name) {
            PlayerStatus::Watched
        } else {
            PlayerStatus::Normal
        }
    }

    /// Check a player and log if detected on any list.
    pub fn check_and_log(&self, name: &str) -> PlayerStatus {
        let status = self.check_player(name);

        if status != PlayerStatus::Normal {
            self.log_detection(name, status);
        }

        status
    }

    /// Log a detection event to the bound per-account detection log, if any.
    pub fn log_detection(&self, name: &str, status: PlayerStatus) {
        let Some(path) = &self.detection_log else {
            return;
        };
        // Ensure directory exists
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%S");
        let status_str = match status {
            PlayerStatus::Banned => "BANNED",
            PlayerStatus::Watched => "WATCHED",
            PlayerStatus::Normal => return, // Don't log normal players
        };

        let log_line = format!("[{}] {}: {}\n", timestamp, status_str, name);

        // Append to log file
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = file.write_all(log_line.as_bytes());
        }
    }

    /// Get the number of banned players.
    pub fn banned_count(&self) -> usize {
        self.banned.len()
    }

    /// Get the number of watched players.
    pub fn watched_count(&self) -> usize {
        self.watched.len()
    }

    /// Check if the watchlist is empty.
    pub fn is_empty(&self) -> bool {
        self.banned.is_empty() && self.watched.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty() {
        let wl = Watchlist::parse("");
        assert!(wl.is_empty());
    }

    #[test]
    fn test_parse_banned_only() {
        let content = "Player1\nPlayer2\nPlayer3";
        let wl = Watchlist::parse(content);
        assert_eq!(wl.banned_count(), 3);
        assert_eq!(wl.watched_count(), 0);
        assert!(wl.is_banned("Player1"));
        assert!(wl.is_banned("Player2"));
        assert!(wl.is_banned("Player3"));
    }

    #[test]
    fn test_parse_full() {
        let content = "Banned1\nBanned2\nOn watchlist:\nWatched1\nWatched2";
        let wl = Watchlist::parse(content);
        assert_eq!(wl.banned_count(), 2);
        assert_eq!(wl.watched_count(), 2);
        assert!(wl.is_banned("Banned1"));
        assert!(wl.is_banned("Banned2"));
        assert!(wl.is_watched("Watched1"));
        assert!(wl.is_watched("Watched2"));
    }

    #[test]
    fn test_case_sensitive() {
        let content = "Player1\nOn watchlist:\nPlayer2";
        let wl = Watchlist::parse(content);
        assert!(wl.is_banned("Player1"));
        assert!(!wl.is_banned("player1")); // Case sensitive
        assert!(wl.is_watched("Player2"));
        assert!(!wl.is_watched("PLAYER2")); // Case sensitive
    }

    #[test]
    fn test_check_player_precedence() {
        let mut wl = Watchlist::new();
        wl.banned.insert("DualList".to_string());
        wl.watched.insert("DualList".to_string());

        // Banned takes precedence
        assert_eq!(wl.check_player("DualList"), PlayerStatus::Banned);
    }

    #[test]
    fn test_roundtrip() {
        let content = "Banned1\nBanned2\nOn watchlist:\nWatched1";
        let wl = Watchlist::parse(content);
        let output = wl.to_string();
        let wl2 = Watchlist::parse(&output);

        assert_eq!(wl.banned_count(), wl2.banned_count());
        assert_eq!(wl.watched_count(), wl2.watched_count());
    }

    #[test]
    fn load_from_binds_explicit_paths_and_logs_detections() {
        let temp = tempfile::tempdir().unwrap();
        let list = temp.path().join("watchlist.txt");
        fs::write(&list, "Banned1\nOn watchlist:\nWatched1").unwrap();
        let detection = temp.path().join("logs").join("watchlist_detections.log");

        let wl = Watchlist::load_from(list.clone(), Some(detection.clone()));
        assert!(wl.is_banned("Banned1"));
        assert!(wl.is_watched("Watched1"));

        // A detection is written to the explicit per-account log path.
        assert_eq!(wl.check_and_log("Banned1"), PlayerStatus::Banned);
        let logged = fs::read_to_string(&detection).unwrap();
        assert!(logged.contains("BANNED: Banned1"), "log was: {logged}");

        // No detection log bound => no file written, and normal players never log.
        let unbound = Watchlist::load_from(list, None);
        assert_eq!(unbound.check_and_log("Someone"), PlayerStatus::Normal);
    }

    #[test]
    fn replace_entries_preserves_bound_paths_for_save() {
        let temp = tempfile::tempdir().unwrap();
        let list = temp.path().join("watchlist.txt");

        let mut wl = Watchlist::load_from(list.clone(), None);
        wl.replace_entries("Banned1\nOn watchlist:\nWatched1");
        wl.save().expect("save should use the bound path");

        let reloaded = Watchlist::load_from(list, None);
        assert!(reloaded.is_banned("Banned1"));
        assert!(reloaded.is_watched("Watched1"));
    }

    #[test]
    fn save_without_bound_path_errors() {
        let wl = Watchlist::parse("Banned1");
        assert!(wl.save().is_err());
    }
}

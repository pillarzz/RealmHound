//! Update checker and self-updater for RealmHound
//!
//! Checks for new versions by fetching a version manifest from a public gist.
//! Runs on startup and periodically (every 2 hours) while the app is running.
//! Supports in-place self-update: download, swap exe, and relaunch.

use serde::Deserialize;
use std::cmp::Ordering;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// URL to the version manifest gist
const VERSION_MANIFEST_URL: &str =
    "https://gist.githubusercontent.com/pillarzz/4f50870a65c14d179eb2c361b5915946/raw/version.json";

/// Timeout for HTTP requests
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Default check interval (2 hours)
const DEFAULT_CHECK_INTERVAL: Duration = Duration::from_secs(2 * 60 * 60);

/// Version manifest fetched from the gist
#[derive(Debug, Clone, Deserialize)]
pub struct VersionManifest {
    pub latest_version: String,
    pub download_url: String,
    /// SHA-256 hex digest of the exe at download_url
    #[serde(default)]
    pub sha256: Option<String>,
    pub releases: Vec<Release>,
}

/// A single release entry in the manifest
#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub version: String,
    pub date: String,
    pub changes: Vec<String>,
}

/// Error information for failed update checks
#[derive(Debug, Clone)]
pub struct UpdateError {
    /// User-friendly message for display in the UI
    pub message: String,
    /// Technical details for debugging (logged, not shown to user)
    pub details: String,
}

impl UpdateError {
    /// Create a new update error with categorized user-friendly message
    fn new(details: String) -> Self {
        let message = if details.contains("Failed to create HTTP client") {
            "Network initialization failed".to_string()
        } else if details.contains("Failed to fetch") || details.contains("error sending request") {
            "Could not connect to update server".to_string()
        } else if details.contains("status:") {
            "Update server returned an error".to_string()
        } else if details.contains("Failed to parse") {
            "Invalid response from update server".to_string()
        } else if details.contains("disconnected") {
            "Update check was interrupted".to_string()
        } else {
            "Update check failed".to_string()
        };
        Self { message, details }
    }
}

/// Current state of the update checker
#[derive(Debug, Clone)]
pub enum UpdateState {
    /// Haven't checked yet
    Unknown,
    /// Currently checking for updates
    Checking,
    /// App is up to date
    UpToDate,
    /// A new version is available
    UpdateAvailable {
        manifest: VersionManifest,
        /// Releases between current version and latest
        new_releases: Vec<Release>,
    },
    /// Failed to check (network error, parse error, etc.)
    Error(UpdateError),
}

impl Default for UpdateState {
    fn default() -> Self {
        Self::Unknown
    }
}

/// Update checker that runs on startup and periodically
pub struct UpdateChecker {
    /// Current state
    state: UpdateState,
    /// When we last checked for updates
    last_check: Option<Instant>,
    /// How often to check (default: 2 hours)
    check_interval: Duration,
    /// Whether a check is currently in progress
    check_in_progress: bool,
    /// Receiver for async check results
    result_receiver: Option<mpsc::Receiver<Result<VersionManifest, String>>>,
    /// Current app version
    current_version: String,
    /// All releases from the manifest (kept after any successful fetch)
    all_releases: Vec<Release>,
}

impl UpdateChecker {
    /// Create a new update checker
    pub fn new() -> Self {
        Self {
            state: UpdateState::Unknown,
            last_check: None,
            check_interval: DEFAULT_CHECK_INTERVAL,
            check_in_progress: false,
            result_receiver: None,
            current_version: env!("CARGO_PKG_VERSION").to_string(),
            all_releases: Vec::new(),
        }
    }

    /// Create with a custom check interval (useful for testing)
    pub fn with_interval(interval: Duration) -> Self {
        Self {
            check_interval: interval,
            ..Self::new()
        }
    }

    /// Get the current state
    pub fn state(&self) -> &UpdateState {
        &self.state
    }

    /// Get the current app version
    pub fn current_version(&self) -> &str {
        &self.current_version
    }

    /// Get all releases from the last successful manifest fetch
    pub fn all_releases(&self) -> &[Release] {
        &self.all_releases
    }

    /// Check if we should trigger an update check now
    fn should_check_now(&self) -> bool {
        if self.check_in_progress {
            return false;
        }

        match self.last_check {
            None => true, // Never checked (startup)
            Some(last) => last.elapsed() >= self.check_interval,
        }
    }

    /// Call this every frame to manage update checking
    pub fn update(&mut self) {
        // Check if we should start a new check
        if self.should_check_now() {
            self.trigger_check();
        }

        // Poll for results from async check
        self.poll_result();
    }

    /// Trigger an async update check
    fn trigger_check(&mut self) {
        if self.check_in_progress {
            return;
        }

        info!("Checking for updates...");
        self.state = UpdateState::Checking;
        self.check_in_progress = true;
        self.last_check = Some(Instant::now());

        // Create a channel for the result
        let (tx, rx) = mpsc::channel();
        self.result_receiver = Some(rx);

        // Spawn a thread to do the HTTP request (non-blocking)
        std::thread::spawn(move || {
            let result = fetch_manifest_blocking();
            let _ = tx.send(result);
        });
    }

    /// Poll for async check completion
    fn poll_result(&mut self) {
        let receiver = match &self.result_receiver {
            Some(rx) => rx,
            None => return,
        };

        // Try to receive without blocking
        match receiver.try_recv() {
            Ok(result) => {
                self.check_in_progress = false;
                self.result_receiver = None;

                match result {
                    Ok(manifest) => {
                        self.process_manifest(manifest);
                    }
                    Err(e) => {
                        warn!("Update check failed: {}", e);
                        self.state = UpdateState::Error(UpdateError::new(e));
                    }
                }
            }
            Err(mpsc::TryRecvError::Empty) => {
                // Still waiting, do nothing
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                // Thread panicked or something went wrong
                self.check_in_progress = false;
                self.result_receiver = None;
                self.state = UpdateState::Error(UpdateError::new(
                    "Update check thread disconnected".to_string(),
                ));
            }
        }
    }

    /// Process a successfully fetched manifest
    fn process_manifest(&mut self, manifest: VersionManifest) {
        self.all_releases = manifest.releases.clone();

        let comparison = compare_versions(&self.current_version, &manifest.latest_version);

        match comparison {
            Ordering::Less => {
                // Current version is older, update available
                let new_releases = get_releases_since(&manifest, &self.current_version);
                info!(
                    "Update available: {} -> {} ({} new releases)",
                    self.current_version,
                    manifest.latest_version,
                    new_releases.len()
                );
                self.state = UpdateState::UpdateAvailable {
                    manifest,
                    new_releases,
                };
            }
            Ordering::Equal | Ordering::Greater => {
                debug!("App is up to date (v{})", self.current_version);
                self.state = UpdateState::UpToDate;
            }
        }
    }

    /// Force a check regardless of timing
    pub fn force_check(&mut self) {
        self.last_check = None;
        self.update();
    }
}

impl Default for UpdateChecker {
    fn default() -> Self {
        Self::new()
    }
}

/// Fetch the version manifest (blocking, for use in a thread)
fn fetch_manifest_blocking() -> Result<VersionManifest, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let response = client
        .get(VERSION_MANIFEST_URL)
        .send()
        .map_err(|e| format!("Failed to fetch version manifest: {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "Version manifest request failed with status: {}",
            response.status()
        ));
    }

    let manifest: VersionManifest = response
        .json()
        .map_err(|e| format!("Failed to parse version manifest: {}", e))?;

    debug!(
        "Fetched version manifest: latest = {}",
        manifest.latest_version
    );
    Ok(manifest)
}

/// Compare two version strings using semver
/// Returns Ordering::Less if current < latest (update available)
pub fn compare_versions(current: &str, latest: &str) -> Ordering {
    // Try to parse as semver first
    match (
        semver::Version::parse(current),
        semver::Version::parse(latest),
    ) {
        (Ok(curr), Ok(lat)) => curr.cmp(&lat),
        _ => {
            // Fallback to string comparison if not valid semver
            warn!(
                "Could not parse versions as semver: current='{}', latest='{}'",
                current, latest
            );
            current.cmp(latest)
        }
    }
}

/// Get all releases newer than the current version
pub fn get_releases_since(manifest: &VersionManifest, current_version: &str) -> Vec<Release> {
    manifest
        .releases
        .iter()
        .filter(|release| compare_versions(current_version, &release.version) == Ordering::Less)
        .cloned()
        .collect()
}

// ─── Self-Update ─────────────────────────────────────────────────────────────

/// Progress state shared between the download thread and the UI.
#[derive(Debug, Clone)]
pub struct DownloadProgress {
    /// Bytes downloaded so far
    pub downloaded: u64,
    /// Total size in bytes (if known from Content-Length)
    pub total: Option<u64>,
}

/// State of the self-update process.
#[derive(Debug, Clone)]
pub enum SelfUpdateState {
    /// Idle, no update in progress
    Idle,
    /// Downloading the new exe
    Downloading(DownloadProgress),
    /// Download complete, ready to apply
    ReadyToApply,
    /// Update failed
    Failed(String),
}

impl Default for SelfUpdateState {
    fn default() -> Self {
        Self::Idle
    }
}

/// Manages the self-update download and apply process.
pub struct SelfUpdater {
    state: Arc<Mutex<SelfUpdateState>>,
    /// Path to the downloaded new exe (set after download completes)
    new_exe_path: Arc<Mutex<Option<std::path::PathBuf>>>,
}

impl SelfUpdater {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(SelfUpdateState::Idle)),
            new_exe_path: Arc::new(Mutex::new(None)),
        }
    }

    /// Get the current self-update state.
    pub fn state(&self) -> SelfUpdateState {
        self.state.lock().unwrap().clone()
    }

    /// Whether an update operation is in progress.
    pub fn is_busy(&self) -> bool {
        !matches!(
            *self.state.lock().unwrap(),
            SelfUpdateState::Idle | SelfUpdateState::Failed(_)
        )
    }

    /// Start downloading the update in a background thread.
    pub fn start_download(&self, download_url: String, expected_sha256: Option<String>) {
        let expected_sha256 = match expected_sha256 {
            Some(value)
                if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
            {
                value
            }
            Some(_) => {
                *self.state.lock().unwrap() =
                    SelfUpdateState::Failed("Update manifest has an invalid SHA-256".to_string());
                return;
            }
            None => {
                *self.state.lock().unwrap() =
                    SelfUpdateState::Failed("Update manifest is missing SHA-256".to_string());
                return;
            }
        };

        let state = Arc::clone(&self.state);
        let new_exe_path = Arc::clone(&self.new_exe_path);

        *state.lock().unwrap() = SelfUpdateState::Downloading(DownloadProgress {
            downloaded: 0,
            total: None,
        });

        std::thread::spawn(move || {
            match download_update(&download_url, &expected_sha256, &state) {
                Ok(path) => {
                    *new_exe_path.lock().unwrap() = Some(path);
                    *state.lock().unwrap() = SelfUpdateState::ReadyToApply;
                }
                Err(e) => {
                    warn!("Self-update download failed: {}", e);
                    *state.lock().unwrap() = SelfUpdateState::Failed(e);
                }
            }
        });
    }

    /// Rename the running exe aside and move the downloaded one into place, without
    /// spawning or exiting. On later spawn failure, call [`rollback_executable_swap`].
    pub fn stage_swap(&self) -> Result<StagedSwap, String> {
        let new_path = self
            .new_exe_path
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "No downloaded update to apply".to_string())?;
        stage_executable_swap(&new_path)
    }

    /// Reset state back to idle (e.g. after a failure the user dismisses).
    pub fn reset(&self) {
        *self.state.lock().unwrap() = SelfUpdateState::Idle;
        *self.new_exe_path.lock().unwrap() = None;
    }
}

impl Default for SelfUpdater {
    fn default() -> Self {
        Self::new()
    }
}

/// Maximum allowed download size (100 MB) to prevent unbounded memory usage.
const MAX_DOWNLOAD_SIZE: u64 = 100 * 1024 * 1024;

/// Download the update exe to a temp file next to the current exe.
fn download_update(
    url: &str,
    expected_sha256: &str,
    state: &Arc<Mutex<SelfUpdateState>>,
) -> Result<std::path::PathBuf, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let current_exe =
        std::env::current_exe().map_err(|e| format!("Cannot determine current exe path: {}", e))?;
    let exe_dir = current_exe
        .parent()
        .ok_or_else(|| "Cannot determine exe directory".to_string())?;

    let new_path = exe_dir.join("RealmHound.exe.new");

    info!("Downloading update from {} to {:?}", url, new_path);

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let mut response = client
        .get(url)
        .send()
        .map_err(|e| format!("Failed to download update: {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "Download failed with status: {}",
            response.status()
        ));
    }

    let total_size = response.content_length();
    if let Some(size) = total_size {
        if size > MAX_DOWNLOAD_SIZE {
            return Err(format!("Download too large: {} bytes", size));
        }
    }

    *state.lock().unwrap() = SelfUpdateState::Downloading(DownloadProgress {
        downloaded: 0,
        total: total_size,
    });

    let mut file = std::fs::File::create(&new_path)
        .map_err(|e| format!("Failed to create temp file: {}", e))?;

    // Stream download in chunks for progress reporting
    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;
    let mut buf = vec![0u8; 64 * 1024];

    loop {
        let n = response
            .read(&mut buf)
            .map_err(|e| format!("Read error during download: {}", e))?;
        if n == 0 {
            break;
        }

        downloaded += n as u64;
        if downloaded > MAX_DOWNLOAD_SIZE {
            let _ = std::fs::remove_file(&new_path);
            return Err("Download exceeded maximum size limit".to_string());
        }

        hasher.update(&buf[..n]);
        std::io::Write::write_all(&mut file, &buf[..n])
            .map_err(|e| format!("Failed to write update file: {}", e))?;

        *state.lock().unwrap() = SelfUpdateState::Downloading(DownloadProgress {
            downloaded,
            total: total_size,
        });
    }

    drop(file);

    let actual = format!("{:x}", hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        let _ = std::fs::remove_file(&new_path);
        return Err(format!(
            "SHA-256 mismatch: expected {}, got {}",
            expected_sha256, actual
        ));
    }
    info!("SHA-256 verified: {}", actual);

    info!("Download complete: {} bytes", downloaded);
    Ok(new_path)
}

/// A completed executable swap awaiting relaunch. Holds the paths needed to
/// roll the swap back if spawning the replacement fails.
pub struct StagedSwap {
    /// The live executable path (now holding the new exe).
    current_exe: std::path::PathBuf,
    /// The renamed-aside previous executable.
    old_path: std::path::PathBuf,
    /// Where the downloaded exe originally sat; rollback moves it back here.
    new_exe_path: std::path::PathBuf,
}

impl StagedSwap {
    /// The executable to spawn as the replacement process.
    pub fn exe_path(&self) -> &std::path::Path {
        &self.current_exe
    }
}

/// Swap the running exe with the new one, leaving the process running.
/// On Windows: rename current -> .old, rename .new -> current. On the second
/// rename failing, the first is rolled back before returning the error.
pub fn stage_executable_swap(new_exe_path: &std::path::Path) -> Result<StagedSwap, String> {
    let current_exe =
        std::env::current_exe().map_err(|e| format!("Cannot determine current exe path: {}", e))?;
    let old_path = current_exe.with_extension("exe.old");

    info!("Applying update: swapping {:?}", current_exe);

    // Remove previous .old file if it exists
    if old_path.exists() {
        std::fs::remove_file(&old_path)
            .map_err(|e| format!("Cannot remove old backup ({}): {}", old_path.display(), e))?;
    }

    // Rename running exe to .old (Windows allows renaming a running exe)
    std::fs::rename(&current_exe, &old_path)
        .map_err(|e| format!("Failed to rename current exe to .old: {}", e))?;

    // Rename .new to the original exe name
    if let Err(e) = std::fs::rename(new_exe_path, &current_exe) {
        // Rollback: restore original
        let _ = std::fs::rename(&old_path, &current_exe);
        return Err(format!("Failed to rename new exe into place: {}", e));
    }

    Ok(StagedSwap {
        current_exe,
        old_path,
        new_exe_path: new_exe_path.to_path_buf(),
    })
}

/// Undo a staged swap after a failed relaunch: move the new exe back out and
/// restore the original so the running process's on-disk image is intact.
pub fn rollback_executable_swap(staged: &StagedSwap) {
    let _ = std::fs::rename(&staged.current_exe, &staged.new_exe_path);
    let _ = std::fs::rename(&staged.old_path, &staged.current_exe);
}

/// Clean up leftover .old file from a previous update.
/// Call this on app startup.
pub fn cleanup_old_exe() {
    if let Ok(current_exe) = std::env::current_exe() {
        let old_path = current_exe.with_extension("exe.old");
        if old_path.exists() {
            match std::fs::remove_file(&old_path) {
                Ok(()) => info!("Cleaned up old exe: {:?}", old_path),
                Err(e) => debug!("Could not clean up old exe: {}", e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compare_versions() {
        assert_eq!(compare_versions("0.1.0", "0.2.0"), Ordering::Less);
        assert_eq!(compare_versions("0.2.0", "0.2.0"), Ordering::Equal);
        assert_eq!(compare_versions("0.3.0", "0.2.0"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.0", "0.9.9"), Ordering::Greater);
        assert_eq!(compare_versions("0.2.0", "0.2.1"), Ordering::Less);
    }

    #[test]
    fn test_get_releases_since() {
        let manifest = VersionManifest {
            latest_version: "0.5.0".to_string(),
            download_url: "http://example.com".to_string(),
            sha256: None,
            releases: vec![
                Release {
                    version: "0.5.0".to_string(),
                    date: "2026-01-30".to_string(),
                    changes: vec!["Feature A".to_string()],
                },
                Release {
                    version: "0.4.0".to_string(),
                    date: "2026-01-25".to_string(),
                    changes: vec!["Feature B".to_string()],
                },
                Release {
                    version: "0.3.0".to_string(),
                    date: "2026-01-20".to_string(),
                    changes: vec!["Feature C".to_string()],
                },
            ],
        };

        let releases = get_releases_since(&manifest, "0.3.0");
        assert_eq!(releases.len(), 2);
        assert_eq!(releases[0].version, "0.5.0");
        assert_eq!(releases[1].version, "0.4.0");

        let releases = get_releases_since(&manifest, "0.5.0");
        assert_eq!(releases.len(), 0);
    }

    #[test]
    fn test_legacy_manifest_without_checksum_still_parses() {
        let manifest: VersionManifest = serde_json::from_str(
            r#"{
                "latest_version": "0.20.2",
                "download_url": "http://example.com/RealmHound.exe",
                "releases": [{
                    "version": "0.20.2",
                    "date": "2026-09-12",
                    "changes": ["Bug fixes"]
                }]
            }"#,
        )
        .unwrap();

        assert_eq!(manifest.sha256, None);
        assert_eq!(manifest.releases.len(), 1);
    }

    #[test]
    fn test_missing_checksum_blocks_download() {
        let updater = SelfUpdater::new();

        updater.start_download("http://example.com".to_string(), None);

        assert!(matches!(
            updater.state(),
            SelfUpdateState::Failed(message) if message == "Update manifest is missing SHA-256"
        ));
    }

    #[test]
    fn test_invalid_checksum_blocks_download() {
        let updater = SelfUpdater::new();

        updater.start_download("http://example.com".to_string(), Some("g".repeat(64)));

        assert!(matches!(
            updater.state(),
            SelfUpdateState::Failed(message) if message == "Update manifest has an invalid SHA-256"
        ));
    }
}

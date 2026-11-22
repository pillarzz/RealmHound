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

use crate::install::InstallKind;

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
    /// Zipped macOS `RealmHound.app` bundle for this release.
    ///
    /// Absent from manifests published before macOS builds existed, which is
    /// what lets an older client keep reading this document unchanged.
    #[serde(default)]
    pub macos_download_url: Option<String>,
    /// SHA-256 hex digest of the archive at `macos_download_url`.
    #[serde(default)]
    pub macos_sha256: Option<String>,
    /// Bare x86-64 Linux GUI executable for in-app replacement.
    #[serde(default)]
    pub linux_x86_64_download_url: Option<String>,
    /// SHA-256 hex digest of the executable at `linux_x86_64_download_url`.
    #[serde(default)]
    pub linux_x86_64_sha256: Option<String>,
    /// Bare aarch64 Linux GUI executable, once that build is published.
    #[serde(default)]
    pub linux_aarch64_download_url: Option<String>,
    /// SHA-256 hex digest of the executable at `linux_aarch64_download_url`.
    #[serde(default)]
    pub linux_aarch64_sha256: Option<String>,
    /// SHA-256 hex digest of the Linux capture helper in this release.
    #[serde(default)]
    pub linux_helper_sha256: Option<String>,
    /// Oldest capture helper this GUI release works with.
    #[serde(default)]
    pub linux_min_helper_version: Option<u32>,
    pub releases: Vec<Release>,
}

/// A release artifact this platform can actually install: the Windows
/// executable, the macOS `.app` bundle as a zip, or the Linux GUI binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformArtifact {
    pub download_url: String,
    pub sha256: Option<String>,
    /// Linux only: the minimum capture-helper version this release needs.
    /// `None` for platforms without a separately installed helper, and for
    /// manifests that do not declare a requirement.
    pub min_helper_version: Option<u32>,
    /// Linux only: the official helper digest, shown when a manual helper
    /// upgrade is required.
    pub helper_sha256: Option<String>,
}

impl VersionManifest {
    /// The artifact for the platform this build runs on, or `None` when the
    /// release published nothing this platform can run.
    ///
    /// `download_url` is the Windows executable: it predates any other target
    /// and is left unqualified so older clients keep resolving it.
    pub fn artifact_for_this_platform(&self) -> Option<PlatformArtifact> {
        self.artifact_for(std::env::consts::OS, std::env::consts::ARCH)
    }

    /// Pure platform/architecture selection, so every branch is testable
    /// regardless of the host this test binary runs on.
    fn artifact_for(&self, os: &str, arch: &str) -> Option<PlatformArtifact> {
        match (os, arch) {
            ("windows", _) => Some(PlatformArtifact {
                download_url: self.download_url.clone(),
                sha256: self.sha256.clone(),
                min_helper_version: None,
                helper_sha256: None,
            }),
            ("macos", _) => Some(PlatformArtifact {
                download_url: self.macos_download_url.clone()?,
                sha256: self.macos_sha256.clone(),
                min_helper_version: None,
                helper_sha256: None,
            }),
            ("linux", "x86_64") => Some(PlatformArtifact {
                download_url: self.linux_x86_64_download_url.clone()?,
                sha256: self.linux_x86_64_sha256.clone(),
                min_helper_version: self.linux_min_helper_version,
                helper_sha256: self.linux_helper_sha256.clone(),
            }),
            ("linux", "aarch64") => Some(PlatformArtifact {
                download_url: self.linux_aarch64_download_url.clone()?,
                sha256: self.linux_aarch64_sha256.clone(),
                min_helper_version: self.linux_min_helper_version,
                helper_sha256: self.linux_helper_sha256.clone(),
            }),
            _ => None,
        }
    }
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
    /// How this copy was installed. Linux only; other platforms ignore it.
    install_kind: InstallKind,
}

impl SelfUpdater {
    pub fn new(install_kind: InstallKind) -> Self {
        Self {
            state: Arc::new(Mutex::new(SelfUpdateState::Idle)),
            new_exe_path: Arc::new(Mutex::new(None)),
            install_kind,
        }
    }

    /// Get the current self-update state.
    pub fn state(&self) -> SelfUpdateState {
        self.state.lock().unwrap().clone()
    }

    /// How this copy was installed.
    pub fn install_kind(&self) -> InstallKind {
        self.install_kind
    }

    /// Whether an update operation is in progress.
    pub fn is_busy(&self) -> bool {
        !matches!(
            *self.state.lock().unwrap(),
            SelfUpdateState::Idle | SelfUpdateState::Failed(_)
        )
    }

    /// Start downloading the update in a background thread.
    ///
    /// The manifest is vetted on every platform so a malformed or unsigned
    /// entry is reported the same way everywhere. A platform the release
    /// published no binary for is told so rather than being handed one the
    /// kernel cannot run; the update *check* is separate and still reports that
    /// a newer version exists.
    ///
    /// Linux additionally refuses package-managed and unrecognized installs,
    /// and refuses a GUI that requires a newer capture helper until that helper
    /// has been upgraded and rechecked.
    pub fn start_download(&self, artifact: Option<PlatformArtifact>) {
        let Some(artifact) = artifact else {
            *self.state.lock().unwrap() = SelfUpdateState::Failed(missing_artifact_message());
            return;
        };

        // Linux: a package-managed or unrecognized install must never replace
        // itself from upstream. This is a policy gate, not a privilege check.
        #[cfg(target_os = "linux")]
        if let Some(hint) = self.install_kind.update_hint() {
            *self.state.lock().unwrap() = SelfUpdateState::Failed(hint.to_string());
            return;
        }

        #[cfg(target_os = "linux")]
        let helper_sha256 = artifact.helper_sha256.clone();
        let download_url = artifact.download_url;
        let expected_sha256 = match artifact.sha256 {
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

        // Linux: never replace the GUI with one that needs a newer capture
        // helper. The user must upgrade the helper first and retry; silently
        // landing in incompatible capture is not an option.
        #[cfg(target_os = "linux")]
        if let Some(required) = artifact.min_helper_version {
            let installed = crate::capture::linux_helper::installed_helper_version();
            if let Err(message) =
                crate::capture::linux_helper::require_helper_version(required, installed)
            {
                let message = match helper_sha256 {
                    Some(hash) => format!("{message}\n\nOfficial helper SHA-256: {hash}"),
                    None => message,
                };
                *self.state.lock().unwrap() = SelfUpdateState::Failed(message);
                return;
            }
        }

        let state = Arc::clone(&self.state);
        let new_exe_path = Arc::clone(&self.new_exe_path);

        // macOS ships only the .app bundle, so a loose or unwritable install
        // has nothing it can swap itself into. Checked after the manifest so a
        // malformed entry is still reported the same way everywhere, but before
        // the download.
        #[cfg(target_os = "macos")]
        if let Some(blocker) = bundle_update_blocker() {
            *state.lock().unwrap() = SelfUpdateState::Failed(blocker);
            return;
        }

        #[cfg(target_os = "linux")]
        if let Some(blocker) = linux_gui_update_blocker() {
            *state.lock().unwrap() = SelfUpdateState::Failed(blocker);
            return;
        }

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
        #[cfg(target_os = "macos")]
        {
            stage_bundle_swap(&new_path)
        }
        #[cfg(not(target_os = "macos"))]
        {
            stage_executable_swap(&new_path)
        }
    }

    /// Reset state back to idle (e.g. after a failure the user dismisses).
    pub fn reset(&self) {
        *self.state.lock().unwrap() = SelfUpdateState::Idle;
        *self.new_exe_path.lock().unwrap() = None;
    }
}

impl Default for SelfUpdater {
    fn default() -> Self {
        Self::new(InstallKind::Unmanaged)
    }
}

/// Explain an update manifest that has no artifact this build can run.
fn missing_artifact_message() -> String {
    #[cfg(target_os = "linux")]
    {
        let arch = std::env::consts::ARCH;
        format!(
            "This release does not publish a Linux build for {arch}. \
             Supported Linux builds are x86-64. Update through your package \
             manager or build from source."
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        format!(
            "This release does not publish a {os}/{arch} build. Pull the latest \
             source and rebuild to update."
        )
    }
}

/// Refuse a Linux update when the account cannot replace the GUI in place.
#[cfg(target_os = "linux")]
fn linux_gui_update_blocker() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let parent = exe.parent()?;
    if can_write_in(parent) {
        return None;
    }
    Some(format!(
        "RealmHound is in {}, which this account cannot write to. Reinstall it \
         with the official release installer so in-app updates can replace it.",
        parent.display()
    ))
}

/// Maximum allowed download size (100 MB) to prevent unbounded memory usage.
const MAX_DOWNLOAD_SIZE: u64 = 100 * 1024 * 1024;

/// A sibling of `path` with `suffix` appended to the whole file name.
///
/// Appending rather than replacing the extension keeps `RealmHound.exe.old` on
/// Windows while giving the extensionless macOS binary `RealmHound.old` instead
/// of a misleading `.exe.old`.
fn sibling_with_suffix(path: &std::path::Path, suffix: &str) -> Option<std::path::PathBuf> {
    let name = path.file_name()?.to_str()?;
    Some(path.with_file_name(format!("{name}{suffix}")))
}

/// The bundle directory name macOS releases ship, and the executable inside it.
#[cfg(target_os = "macos")]
const BUNDLE_NAME: &str = "RealmHound.app";
#[cfg(target_os = "macos")]
const BUNDLE_EXE: &str = "Contents/MacOS/RealmHound";

/// The `.app` that `exe` runs from, i.e. `<bundle>/Contents/MacOS/<exe>`.
/// `None` when the executable is not inside a bundle at all.
#[cfg(target_os = "macos")]
fn bundle_root_of(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let macos_dir = exe.parent()?;
    let contents = macos_dir.parent()?;
    let bundle = contents.parent()?;
    (macos_dir.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app")
        .then(|| bundle.to_path_buf())
}

/// The installed `RealmHound.app` this process is running from, if any.
#[cfg(target_os = "macos")]
pub fn installed_bundle() -> Option<std::path::PathBuf> {
    bundle_root_of(&std::env::current_exe().ok()?)
}

/// Why this install cannot replace itself, if it cannot.
///
/// Checked before downloading, so the user is told what to fix instead of
/// spending a download on an update that could never be applied.
#[cfg(target_os = "macos")]
fn bundle_update_blocker() -> Option<String> {
    let Some(bundle) = installed_bundle() else {
        return Some(
            "RealmHound can only update itself when run from RealmHound.app. \
             Download the latest release and replace your copy."
                .to_string(),
        );
    };
    // Gatekeeper runs a quarantined bundle from a read-only copy at a random
    // path, so the bundle we can see is not the one the user installed.
    if bundle
        .components()
        .any(|part| part.as_os_str() == "AppTranslocation")
    {
        return Some(
            "macOS is running RealmHound from a temporary read-only copy. Move \
             RealmHound.app to Applications and open it from there."
                .to_string(),
        );
    }
    let parent = bundle.parent()?;
    if !can_write_in(parent) {
        return Some(format!(
            "RealmHound.app is in {}, which this account cannot write to. Move \
             it somewhere you can write to, or update it manually.",
            parent.display()
        ));
    }
    None
}

/// Whether this process can create files in `dir`. Probing is the only reliable
/// answer on Unix, where ACLs and SIP both override the mode bits.
#[cfg(unix)]
fn can_write_in(dir: &std::path::Path) -> bool {
    let probe = dir.join(format!(".realmhound-update-probe-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Phrase a filesystem failure during the swap so the user can act on it.
#[cfg(unix)]
fn swap_error(path: &std::path::Path, error: &std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem => format!(
            "No permission to replace {}. Reinstall RealmHound with the official \
             release installer, or update it manually.",
            path.display()
        ),
        std::io::ErrorKind::NotFound => format!(
            "{} is no longer where RealmHound expected it. Update it manually.",
            path.display()
        ),
        _ => format!("Could not replace {}: {}", path.display(), error),
    }
}

/// Where the downloaded artifact is written.
///
/// Beside the running executable, except for a macOS bundle install: writing
/// inside `RealmHound.app` would leave debris in a bundle we are about to
/// replace wholesale, so the archive lands beside it.
fn download_path() -> Result<std::path::PathBuf, String> {
    #[cfg(target_os = "macos")]
    if let Some(bundle) = installed_bundle() {
        return sibling_with_suffix(&bundle, ".new.zip")
            .ok_or_else(|| "Cannot determine download path".to_string());
    }
    let current_exe =
        std::env::current_exe().map_err(|e| format!("Cannot determine current exe path: {}", e))?;
    sibling_with_suffix(&current_exe, ".new")
        .ok_or_else(|| "Cannot determine download path".to_string())
}

/// Download the update binary to a temp file next to the current one.
fn download_update(
    url: &str,
    expected_sha256: &str,
    state: &Arc<Mutex<SelfUpdateState>>,
) -> Result<std::path::PathBuf, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let new_path = download_path()?;

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

    // Remove a stale partial download (including a symlink) before creating the
    // staging file, so the write can never follow a link out of the install.
    let _ = std::fs::remove_file(&new_path);
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

    // A downloaded file is not executable on Unix until it is marked so.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&new_path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("Failed to mark update executable: {}", e))?;
    }

    info!("Download complete: {} bytes", downloaded);

    // A hash match proves the bytes are the published ones, not that this host
    // can run them. Refuse a wrong-architecture or missing-library binary
    // before it ever replaces the working GUI.
    #[cfg(target_os = "linux")]
    if !downloaded_exe_runs(&new_path) {
        let _ = std::fs::remove_file(&new_path);
        return Err(
            "The downloaded update cannot run on this system. The current \
             installation was not changed."
                .to_string(),
        );
    }

    Ok(new_path)
}

/// Run a Linux GUI with `--version`, bounded so a broken binary cannot wedge
/// the caller. Returns its stdout on a successful exit.
#[cfg(target_os = "linux")]
fn run_version(path: &std::path::Path) -> Option<Vec<u8>> {
    const VALIDATION_TIMEOUT: Duration = Duration::from_secs(10);
    const POLL_INTERVAL: Duration = Duration::from_millis(50);

    let Ok(mut child) = std::process::Command::new(path)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return None;
    };
    let deadline = Instant::now() + VALIDATION_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let mut stdout = Vec::new();
                if let Some(mut output) = child.stdout.take() {
                    use std::io::Read as _;
                    let _ = output.read_to_end(&mut stdout);
                }
                return Some(stdout);
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL_INTERVAL),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Whether a downloaded Linux GUI can answer `--version`.
#[cfg(target_os = "linux")]
fn downloaded_exe_runs(path: &std::path::Path) -> bool {
    run_version(path).is_some()
}

/// The version printed by a Linux GUI's `--version` line.
#[cfg(target_os = "linux")]
fn read_exe_version(path: &std::path::Path) -> Option<String> {
    let stdout = run_version(path)?;
    String::from_utf8_lossy(&stdout)
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().last())
        .map(str::to_string)
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
    /// Set when a whole `.app` was swapped rather than a bare executable;
    /// `current_exe` then points at the executable inside the new bundle.
    #[cfg(target_os = "macos")]
    bundle: Option<std::path::PathBuf>,
}

impl StagedSwap {
    /// The executable to spawn as the replacement process.
    pub fn exe_path(&self) -> &std::path::Path {
        &self.current_exe
    }
}

/// Swap the running binary with the new one, leaving the process running.
///
/// Linux creates `.old` as a hard link and atomically replaces the live path,
/// so a concurrent launcher never observes the executable missing and cannot
/// consume the backup. Other platforms rename current -> .old, then .new ->
/// current, rolling the first rename back if the second fails. Both Windows and
/// Unix allow renaming the image of a running process.
pub fn stage_executable_swap(new_exe_path: &std::path::Path) -> Result<StagedSwap, String> {
    let current_exe =
        std::env::current_exe().map_err(|e| format!("Cannot determine current exe path: {}", e))?;
    stage_executable_swap_at(new_exe_path, &current_exe)
}

/// Swap an explicit executable, so tests never touch the running install.
fn stage_executable_swap_at(
    new_exe_path: &std::path::Path,
    current_exe: &std::path::Path,
) -> Result<StagedSwap, String> {
    let old_path = sibling_with_suffix(current_exe, ".old")
        .ok_or_else(|| "Cannot determine backup path".to_string())?;

    // Only a regular file may become the live executable; a symlink or special
    // file staged by something else must not be renamed into place.
    match std::fs::symlink_metadata(new_exe_path) {
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => {
            return Err(format!(
                "{} is not a regular file; refusing to apply it.",
                new_exe_path.display()
            ));
        }
        Err(error) => {
            return Err(format!(
                "Cannot inspect {}: {}",
                new_exe_path.display(),
                error
            ));
        }
    }

    #[cfg(unix)]
    {
        let parent = current_exe.parent().unwrap_or(current_exe);
        if !parent.exists() {
            return Err(swap_error(
                current_exe,
                &std::io::Error::from(std::io::ErrorKind::NotFound),
            ));
        }
        // Fail before renaming anything when the install is not writable, and
        // phrase it as something the user can act on.
        if !can_write_in(parent) {
            return Err(swap_error(
                current_exe,
                &std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            ));
        }
    }

    info!("Applying update: swapping {:?}", current_exe);

    // Remove previous .old file if it exists
    if old_path.exists() {
        std::fs::remove_file(&old_path)
            .map_err(|e| format!("Cannot remove old backup ({}): {}", old_path.display(), e))?;
    }

    #[cfg(target_os = "linux")]
    {
        // Keep the live path present throughout the swap. The launcher restores
        // .old when the executable is missing, so removing current first would
        // let an overlapping launch consume the backup.
        std::fs::hard_link(current_exe, &old_path)
            .map_err(|e| format!("Failed to link current exe to .old: {}", e))?;
        if let Err(e) = std::fs::rename(new_exe_path, current_exe) {
            let _ = std::fs::remove_file(&old_path);
            return Err(format!("Failed to rename new exe into place: {}", e));
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        // Rename the running binary aside
        std::fs::rename(current_exe, &old_path)
            .map_err(|e| format!("Failed to rename current exe to .old: {}", e))?;

        // Rename .new to the original exe name
        if let Err(e) = std::fs::rename(new_exe_path, current_exe) {
            // Rollback: restore original
            let _ = std::fs::rename(&old_path, current_exe);
            return Err(format!("Failed to rename new exe into place: {}", e));
        }
    }

    Ok(StagedSwap {
        current_exe: current_exe.to_path_buf(),
        old_path,
        new_exe_path: new_exe_path.to_path_buf(),
        #[cfg(target_os = "macos")]
        bundle: None,
    })
}

/// Replace the installed `RealmHound.app` with the downloaded one.
///
/// A bundle update has to move the whole directory: overwriting only
/// `Contents/MacOS/RealmHound` would leave the previous release's `Info.plist`
/// and signature material in place. `ditto` keeps the permissions, symlinks and
/// ad-hoc signature that a plain unzip would drop.
#[cfg(target_os = "macos")]
pub fn stage_bundle_swap(archive: &std::path::Path) -> Result<StagedSwap, String> {
    let bundle = installed_bundle().ok_or_else(|| {
        "RealmHound is not running from RealmHound.app, so it cannot replace itself.".to_string()
    })?;
    stage_bundle_swap_at(archive, &bundle)
}

/// Swap an explicit bundle, so a test never touches the real installation.
#[cfg(target_os = "macos")]
fn stage_bundle_swap_at(
    archive: &std::path::Path,
    bundle: &std::path::Path,
) -> Result<StagedSwap, String> {
    let staging = sibling_with_suffix(bundle, ".new")
        .ok_or_else(|| "Cannot determine staging path".to_string())?;
    let old_path = sibling_with_suffix(bundle, ".old")
        .ok_or_else(|| "Cannot determine backup path".to_string())?;

    // Everything below extracts and renames inside this directory, so check it
    // once here rather than letting ditto or rename report a bare errno.
    let parent = bundle.parent().unwrap_or(bundle);
    if !parent.exists() {
        return Err(swap_error(
            bundle,
            &std::io::Error::from(std::io::ErrorKind::NotFound),
        ));
    }
    if !can_write_in(parent) {
        return Err(swap_error(
            bundle,
            &std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        ));
    }

    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|e| swap_error(&staging, &e))?;
    }
    extract_archive(archive, &staging)?;

    // Only swap once the replacement is known to be a runnable bundle, so a
    // truncated or restructured archive cannot strand the install.
    let extracted = staging.join(BUNDLE_NAME);
    if !extracted.join(BUNDLE_EXE).exists() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!(
            "The downloaded archive does not contain {}/{}",
            BUNDLE_NAME, BUNDLE_EXE
        ));
    }

    if old_path.exists() {
        std::fs::remove_dir_all(&old_path).map_err(|e| swap_error(&old_path, &e))?;
    }

    info!("Applying update: swapping {:?}", bundle);

    std::fs::rename(bundle, &old_path).map_err(|e| swap_error(bundle, &e))?;

    if let Err(e) = std::fs::rename(&extracted, bundle) {
        let _ = std::fs::rename(&old_path, bundle);
        return Err(swap_error(bundle, &e));
    }

    Ok(StagedSwap {
        current_exe: bundle.join(BUNDLE_EXE),
        old_path,
        new_exe_path: extracted,
        bundle: Some(bundle.to_path_buf()),
    })
}

/// Unpack `archive` into `into`, which `ditto` creates.
///
/// The archive's SHA-256 is verified during download, before anything here is
/// asked to read it.
#[cfg(target_os = "macos")]
fn extract_archive(archive: &std::path::Path, into: &std::path::Path) -> Result<(), String> {
    let status = std::process::Command::new("/usr/bin/ditto")
        .arg("-x")
        .arg("-k")
        .arg(archive)
        .arg(into)
        .status()
        .map_err(|e| format!("Could not run ditto: {}", e))?;
    if !status.success() {
        return Err(format!("Could not extract the update archive ({})", status));
    }
    Ok(())
}

/// Undo a staged swap after a failed relaunch: move the new exe back out and
/// restore the original so the running process's on-disk image is intact.
pub fn rollback_executable_swap(staged: &StagedSwap) {
    #[cfg(target_os = "macos")]
    if let Some(bundle) = &staged.bundle {
        let _ = std::fs::rename(bundle, &staged.new_exe_path);
        let _ = std::fs::rename(&staged.old_path, bundle);
        return;
    }
    let _ = std::fs::rename(&staged.current_exe, &staged.new_exe_path);
    let _ = std::fs::rename(&staged.old_path, &staged.current_exe);
}

/// Marker written after the GUI has rendered its first frame.
#[cfg(target_os = "linux")]
const STARTUP_CONFIRMED_NAME: &str = "startup-confirmed";

/// Clean up leftovers from a previous update. Call this on app startup.
///
/// Linux keeps the previous GUI until the *same* version has proved it can
/// render a frame (see [`confirm_startup`]), so a process that merely spawned
/// cannot discard the recovery copy. Windows and macOS keep their existing
/// next-launch cleanup behavior.
pub fn cleanup_old_exe() {
    #[cfg(target_os = "macos")]
    if let Some(bundle) = installed_bundle() {
        for suffix in [".old", ".new"] {
            let Some(path) = sibling_with_suffix(&bundle, suffix) else {
                continue;
            };
            if path.exists() {
                match std::fs::remove_dir_all(&path) {
                    Ok(()) => info!("Cleaned up {:?}", path),
                    Err(e) => debug!("Could not clean up {:?}: {}", path, e),
                }
            }
        }
        if let Some(archive) = sibling_with_suffix(&bundle, ".new.zip") {
            let _ = std::fs::remove_file(archive);
        }
        return;
    }
    if let Ok(current_exe) = std::env::current_exe() {
        #[cfg(target_os = "linux")]
        {
            cleanup_linux_exe(&current_exe);
        }
        #[cfg(not(target_os = "linux"))]
        {
            let Some(old_path) = sibling_with_suffix(&current_exe, ".old") else {
                return;
            };
            if old_path.exists() {
                match std::fs::remove_file(&old_path) {
                    Ok(()) => info!("Cleaned up old exe: {:?}", old_path),
                    Err(e) => debug!("Could not clean up old exe: {}", e),
                }
            }
        }
    }
}

/// Remove a stale partial download and, once this exact version has already
/// started successfully, the previous release it replaced.
#[cfg(target_os = "linux")]
fn cleanup_linux_exe(current_exe: &std::path::Path) {
    let Some(dir) = current_exe.parent() else {
        return;
    };

    if let Some(new_path) = sibling_with_suffix(current_exe, ".new") {
        if new_path.exists() {
            match std::fs::remove_file(&new_path) {
                Ok(()) => info!("Removed incomplete update download: {:?}", new_path),
                Err(e) => debug!("Could not remove {:?}: {}", new_path, e),
            }
        }
    }

    let Some(old_path) = sibling_with_suffix(current_exe, ".old") else {
        return;
    };
    if !old_path.exists() {
        return;
    }
    let confirmed = std::fs::read_to_string(dir.join(STARTUP_CONFIRMED_NAME))
        .map(|version| version.trim() == env!("CARGO_PKG_VERSION"))
        .unwrap_or(false);
    if !confirmed {
        info!(
            "Keeping the previous RealmHound at {:?} until this version starts successfully",
            old_path
        );
        return;
    }
    match std::fs::remove_file(&old_path) {
        Ok(()) => info!("Cleaned up old exe: {:?}", old_path),
        Err(e) => debug!("Could not clean up old exe: {}", e),
    }
}

/// Record the version of a replacement before starting it directly.
///
/// The launcher normally does this. A fallback direct relaunch (for example, a
/// missing launcher) still needs the attempt marker so the next desktop launch
/// can detect that the replacement never confirmed startup.
#[cfg(target_os = "linux")]
pub fn record_launch_attempt(exe: &std::path::Path) {
    let Some(version) = read_exe_version(exe) else {
        return;
    };
    let Some(dir) = exe.parent() else {
        return;
    };
    let staging = dir.join(format!(".launch-attempt.{}", std::process::id()));
    if std::fs::write(&staging, version).is_ok() {
        let _ = std::fs::rename(&staging, dir.join(".launch-attempt"));
    }
}

/// Non-Linux relaunches do not use the Linux launcher recovery marker.
#[cfg(not(target_os = "linux"))]
pub fn record_launch_attempt(_exe: &std::path::Path) {}

/// Record that this version reached its first rendered frame.
///
/// Called once per run. The next launch may then remove the previous release,
/// so a broken update keeps its recovery copy through startup.
pub fn confirm_startup(install_kind: InstallKind) {
    #[cfg(target_os = "linux")]
    {
        if install_kind != InstallKind::OfficialUser {
            return;
        }
        let Ok(current_exe) = std::env::current_exe() else {
            return;
        };
        let Some(dir) = current_exe.parent() else {
            return;
        };
        let marker = dir.join(STARTUP_CONFIRMED_NAME);
        let staging = dir.join(format!(".{STARTUP_CONFIRMED_NAME}.{}", std::process::id()));
        if std::fs::write(&staging, env!("CARGO_PKG_VERSION")).is_ok() {
            let _ = std::fs::rename(&staging, &marker);
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = install_kind;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A manifest in the shape published before macOS builds existed.
    const LEGACY_MANIFEST: &str = r#"{
        "latest_version": "0.21.1",
        "download_url": "https://example.invalid/RealmHound.exe",
        "sha256": "aa",
        "releases": []
    }"#;

    #[test]
    fn a_manifest_without_macos_fields_still_parses() {
        // Every published manifest predates those fields, so dropping this
        // compatibility would break the update check for every client.
        let manifest: VersionManifest =
            serde_json::from_str(LEGACY_MANIFEST).expect("legacy manifest parses");
        assert_eq!(manifest.latest_version, "0.21.1");
        assert_eq!(manifest.macos_download_url, None);
        assert_eq!(manifest.macos_sha256, None);
    }

    fn full_manifest() -> VersionManifest {
        serde_json::from_str(
            r#"{
                "latest_version": "0.22.2",
                "download_url": "https://example.invalid/RealmHound.exe",
                "sha256": "ab",
                "macos_download_url": "https://example.invalid/RealmHound-macos-app.zip",
                "macos_sha256": "cd",
                "linux_x86_64_download_url": "https://example.invalid/RealmHound-linux-x86_64",
                "linux_x86_64_sha256": "ef",
                "linux_helper_sha256": "12",
                "linux_min_helper_version": 2,
                "releases": []
            }"#,
        )
        .expect("manifest parses")
    }

    #[test]
    fn each_supported_platform_resolves_only_its_own_binary() {
        let manifest = full_manifest();

        let windows = manifest.artifact_for("windows", "x86_64").unwrap();
        assert!(windows.download_url.ends_with(".exe"));
        assert_eq!(windows.sha256.as_deref(), Some("ab"));
        assert_eq!(windows.min_helper_version, None);

        let macos = manifest.artifact_for("macos", "aarch64").unwrap();
        assert!(macos.download_url.ends_with("-macos-app.zip"));
        assert_eq!(macos.sha256.as_deref(), Some("cd"));
        assert_eq!(macos.min_helper_version, None);

        let linux = manifest.artifact_for("linux", "x86_64").unwrap();
        assert!(linux.download_url.ends_with("-linux-x86_64"));
        assert_eq!(linux.sha256.as_deref(), Some("ef"));
        assert_eq!(linux.min_helper_version, Some(2));
        assert_eq!(linux.helper_sha256.as_deref(), Some("12"));
    }

    #[test]
    fn an_unpublished_linux_architecture_resolves_nothing() {
        let manifest = full_manifest();
        assert_eq!(
            manifest.artifact_for("linux", "aarch64"),
            None,
            "no aarch64 artifact is published"
        );
        assert_eq!(manifest.artifact_for("freebsd", "x86_64"), None);
    }

    #[test]
    fn a_legacy_manifest_never_offers_windows_to_linux() {
        let manifest: VersionManifest =
            serde_json::from_str(LEGACY_MANIFEST).expect("legacy manifest parses");
        assert_eq!(manifest.artifact_for("linux", "x86_64"), None);
        assert!(manifest.artifact_for("windows", "x86_64").is_some());
    }

    #[test]
    fn backup_and_download_names_append_rather_than_replace_the_extension() {
        // `with_extension` would turn the extensionless macOS binary into
        // "RealmHound.exe.old", and must leave the Windows name untouched.
        let windows = std::path::Path::new("/opt/RealmHound.exe");
        assert_eq!(
            sibling_with_suffix(windows, ".old").unwrap(),
            std::path::Path::new("/opt/RealmHound.exe.old")
        );
        let unix = std::path::Path::new("/opt/RealmHound");
        assert_eq!(
            sibling_with_suffix(unix, ".new").unwrap(),
            std::path::Path::new("/opt/RealmHound.new")
        );
    }

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
            macos_download_url: None,
            macos_sha256: None,
            linux_x86_64_download_url: None,
            linux_x86_64_sha256: None,
            linux_aarch64_download_url: None,
            linux_aarch64_sha256: None,
            linux_helper_sha256: None,
            linux_min_helper_version: None,
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
        let updater = SelfUpdater::new(InstallKind::OfficialUser);

        updater.start_download(Some(PlatformArtifact {
            download_url: "http://example.com".to_string(),
            sha256: None,
            min_helper_version: None,
            helper_sha256: None,
        }));

        assert!(matches!(
            updater.state(),
            SelfUpdateState::Failed(message) if message == "Update manifest is missing SHA-256"
        ));
    }

    #[test]
    fn test_invalid_checksum_blocks_download() {
        let updater = SelfUpdater::new(InstallKind::OfficialUser);

        updater.start_download(Some(PlatformArtifact {
            download_url: "http://example.com".to_string(),
            sha256: Some("g".repeat(64)),
            min_helper_version: None,
            helper_sha256: None,
        }));

        assert!(matches!(
            updater.state(),
            SelfUpdateState::Failed(message) if message == "Update manifest has an invalid SHA-256"
        ));
    }

    #[test]
    fn a_platform_without_a_published_binary_is_told_so() {
        let updater = SelfUpdater::new(InstallKind::OfficialUser);

        updater.start_download(None);

        assert!(matches!(
            updater.state(),
            SelfUpdateState::Failed(message) if message.contains("does not publish")
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn only_an_executable_inside_a_bundle_resolves_to_one() {
        use std::path::Path;

        assert_eq!(
            bundle_root_of(Path::new(
                "/Applications/RealmHound.app/Contents/MacOS/RealmHound"
            )),
            Some(Path::new("/Applications/RealmHound.app").to_path_buf())
        );
        // A loose binary or a cargo build is not a bundle install and must
        // never be swapped as one.
        assert_eq!(bundle_root_of(Path::new("/usr/local/bin/RealmHound")), None);
        assert_eq!(
            bundle_root_of(Path::new("/src/target/release/RealmHound")),
            None
        );
        assert_eq!(
            bundle_root_of(Path::new(
                "/Applications/Other.app/Contents/Helpers/RealmHound"
            )),
            None
        );
    }

    /// Zip `bundle` the way the release workflow does.
    #[cfg(target_os = "macos")]
    fn zip_bundle(bundle: &std::path::Path, archive: &std::path::Path) {
        assert!(std::process::Command::new("/usr/bin/ditto")
            .args(["-c", "-k", "--sequesterRsrc", "--keepParent"])
            .arg(bundle)
            .arg(archive)
            .status()
            .expect("ditto")
            .success());
    }

    /// A read-only install directory must be reported as something the user can
    /// fix, not as a raw errno.
    #[cfg(target_os = "macos")]
    #[test]
    fn an_unwritable_install_is_explained_rather_than_attempted() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().expect("temp dir");
        let install_dir = temp.path().join("Applications");
        std::fs::create_dir(&install_dir).expect("mkdir");

        let installed = install_dir.join(BUNDLE_NAME);
        std::fs::create_dir_all(installed.join("Contents/MacOS")).expect("mkdir");
        std::fs::write(installed.join(BUNDLE_EXE), b"old exe").expect("write exe");

        let source = temp.path().join("source").join(BUNDLE_NAME);
        std::fs::create_dir_all(source.join("Contents/MacOS")).expect("mkdir");
        std::fs::write(source.join(BUNDLE_EXE), b"new exe").expect("write exe");
        let archive = temp.path().join("update.zip");
        zip_bundle(&source, &archive);

        assert!(can_write_in(&install_dir), "writable before the chmod");
        std::fs::set_permissions(&install_dir, std::fs::Permissions::from_mode(0o500))
            .expect("chmod");
        assert!(
            !can_write_in(&install_dir),
            "a read-only directory must not look writable"
        );

        let error = stage_bundle_swap_at(&archive, &installed)
            .map(|_| ())
            .expect_err("swap must fail");
        assert!(
            error.contains("No permission to replace"),
            "unhelpful error: {error}"
        );
        assert!(
            !error.contains("os error"),
            "raw errno leaked to the user: {error}"
        );

        // Leave the directory writable so the temp dir can be torn down.
        std::fs::set_permissions(&install_dir, std::fs::Permissions::from_mode(0o700))
            .expect("chmod");
        assert_eq!(
            std::fs::read(installed.join(BUNDLE_EXE)).expect("exe"),
            b"old exe",
            "the install must survive a refused update"
        );
    }

    /// A bundle that vanished between the check and the swap.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_missing_install_is_explained_rather_than_reported_as_an_errno() {
        let temp = tempfile::tempdir().expect("temp dir");

        let source = temp.path().join("source").join(BUNDLE_NAME);
        std::fs::create_dir_all(source.join("Contents/MacOS")).expect("mkdir");
        std::fs::write(source.join(BUNDLE_EXE), b"new exe").expect("write exe");
        let archive = temp.path().join("update.zip");
        zip_bundle(&source, &archive);

        let absent = temp.path().join("gone").join(BUNDLE_NAME);
        let error = stage_bundle_swap_at(&archive, &absent)
            .map(|_| ())
            .expect_err("swap must fail");
        assert!(!error.contains("os error"), "raw errno leaked: {error}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_bundle_update_replaces_the_whole_app() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path();

        // An "installed" bundle carrying metadata from the previous release.
        let installed = root.join(BUNDLE_NAME);
        std::fs::create_dir_all(installed.join("Contents/MacOS")).expect("mkdir");
        std::fs::write(installed.join(BUNDLE_EXE), b"old exe").expect("write exe");
        std::fs::write(installed.join("Contents/Info.plist"), b"old plist").expect("write plist");

        let source = root.join("source").join(BUNDLE_NAME);
        std::fs::create_dir_all(source.join("Contents/MacOS")).expect("mkdir");
        std::fs::write(source.join(BUNDLE_EXE), b"new exe").expect("write exe");
        std::fs::write(source.join("Contents/Info.plist"), b"new plist").expect("write plist");
        let archive = root.join("update.zip");
        zip_bundle(&source, &archive);

        let staged = stage_bundle_swap_at(&archive, &installed).expect("bundle swap");

        assert_eq!(
            std::fs::read(installed.join(BUNDLE_EXE)).expect("exe"),
            b"new exe"
        );
        // The point of a whole-bundle update: no metadata survives from the
        // previous release alongside the new executable.
        assert_eq!(
            std::fs::read(installed.join("Contents/Info.plist")).expect("plist"),
            b"new plist"
        );
        assert_eq!(staged.exe_path(), installed.join(BUNDLE_EXE));

        rollback_executable_swap(&staged);
        assert_eq!(
            std::fs::read(installed.join(BUNDLE_EXE)).expect("exe"),
            b"old exe",
            "a failed relaunch must put the previous bundle back"
        );
        assert_eq!(
            std::fs::read(installed.join("Contents/Info.plist")).expect("plist"),
            b"old plist"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_bundle_update_refuses_an_archive_without_an_executable() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path();

        let installed = root.join(BUNDLE_NAME);
        std::fs::create_dir_all(installed.join("Contents/MacOS")).expect("mkdir");
        std::fs::write(installed.join(BUNDLE_EXE), b"old exe").expect("write exe");

        // Shaped like a bundle but with nothing to run.
        let source = root.join("source").join(BUNDLE_NAME);
        std::fs::create_dir_all(source.join("Contents/MacOS")).expect("mkdir");
        let archive = root.join("update.zip");
        zip_bundle(&source, &archive);

        assert!(stage_bundle_swap_at(&archive, &installed).is_err());
        assert_eq!(
            std::fs::read(installed.join(BUNDLE_EXE)).expect("exe"),
            b"old exe",
            "the install must be left alone when the archive is unusable"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn non_official_linux_installs_are_refused_before_download() {
        for (kind, expected) in [
            (InstallKind::PackageManaged, "package manager"),
            (InstallKind::Unmanaged, "official release installer"),
        ] {
            let updater = SelfUpdater::new(kind);
            updater.start_download(Some(PlatformArtifact {
                download_url: "http://example.invalid/RealmHound".to_string(),
                sha256: None,
                min_helper_version: None,
                helper_sha256: None,
            }));
            assert!(matches!(
                updater.state(),
                SelfUpdateState::Failed(message) if message.contains(expected)
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_unwritable_directory_is_not_reported_as_writable() {
        let temp = tempfile::tempdir().expect("temp dir");
        assert!(can_write_in(temp.path()));
        assert!(!can_write_in(&temp.path().join("missing")));
    }

    #[cfg(unix)]
    #[test]
    fn staging_swaps_the_executable_and_rolls_back() {
        let temp = tempfile::tempdir().expect("temp dir");
        let current = temp.path().join("RealmHound");
        let new = temp.path().join("RealmHound.new");
        std::fs::write(&current, b"old exe").expect("write old");
        std::fs::write(&new, b"new exe").expect("write new");

        let staged = stage_executable_swap_at(&new, &current).expect("swap");

        assert_eq!(staged.exe_path(), current);
        assert_eq!(std::fs::read(&current).expect("exe"), b"new exe");
        assert_eq!(
            std::fs::read(current.with_file_name("RealmHound.old")).expect("backup"),
            b"old exe"
        );

        rollback_executable_swap(&staged);
        assert_eq!(std::fs::read(&current).expect("exe"), b"old exe");
        assert_eq!(std::fs::read(&new).expect("new"), b"new exe");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_staged_binary_is_refused() {
        let temp = tempfile::tempdir().expect("temp dir");
        let current = temp.path().join("RealmHound");
        let target = temp.path().join("elsewhere");
        let new = temp.path().join("RealmHound.new");
        std::fs::write(&current, b"old exe").expect("write current");
        std::fs::write(&target, b"new exe").expect("write target");
        std::os::unix::fs::symlink(&target, &new).expect("symlink");

        let error = match stage_executable_swap_at(&new, &current) {
            Ok(_) => panic!("a symlinked staged binary must be refused"),
            Err(error) => error,
        };

        assert!(error.contains("not a regular file"));
        assert_eq!(
            std::fs::read(&current).expect("current"),
            b"old exe",
            "the live install must not change"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn downloaded_binaries_are_validated_by_exit_status() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().expect("temp dir");
        let executable = |name: &str, body: &str| {
            let path = temp.path().join(name);
            std::fs::write(&path, body).expect("write script");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
            path
        };

        assert!(downloaded_exe_runs(&executable(
            "ok",
            "#!/bin/sh\nexit 0\n"
        )));
        assert!(!downloaded_exe_runs(&executable(
            "bad",
            "#!/bin/sh\nexit 1\n"
        )));
        assert!(!downloaded_exe_runs(&temp.path().join("missing")));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn records_the_replacement_version_before_a_direct_relaunch() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().expect("temp dir");
        let exe = temp.path().join("RealmHound");
        std::fs::write(&exe, "#!/bin/sh\necho 'RealmHound 9.9.9'\n").expect("write");
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        record_launch_attempt(&exe);

        assert_eq!(
            std::fs::read_to_string(temp.path().join(".launch-attempt")).expect("marker"),
            "9.9.9"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_linux_swap_keeps_a_hard_link_backup() {
        use std::os::unix::fs::MetadataExt as _;

        let temp = tempfile::tempdir().expect("temp dir");
        let current = temp.path().join("RealmHound");
        let new = temp.path().join("RealmHound.new");
        std::fs::write(&current, b"old exe").expect("write old");
        std::fs::write(&new, b"new exe").expect("write new");
        let old_inode = std::fs::metadata(&current).expect("metadata").ino();

        let staged = stage_executable_swap_at(&new, &current).expect("swap");
        let old = current.with_file_name("RealmHound.old");

        assert_eq!(std::fs::metadata(&old).expect("old").ino(), old_inode);
        assert_eq!(std::fs::read(&old).expect("old"), b"old exe");
        assert_eq!(std::fs::read(&current).expect("current"), b"new exe");
        assert_ne!(
            std::fs::metadata(&old).expect("old").ino(),
            std::fs::metadata(&current).expect("current").ino()
        );

        rollback_executable_swap(&staged);
        assert_eq!(std::fs::read(&current).expect("current"), b"old exe");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cleanup_keeps_the_previous_version_until_this_version_has_started() {
        let temp = tempfile::tempdir().expect("temp dir");
        let current = temp.path().join("RealmHound");
        let old = temp.path().join("RealmHound.old");
        let new = temp.path().join("RealmHound.new");
        std::fs::write(&current, b"new exe").expect("write current");
        std::fs::write(&old, b"old exe").expect("write old");
        std::fs::write(&new, b"partial").expect("write new");

        cleanup_linux_exe(&current);
        assert!(!new.exists(), "a partial download must be removed");
        assert!(old.exists(), "spawning alone must not discard the backup");

        std::fs::write(
            temp.path().join(STARTUP_CONFIRMED_NAME),
            env!("CARGO_PKG_VERSION"),
        )
        .expect("write marker");
        cleanup_linux_exe(&current);
        assert!(!old.exists(), "a confirmed start releases the backup");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_marker_from_another_version_does_not_discard_the_backup() {
        let temp = tempfile::tempdir().expect("temp dir");
        let current = temp.path().join("RealmHound");
        let old = temp.path().join("RealmHound.old");
        std::fs::write(&current, b"new exe").expect("write current");
        std::fs::write(&old, b"old exe").expect("write old");
        std::fs::write(temp.path().join(STARTUP_CONFIRMED_NAME), "0.0.0").expect("write marker");

        cleanup_linux_exe(&current);

        assert!(old.exists(), "another version's success is not this one's");
    }
}

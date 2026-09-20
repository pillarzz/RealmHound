//! RealmHound - RotMG Packet Sniffer
//!
//! A high-performance packet capture and analysis tool for Realm of the Mad God.

// Hide console window in release builds on Windows
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use anyhow::Result;
use eframe::egui;
use realmhound_core::account::{
    AccountContext, CredentialStore, DiscoveryStartup, StartupResolution, StartupResolver,
};
use realmhound_core::storage::StorageRoot;
use realmhound_core::Settings;
use std::fs;
use std::sync::{mpsc, Arc, RwLock};
use std::time::{Duration, Instant};
use tracing::{info, Level};
use tracing_subscriber::{filter::Targets, fmt, layer::SubscriberExt, util::SubscriberInitExt};

#[cfg(windows)]
mod power_throttling;
#[cfg(windows)]
mod single_instance;

mod app;
mod audio;
mod capture_manager;
mod client_process;
mod discovery;
mod enchant_sound;
mod event_log;
mod memo;
mod modals;
mod panels;
mod processing;
mod profiling;
mod realm_event_sound;
mod rendering;
mod shadcn_ui;
mod sound;
mod tab_icons;
mod ui_colors;
mod ui_ext;
mod virtual_list;

#[cfg(test)]
mod test_support {
    use std::path::PathBuf;
    use std::sync::{Mutex, MutexGuard};

    static ASSET_MANAGER_LOCK: Mutex<()> = Mutex::new(());

    pub fn asset_manager_guard() -> MutexGuard<'static, ()> {
        ASSET_MANAGER_LOCK.lock().unwrap()
    }

    pub fn modifier_assets() -> MutexGuard<'static, ()> {
        let guard = asset_manager_guard();
        // Only modifier lookups are expected to succeed from this fixture.
        let assets_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("assets");
        realmhound_core::assets::get_asset_manager().set_assets_dir(assets_dir);
        guard
    }
}

use app::{RealmHoundApp, PKG_NAME, VERSION};

fn main() -> Result<()> {
    // A relaunch replacement waits (bounded) for the previous instance to exit
    // before contending for the mutex or profile lock.
    #[cfg(windows)]
    if let Some(pid) = relaunch_wait_pid() {
        realmhound_core::relaunch::wait_for_process_exit(pid, std::time::Duration::from_secs(10));
    }

    // Ensure only one instance of RealmHound runs at a time.
    // The guard must live for the entire program duration.
    #[cfg(windows)]
    let _single_instance = single_instance::acquire_or_exit();

    // Initialize logging
    // In debug builds: log to console
    // In release builds: log to file (no console window)
    let _guard = init_logging();

    info!("{} v{} starting...", PKG_NAME, VERSION);
    #[cfg(feature = "latency-diagnostics")]
    info!(
        "[LATENCY] Diagnostic instrumentation enabled pipeline_age_warn_ms=500 \
         pipeline_work_warn_ms=250 pipeline_extreme_age_ms=2000 \
         pipeline_extreme_work_ms=1000 warning_throttle_ms=2000 \
         health_min_interval_ms=30000 storage_warn_ms=100 \
         ui_queue_warn_ms=500 audio_queue_warn_ms=250 audio_stale_drop_ms=5000 \
         capture_queue_capacity={} pcap_timeout_ms={}",
        realmhound_core::capture::DEFAULT_QUEUE_CAPACITY,
        realmhound_core::capture::SnifferConfig::default().timeout_ms
    );
    #[cfg(all(feature = "latency-diagnostics", windows))]
    log_latency_system_info();

    // Keep capture/worker/audio threads full-speed while minimised.
    #[cfg(windows)]
    power_throttling::disable_background_throttling();

    // Clean up leftover .old exe from a previous self-update
    realmhound_core::update::cleanup_old_exe();

    let preview = preview_settings();
    let (window_size, window_pos, maximized) = (
        [preview.window.width, preview.window.height],
        preview.window.x.zip(preview.window.y),
        preview.window.maximized,
    );

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size(window_size)
        .with_min_inner_size([800.0, 600.0])
        .with_title("RealmHound")
        // Pass an explicit empty icon so eframe skips its runtime WM_SETICON path
        // (which scales a single bitmap to system-metric sizes and blurs on high-DPI
        // taskbars). Windows then uses the crisp multi-size .ico embedded as a
        // resource by build.rs, picking the correct frame per DPI.
        .with_icon(egui::IconData::default());

    if let Some((x, y)) = window_pos {
        if x > -10000.0 && y > -10000.0 {
            viewport = viewport.with_position([x, y]);
        }
    }
    if maximized {
        viewport = viewport.with_maximized(true);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    // Decide whether startup can resolve synchronously (fast path) or needs a
    // background thread with a progress screen (slow path). The slow path is
    // only needed when migration is pending (can take minutes) or a relaunch
    // replacement must wait for the previous instance's profile lock.
    let is_relaunch = std::env::args().any(|a| a == "--relaunch");
    let resolver = build_resolver(is_relaunch);

    match resolver {
        Some(resolver) if !is_relaunch && !resolver.migration_pending() => {
            // Fast path: resolve synchronously so the first frame is the real app.
            let outcome = resolver.resolve_startup();
            let launch = into_launch(outcome.resolution);
            eframe::run_native(
                "RealmHound",
                options,
                Box::new(move |cc| Ok(RootApp::build(&cc.egui_ctx, outcome.settings, launch))),
            )
            .map_err(|e| anyhow::anyhow!("Failed to run application: {}", e))?;
        }
        _ => {
            // Slow path: resolve on a background thread behind a progress screen.
            eframe::run_native(
                "RealmHound",
                options,
                Box::new(|cc| {
                    let egui_ctx = cc.egui_ctx.clone();
                    let (tx, rx) = mpsc::channel();
                    std::thread::Builder::new()
                        .name("startup-resolver".to_string())
                        .spawn(move || {
                            resolve_startup(&tx);
                            egui_ctx.request_repaint();
                        })
                        .map_err(|e| {
                            Box::<dyn std::error::Error + Send + Sync>::from(e.to_string())
                        })?;
                    Ok(Box::new(RootApp::new(rx)) as Box<dyn eframe::App>)
                }),
            )
            .map_err(|e| anyhow::anyhow!("Failed to run application: {}", e))?;
        }
    }

    Ok(())
}

#[cfg(all(feature = "latency-diagnostics", windows))]
fn log_latency_system_info() {
    let logical_processors = logical_processor_count();
    let cpu = cpu_model_name().unwrap_or_else(|| {
        format!(
            "{} ({logical_processors} logical processors)",
            std::env::consts::ARCH
        )
    });
    let display_adapters = attached_display_adapters().unwrap_or_else(|| "unknown".to_string());
    let (total_memory, available_memory) = physical_memory()
        .map(|(total, available)| (format_memory(total), format_memory(available)))
        .unwrap_or_else(|| ("unknown".to_string(), "unknown".to_string()));

    info!(
        "[LATENCY][SYSTEM] cpu=\"{}\" logical_processors={} display_adapters=\"{}\"",
        cpu, logical_processors, display_adapters
    );
    info!(
        "[LATENCY][SYSTEM] physical_memory_total={} physical_memory_available={}",
        total_memory, available_memory
    );
}

#[cfg(all(feature = "latency-diagnostics", windows))]
fn logical_processor_count() -> u32 {
    use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
    use windows_sys::Win32::System::Threading::{GetActiveProcessorCount, ALL_PROCESSOR_GROUPS};

    let active = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) };
    if active > 0 {
        return active;
    }
    let mut info: SYSTEM_INFO = unsafe { std::mem::zeroed() };
    unsafe {
        GetSystemInfo(&mut info);
    }
    info.dwNumberOfProcessors
}

#[cfg(all(feature = "latency-diagnostics", windows))]
fn cpu_model_name() -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY_LOCAL_MACHINE, KEY_READ, REG_EXPAND_SZ,
        REG_SZ,
    };

    let path: Vec<u16> = "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0\0"
        .encode_utf16()
        .collect();
    let value: Vec<u16> = "ProcessorNameString\0".encode_utf16().collect();
    let mut key = 0;
    if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, KEY_READ, &mut key) } != 0 {
        return None;
    }

    let mut value_type = 0;
    let mut byte_len = 0;
    let size_result = unsafe {
        RegQueryValueExW(
            key,
            value.as_ptr(),
            std::ptr::null_mut(),
            &mut value_type,
            std::ptr::null_mut(),
            &mut byte_len,
        )
    };
    if size_result != 0 || !matches!(value_type, REG_SZ | REG_EXPAND_SZ) || byte_len < 2 {
        unsafe {
            RegCloseKey(key);
        }
        return None;
    }

    let mut buffer = vec![0u16; byte_len as usize / 2];
    let query_result = unsafe {
        RegQueryValueExW(
            key,
            value.as_ptr(),
            std::ptr::null_mut(),
            &mut value_type,
            buffer.as_mut_ptr().cast(),
            &mut byte_len,
        )
    };
    unsafe {
        RegCloseKey(key);
    }
    if query_result != 0 {
        return None;
    }
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    let name = sanitize_system_label(&String::from_utf16_lossy(&buffer[..end]));
    (!name.is_empty()).then_some(name)
}

#[cfg(all(feature = "latency-diagnostics", windows))]
fn attached_display_adapters() -> Option<String> {
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayDevicesW, DISPLAY_DEVICEW, DISPLAY_DEVICE_ATTACHED_TO_DESKTOP,
    };

    let mut index = 0;
    let mut names = Vec::new();
    loop {
        let mut device: DISPLAY_DEVICEW = unsafe { std::mem::zeroed() };
        device.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
        if unsafe { EnumDisplayDevicesW(std::ptr::null(), index, &mut device, 0) } == 0 {
            break;
        }
        let attached = device.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP != 0;
        if attached {
            let end = device
                .DeviceString
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(device.DeviceString.len());
            names.push(String::from_utf16_lossy(&device.DeviceString[..end]));
        }
        index += 1;
    }
    format_display_adapter_names(names)
}

#[cfg(all(feature = "latency-diagnostics", windows))]
fn physical_memory() -> Option<(u64, u64)> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    (unsafe { GlobalMemoryStatusEx(&mut status) } != 0)
        .then_some((status.ullTotalPhys, status.ullAvailPhys))
}

#[cfg(all(feature = "latency-diagnostics", windows))]
fn format_memory(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * MIB;
    if bytes >= GIB as u64 {
        format!("{:.1} GiB", bytes as f64 / GIB)
    } else {
        format!("{:.0} MiB", bytes as f64 / MIB)
    }
}

#[cfg(all(feature = "latency-diagnostics", windows))]
fn sanitize_system_label(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(160)
        .collect()
}

#[cfg(all(feature = "latency-diagnostics", windows))]
fn format_display_adapter_names<I, S>(names: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    const MAX_ADAPTERS: usize = 8;
    const MAX_TOTAL_LEN: usize = 512;

    let mut unique = Vec::new();
    let mut total_len = 0usize;
    for name in names {
        let name = sanitize_system_label(name.as_ref());
        if name.is_empty() || unique.iter().any(|existing| existing == &name) {
            continue;
        }
        let added_len = name.chars().count() + usize::from(!unique.is_empty()) * 3;
        if unique.len() >= MAX_ADAPTERS || total_len + added_len > MAX_TOTAL_LEN {
            break;
        }
        total_len += added_len;
        unique.push(name);
    }
    (!unique.is_empty()).then(|| unique.join(" | "))
}

#[cfg(all(test, feature = "latency-diagnostics", windows))]
mod latency_system_tests {
    use super::{format_display_adapter_names, format_memory, sanitize_system_label};

    #[test]
    fn formats_memory_in_gibibytes() {
        assert_eq!(format_memory(16 * 1024 * 1024 * 1024), "16.0 GiB");
    }

    #[test]
    fn formats_memory_in_mebibytes_below_one_gibibyte() {
        assert_eq!(format_memory(768 * 1024 * 1024), "768 MiB");
    }

    #[test]
    fn sanitizes_system_labels_for_logging() {
        assert_eq!(
            sanitize_system_label("  Example\r\nAdapter  "),
            "Example Adapter"
        );
    }

    #[test]
    fn display_adapters_are_sanitized_and_deduplicated() {
        assert_eq!(
            format_display_adapter_names([
                " DisplayLink USB Device ",
                "Example GPU",
                "Example\r\nGPU",
                "",
            ]),
            Some("DisplayLink USB Device | Example GPU".to_string())
        );
    }

    #[test]
    fn display_adapter_list_is_capped() {
        let names: Vec<String> = (0..12).map(|index| format!("Adapter {index}")).collect();
        let formatted = format_display_adapter_names(&names).unwrap();
        assert_eq!(formatted.split(" | ").count(), 8);
        assert!(formatted.chars().count() <= 512);
    }

    #[test]
    fn display_adapter_length_cap_counts_characters() {
        let first = "\u{663e}".repeat(160);
        let second = "\u{5361}".repeat(160);
        let formatted = format_display_adapter_names([first, second]).unwrap();
        assert_eq!(formatted.chars().count(), 323);
    }
}

/// Non-destructive read of installation settings for the initial window size,
/// before the background resolver produces the authoritative settings.
fn preview_settings() -> Settings {
    match Settings::settings_path() {
        Some(path) => Settings::load_explicit(&path).unwrap_or_default(),
        None => Settings::default(),
    }
}

/// Build a [`StartupResolver`] from the default storage roots. Returns `None`
/// when the local storage root cannot be created (the slow path will surface
/// the error through its own recovery UI).
fn build_resolver(is_relaunch: bool) -> Option<StartupResolver> {
    let local = StorageRoot::default_local().ok()?;
    let roaming = std::env::var_os("APPDATA")
        .map(|app_data| std::path::PathBuf::from(app_data).join("RealmHound"))
        .and_then(|path| StorageRoot::from_path(path).ok());
    let credentials = make_credential_store();
    let mut resolver = StartupResolver::new(local, roaming, credentials);
    if is_relaunch {
        resolver = resolver.with_relaunch_lock_wait(Duration::from_secs(10));
    }
    Some(resolver)
}

fn into_launch(resolution: StartupResolution) -> StartupLaunch {
    match resolution {
        StartupResolution::Selected(selected) => {
            StartupLaunch::Selected(Arc::new(selected.into_context()))
        }
        StartupResolution::Discovery(discovery) => StartupLaunch::Discovery(discovery),
        StartupResolution::Recovery(recovery) => {
            StartupLaunch::Recovery(recovery.into_error().to_string())
        }
    }
}

/// Startup progress sent from the background resolver to [`RootApp`].
enum StartupMessage {
    /// Migration work is actually pending; show the migration screen.
    Migrating,
    /// Startup resolved; build the application.
    Done(Settings, StartupLaunch),
}

/// Top-level application shown from launch. It starts in a neutral resource-free
/// starting screen while startup resolves on a background thread, switching to
/// the migration screen only if migration is pending, then becomes the selected-
/// profile application or a bounded discovery/recovery window.
struct RootApp {
    state: RootState,
    resolved: mpsc::Receiver<StartupMessage>,
    started: Instant,
}

enum RootState {
    /// Startup is resolving; no migration work has been reported.
    Starting,
    /// A one-time migration is actively running.
    Migrating,
    /// The resolved application: full profile app or a bounded window.
    Active(Box<dyn eframe::App>),
}

impl RootApp {
    fn new(resolved: mpsc::Receiver<StartupMessage>) -> Self {
        Self {
            state: RootState::Starting,
            resolved,
            started: Instant::now(),
        }
    }

    /// Build the resolved application from a completed startup outcome.
    fn build(
        egui_ctx: &egui::Context,
        settings: Settings,
        launch: StartupLaunch,
    ) -> Box<dyn eframe::App> {
        match launch {
            StartupLaunch::Selected(account) => {
                let settings = Arc::new(RwLock::new(settings));
                match RealmHoundApp::new(egui_ctx, settings, account) {
                    Ok(app) => Box::new(app),
                    Err(error) => {
                        // Construction dropped the account context, releasing its
                        // lock; show recovery instead of falling back to flat data.
                        tracing::error!("[STARTUP] Application construction failed: {error}");
                        Box::new(BoundedApp::recovery(format!(
                            "RealmHound could not open the selected account profile: {error}. \
                             Please relaunch RealmHound to try again."
                        )))
                    }
                }
            }
            StartupLaunch::Discovery(discovery) => {
                info!("[STARTUP] Entering account discovery (no profile selected)");
                Box::new(BoundedApp::discovery(discovery))
            }
            StartupLaunch::Recovery(message) => {
                info!("[STARTUP] Recovery: {message}");
                Box::new(BoundedApp::recovery(message))
            }
        }
    }

    fn render_starting(&self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(72.0);
                ui.heading("RealmHound");
                ui.add_space(28.0);
                ui.add(egui::Spinner::new().size(48.0));
                ui.add_space(28.0);
                ui.label("Starting…");
            });
        });
    }

    fn render_migrating(&self, ctx: &egui::Context) {
        let elapsed = self.started.elapsed().as_secs();
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(72.0);
                ui.heading("RealmHound");
                ui.add_space(28.0);
                ui.add(egui::Spinner::new().size(48.0));
                ui.add_space(28.0);
                ui.label("Upgrading your data to the new account profile format.");
                ui.label("This one-time step can take a few minutes for large histories.");
                ui.add_space(10.0);
                ui.label(egui::RichText::new("Please don't close RealmHound.").strong());
                ui.add_space(18.0);
                ui.label(format!("Elapsed: {:02}:{:02}", elapsed / 60, elapsed % 60));
            });
        });
    }
}

impl eframe::App for RootApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        #[cfg(windows)]
        crate::apply_window_chrome(&*frame);
        if !matches!(self.state, RootState::Active(_)) {
            match self.resolved.try_recv() {
                Ok(StartupMessage::Migrating) => {
                    self.state = RootState::Migrating;
                }
                Ok(StartupMessage::Done(settings, launch)) => {
                    self.state = RootState::Active(Self::build(ctx, settings, launch));
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.state = RootState::Active(Box::new(BoundedApp::recovery(
                        "RealmHound startup failed unexpectedly. Please relaunch RealmHound."
                            .to_string(),
                    )));
                }
            }

            match &self.state {
                RootState::Starting => {
                    self.render_starting(ctx);
                    ctx.request_repaint_after(Duration::from_millis(150));
                }
                RootState::Migrating => {
                    self.render_migrating(ctx);
                    ctx.request_repaint_after(Duration::from_millis(150));
                }
                RootState::Active(_) => ctx.request_repaint(),
            }
        }

        if let RootState::Active(app) = &mut self.state {
            app.update(ctx, frame);
        }
    }
}

/// The three ways RealmHound launches, resolved before any account resource is
/// opened.
enum StartupLaunch {
    /// One validated, exclusively locked profile.
    Selected(Arc<AccountContext>),
    /// Account discovery: no account-scoped resource is opened.
    Discovery(DiscoveryStartup),
    /// A recoverable startup error, shown to the user.
    Recovery(String),
}

/// Parse the `--wait-pid <pid>` argument a relaunch replacement carries, if any.
#[cfg(windows)]
fn relaunch_wait_pid() -> Option<u32> {
    let mut args = std::env::args();
    while let Some(arg) = args.next() {
        if arg == "--wait-pid" {
            return args.next().and_then(|v| v.parse().ok());
        }
    }
    None
}

/// Resolve the startup mode from the default installation storage root, loading
/// installation settings from that same explicit root in one pass. Signals
/// [`StartupMessage::Migrating`] before resolving only when migration work is
/// actually pending, then always sends [`StartupMessage::Done`].
fn resolve_startup(progress: &mpsc::Sender<StartupMessage>) {
    let resolver = match build_resolver(std::env::args().any(|a| a == "--relaunch")) {
        Some(r) => r,
        None => {
            let _ = progress.send(StartupMessage::Done(
                Settings::default(),
                StartupLaunch::Recovery("Failed to initialize storage".to_string()),
            ));
            return;
        }
    };
    if resolver.migration_pending() {
        let _ = progress.send(StartupMessage::Migrating);
    }
    let outcome = resolver.resolve_startup();
    let launch = into_launch(outcome.resolution);
    let _ = progress.send(StartupMessage::Done(outcome.settings, launch));
}

/// Build the platform credential store. Windows uses Credential Manager; other
/// platforms fall back to an in-memory store.
fn make_credential_store() -> Arc<dyn CredentialStore> {
    #[cfg(windows)]
    {
        Arc::new(realmhound_core::account::WindowsCredentialStore::new())
    }
    #[cfg(not(windows))]
    {
        Arc::new(realmhound_core::account::InMemoryCredentialStore::new())
    }
}

/// Bounded startup window for discovery and recovery. Opens no account-scoped
/// resource.
struct BoundedApp {
    heading: &'static str,
    message: String,
    discovery: Option<DiscoveryStartup>,
    credential_store: Option<Arc<dyn CredentialStore>>,
    capture_handle: Option<discovery::DiscoveryCaptureHandle>,
    candidate_rx: Option<std::sync::mpsc::Receiver<discovery::DiscoveredCandidate>>,
    candidates: Vec<discovery::DiscoveredCandidate>,
    selected_idx: Option<usize>,
    action_error: Option<String>,
    /// Disarms the Drop safety net after a successful commit or cancel.
    committed: bool,
}

impl BoundedApp {
    fn recovery(message: String) -> Self {
        Self {
            heading: "Startup recovery",
            message,
            discovery: None,
            credential_store: None,
            capture_handle: None,
            candidate_rx: None,
            candidates: Vec::new(),
            selected_idx: None,
            action_error: None,
            committed: false,
        }
    }

    fn discovery(discovery: DiscoveryStartup) -> Self {
        let credential_store: Arc<dyn CredentialStore> = make_credential_store();

        let interfaces = realmhound_core::capture::NetworkInterface::list_all();
        let (capture_handle, candidate_rx) = match interfaces {
            Ok(ref ifaces) if !ifaces.is_empty() => {
                match discovery::start_discovery_capture(ifaces) {
                    Ok((handle, rx)) => (Some(handle), Some(rx)),
                    Err(e) => {
                        tracing::warn!("[DISCOVERY] Failed to start capture: {e}");
                        (None, None)
                    }
                }
            }
            Ok(_) => {
                tracing::warn!("[DISCOVERY] No network interfaces found");
                (None, None)
            }
            Err(e) => {
                tracing::warn!("[DISCOVERY] Failed to list interfaces: {e}");
                (None, None)
            }
        };

        Self {
            heading: "Account discovery",
            message: "Connect to a game server with the account you want to add. \
                      RealmHound is listening for connections..."
                .to_string(),
            discovery: Some(discovery),
            credential_store: Some(credential_store),
            capture_handle,
            candidate_rx,
            candidates: Vec::new(),
            selected_idx: None,
            action_error: None,
            committed: false,
        }
    }

    fn poll_candidates(&mut self) {
        let Some(rx) = &self.candidate_rx else { return };
        while let Ok(candidate) = rx.try_recv() {
            // Deduplicate by account ID.
            if let Some(pos) = self
                .candidates
                .iter()
                .position(|c| c.account_id == candidate.account_id)
            {
                self.candidates[pos] = candidate;
            } else {
                self.candidates.push(candidate);
            }
        }
        // Auto-select when there's exactly one candidate.
        if self.candidates.len() == 1 && self.selected_idx.is_none() {
            self.selected_idx = Some(0);
        }
    }

    /// Commit the selected candidate: register, store credential, select, relaunch.
    fn commit_selected(&mut self, ctx: &egui::Context) {
        let Some(idx) = self.selected_idx else { return };
        let Some(candidate) = self.candidates.get(idx).cloned() else {
            return;
        };
        let Some(discovery) = &self.discovery else {
            return;
        };

        self.capture_handle.take();
        self.candidate_rx.take();

        let now = chrono::Utc::now();

        let account_key = match discovery.register_and_select(
            candidate.account_id,
            candidate.display_name.as_deref(),
            now,
        ) {
            Ok(key) => key,
            Err(e) => {
                self.action_error = Some(format!("Failed to register account: {e}"));
                return;
            }
        };

        // Non-fatal: next HELLO will recapture the token.
        if let Some(cred_store) = &self.credential_store {
            let target = realmhound_core::account::credential_target(account_key);
            let cred = realmhound_core::account::SavedCredential::new(candidate.token, Some(now));
            if let Err(e) = cred_store.write(&target, &cred) {
                tracing::warn!("[DISCOVERY] Credential store failed: {e}");
            }
        }

        let exe = match std::env::current_exe() {
            Ok(path) => path,
            Err(e) => {
                self.action_error = Some(format!("Cannot resolve executable: {e}"));
                return;
            }
        };

        if let Err(e) = realmhound_core::relaunch::spawn_replacement(&exe, std::process::id()) {
            self.action_error = Some(format!("Relaunch failed: {e}"));
            return;
        }

        self.committed = true;
        self.discovery.take();
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Cancel discovery: restore previous account and relaunch or exit.
    fn cancel_discovery(&mut self, ctx: &egui::Context) {
        self.capture_handle.take();
        self.candidate_rx.take();

        if let Some(discovery) = self.discovery.take() {
            let had_previous = discovery.previous_account_key().is_some();
            if let Err(e) = discovery.cancel_discovery(chrono::Utc::now()) {
                tracing::warn!("[DISCOVERY] Failed to restore previous selection: {e}");
                self.action_error = Some(format!("Failed to restore previous account: {e}"));
                return;
            }
            self.committed = true;
            if had_previous {
                let exe = match std::env::current_exe() {
                    Ok(path) => path,
                    Err(e) => {
                        self.action_error = Some(format!(
                            "Previous account restored but restart failed: {e}. \
                                          Please restart RealmHound manually."
                        ));
                        return;
                    }
                };
                if let Err(e) =
                    realmhound_core::relaunch::spawn_replacement(&exe, std::process::id())
                {
                    self.action_error = Some(format!(
                        "Previous account restored but restart failed: {e}. \
                                      Please restart RealmHound manually."
                    ));
                    return;
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
}

impl eframe::App for BoundedApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(windows)]
        crate::apply_window_chrome(&*_frame);

        self.poll_candidates();

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(48.0);
                ui.heading(self.heading);
                ui.add_space(16.0);

                if let Some(err) = &self.action_error {
                    ui.label(
                        egui::RichText::new(err.as_str())
                            .color(egui::Color32::from_rgb(255, 100, 100)),
                    );
                    ui.add_space(8.0);
                }

                if self.discovery.is_some() {
                    // Discovery mode UI.
                    if self.candidates.is_empty() {
                        ui.label(&self.message);
                        ui.add_space(16.0);
                        ui.add(egui::Spinner::new().size(32.0));
                    } else {
                        ui.label("Verified account(s) detected:");
                        ui.add_space(10.0);

                        let mut commit = false;
                        for (i, candidate) in self.candidates.iter().enumerate() {
                            let name = candidate.display_name.as_deref().unwrap_or("Unknown");
                            let selected = self.selected_idx == Some(i);
                            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                                ui.horizontal(|ui| {
                                    if ui
                                        .selectable_label(
                                            selected,
                                            egui::RichText::new(name).strong(),
                                        )
                                        .clicked()
                                    {
                                        self.selected_idx = Some(i);
                                    }
                                    if selected && ui.button("Add this account").clicked() {
                                        commit = true;
                                    }
                                });
                            });
                        }

                        if commit {
                            self.commit_selected(ctx);
                        }
                    }

                    ui.add_space(16.0);
                    if ui.button("Cancel").clicked() {
                        self.cancel_discovery(ctx);
                    }
                } else {
                    // Recovery mode: static message only.
                    ui.label(&self.message);
                }
            });
        });

        // Keep repainting while discovery is active (polling candidates).
        if self.discovery.is_some() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }
}

impl Drop for BoundedApp {
    fn drop(&mut self) {
        self.capture_handle.take();
        self.candidate_rx.take();

        // Safety net for unexpected exits (committed flows clear `discovery`).
        if let Some(discovery) = self.discovery.take() {
            if !self.committed {
                if let Err(error) = discovery.cancel_discovery(chrono::Utc::now()) {
                    tracing::warn!("[STARTUP] Failed to restore previous selection: {error}");
                }
            }
        }
    }
}

/// Apply the crisp title-bar icon and dark-mode caption on the first frame.
/// Safe to call from multiple `eframe::App` impls -- the `Once` guard ensures
/// the Win32 calls execute exactly once per process.
#[cfg(windows)]
pub fn apply_window_chrome<W: raw_window_handle::HasWindowHandle>(window: &W) {
    use std::sync::Once;
    static CHROME: Once = Once::new();
    CHROME.call_once(|| {
        set_titlebar_icon_from_resource(window);
        set_titlebar_dark_mode(window);
    });
}

/// Set the window's title-bar icon (ICON_SMALL/ICON_BIG) from the multi-size
/// `.ico` embedded as resource id 1 by winres. eframe is told to use an empty
/// `IconData` so it skips its own WM_SETICON path (which upscales one bitmap and
/// blurs on high-DPI). `LoadImageW` instead picks the crisp best-matching frame
/// straight from the embedded resource.
#[cfg(windows)]
pub fn set_titlebar_icon_from_resource<W: raw_window_handle::HasWindowHandle>(window: &W) {
    use raw_window_handle::RawWindowHandle;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, LoadImageW, SendMessageW, ICON_BIG, ICON_SMALL, IMAGE_ICON,
        LR_DEFAULTCOLOR, SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON, WM_SETICON,
    };

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return;
    };
    let hwnd = win32.hwnd.get() as HWND;

    unsafe {
        let hinstance = GetModuleHandleW(std::ptr::null());
        let icon_id = 1 as *const u16; // MAKEINTRESOURCEW(1)

        let small = LoadImageW(
            hinstance,
            icon_id,
            IMAGE_ICON,
            GetSystemMetrics(SM_CXSMICON),
            GetSystemMetrics(SM_CYSMICON),
            LR_DEFAULTCOLOR,
        );
        if small != 0 {
            SendMessageW(hwnd, WM_SETICON, ICON_SMALL as WPARAM, small as LPARAM);
        }

        let big = LoadImageW(
            hinstance,
            icon_id,
            IMAGE_ICON,
            GetSystemMetrics(SM_CXICON),
            GetSystemMetrics(SM_CYICON),
            LR_DEFAULTCOLOR,
        );
        if big != 0 {
            SendMessageW(hwnd, WM_SETICON, ICON_BIG as WPARAM, big as LPARAM);
        }
    }
}

/// Pin the native title bar to a fixed dark caption so it stays dark even while
/// the window is unfocused. Since the egui 0.27 -> 0.33 upgrade the caption is
/// only painted black while focused; when unfocused Windows repaints the
/// *inactive* caption a lighter shade (a yellowish-grey). Setting an explicit
/// `DWMWA_CAPTION_COLOR` (Windows 11 22000+) overrides both focus states with one
/// dark colour, and `DWMWA_USE_IMMERSIVE_DARK_MODE` keeps the caption text light.
/// Affects only the OS-drawn non-client caption -- egui's client rendering is
/// untouched. Both attributes are best-effort no-ops on older Windows builds.
#[cfg(windows)]
pub fn set_titlebar_dark_mode<W: raw_window_handle::HasWindowHandle>(window: &W) {
    use raw_window_handle::RawWindowHandle;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
    };

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return;
    };
    let hwnd = win32.hwnd.get() as HWND;

    unsafe {
        let enabled: i32 = 1; // BOOL TRUE -- light caption text
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            &enabled as *const i32 as *const core::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );

        // Near-black COLORREF (0x00BBGGRR) matching the focused caption, applied to
        // both focus states so the bar never lightens when unfocused.
        let caption: u32 = 0x0019_1919;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR as u32,
            &caption as *const u32 as *const core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        );

        // DWMWA_CAPTION_COLOR is Windows 11-only. On Windows 10 the system repaints
        // the inactive caption a lighter shade and won't accept a custom colour, so
        // subclass the window and keep it in its active (dark) appearance instead.
        windows_sys::Win32::UI::Shell::SetWindowSubclass(
            hwnd,
            Some(keep_caption_active_subclass),
            KEEP_ACTIVE_SUBCLASS_ID,
            0,
        );
    }
}

/// Subclass proc that forces the native title bar to keep its active (dark)
/// appearance even when the window is unfocused. `WM_NCACTIVATE` normally tells
/// Windows to repaint the non-client caption in the *inactive* colour; passing
/// `wParam = TRUE` down the chain keeps it painting the active caption. All other
/// messages are forwarded unchanged so winit's own handling is preserved.
#[cfg(windows)]
unsafe extern "system" fn keep_caption_active_subclass(
    hwnd: windows_sys::Win32::Foundation::HWND,
    umsg: u32,
    wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
    _uidsubclass: usize,
    _dwrefdata: usize,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::UI::Shell::DefSubclassProc;
    use windows_sys::Win32::UI::WindowsAndMessaging::WM_NCACTIVATE;
    if umsg == WM_NCACTIVATE {
        return DefSubclassProc(hwnd, umsg, 1 as _, lparam);
    }
    DefSubclassProc(hwnd, umsg, wparam, lparam)
}

#[cfg(windows)]
const KEEP_ACTIVE_SUBCLASS_ID: usize = 1;
/// Global log filter: keep `level` for our crates, but silence the noisy
/// per-rebuild "Initializing egui-shadcn theme" INFO from the theme crate
/// (the live theme rebuilds every frame while scrubbing the color picker).
fn theme_log_filter(level: Level) -> Targets {
    Targets::new()
        .with_default(level)
        .with_target("egui_shadcn", Level::WARN)
        .with_target("symphonia_format_isomp4", Level::WARN)
        .with_target("symphonia_core", Level::WARN)
        .with_target("symphonia_bundle_mp3", Level::WARN)
}

/// Initialize logging based on build type.
///
/// - Debug builds: Log to console (stdout)
/// - Release builds: Log to session files in logs/ directory
///
/// Returns a guard that must be kept alive for the duration of the program
/// to ensure logs are flushed properly.
fn init_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let level = if cfg!(debug_assertions) {
        Level::DEBUG
    } else {
        Level::INFO
    };

    if cfg!(debug_assertions) {
        // Debug build: log to console
        tracing_subscriber::registry()
            .with(
                fmt::layer()
                    .with_target(false)
                    .with_thread_ids(false)
                    .compact(),
            )
            .with(theme_log_filter(level))
            .init();
        None
    } else {
        // Release build: log to file
        let log_dir = get_log_directory();

        // Create logs directory if it doesn't exist
        if let Err(e) = fs::create_dir_all(&log_dir) {
            eprintln!("Failed to create log directory: {}", e);
            return None;
        }

        // Clean up old log files (keep last 10 sessions)
        cleanup_old_logs(&log_dir, 10);

        // Create session log file with timestamp
        let timestamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
        let log_filename = format!("RealmHound_{}.log", timestamp);

        let file_appender = tracing_appender::rolling::never(&log_dir, &log_filename);
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

        tracing_subscriber::registry()
            .with(
                fmt::layer()
                    .with_writer(non_blocking)
                    .with_target(false)
                    .with_thread_ids(false)
                    .with_ansi(false), // No ANSI colors in file
            )
            .with(theme_log_filter(level))
            .init();

        Some(guard)
    }
}

/// Get the directory for log files.
fn get_log_directory() -> std::path::PathBuf {
    if let Some(data_dir) = dirs::data_local_dir() {
        data_dir.join("RealmHound").join("logs")
    } else {
        // Fallback to current directory
        std::path::PathBuf::from("logs")
    }
}

/// Remove old log files, keeping only the most recent `keep_count` files.
fn cleanup_old_logs(log_dir: &std::path::Path, keep_count: usize) {
    let Ok(entries) = fs::read_dir(log_dir) else {
        return;
    };

    // Collect log files with their modification times
    let mut log_files: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "log")
                .unwrap_or(false)
        })
        .filter_map(|e| {
            e.metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|time| (e.path(), time))
        })
        .collect();

    // Sort by modification time (newest first)
    log_files.sort_by(|a, b| b.1.cmp(&a.1));

    // Remove old files beyond keep_count
    for (path, _) in log_files.into_iter().skip(keep_count) {
        let _ = fs::remove_file(path);
    }
}

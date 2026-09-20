//! Main application module.

/// Application version from Cargo.toml
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const PKG_NAME: &str = env!("CARGO_PKG_NAME");
pub const PKG_AUTHORS: &str = env!("CARGO_PKG_AUTHORS");
pub const PKG_DESCRIPTION: &str = env!("CARGO_PKG_DESCRIPTION");
const PROJECT_LICENSE: &str = include_str!("../../../../LICENSE");
const THIRD_PARTY_LICENSES: &str = include_str!("../../../../THIRD-PARTY-LICENSES.md");

use crate::audio::{AudioHandle, AudioSender};
use crate::capture_manager::CaptureManager;
use crate::modals::{ModalState, SettingsCategory};
use crate::panels::characters::CharactersPanel;
use crate::panels::chat::ChatPanel;
use crate::panels::combat_history::CombatHistoryPanel;
use crate::panels::exaltations::ExaltationsPanel;
use crate::panels::live_feed::LiveFeedPanel;
use crate::panels::loot::LootPanel;
use crate::panels::party::PartyPanel;
use crate::panels::pet_yard::PetYardPanel;
use crate::panels::quest::QuestPanel;
use crate::panels::treasury::TreasuryPanel;
use crate::panels::trophy_hall::TrophyHallPanel;
use crate::panels::update::UpdatePanel;
use crate::panels::vault::VaultPanel;
use crate::panels::{ActiveTab, AppAction, Panel, PanelContext};
use crate::processing::contract::relaunch_preflight;
use crate::processing::{
    spawn_worker, AccountOperationScope, AudioCommand, ControlMsg, PacketProcessor, UiPayload,
    UiUpdate, ViewState, WorkerHandle,
};
use crate::rendering::{EmbeddedIcon, SpriteRenderer};
use crate::sound::{custom_sounds_dir, SoundType};
use crate::tab_icons::get_tab_icon;
use crate::ui_ext::HoverTooltipExt;
use eframe::egui::{self, Color32, RichText, Visuals};
use realmhound_core::{
    account::{AccountContext, AccountPersistencePaths, AccountRegistryStore, SavedCredential},
    account_stats::{ForgeMaterialTier, MaterialAmounts},
    assets::get_asset_manager,
    combat::CombatDatabase,
    dust::{DustAmounts, DustState, DustType},
    loot::LootDatabase,
    relaunch::{spawn_replacement, RelaunchReason},
    session::MainAccountStatus,
    storage::StorageError,
    update::rollback_executable_swap,
    update::SelfUpdater,
    update::UpdateChecker,
    vault::{AccountData, LiveVaultData},
    GameSession, Settings,
};
use std::fs;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, RwLock};
#[cfg(feature = "latency-diagnostics")]
use std::time::{Duration, Instant};

// MainAccountStatus is now in realmhound_core::session

/// Fixed column widths (px) for the realm-event notification rows so labels,
/// sliders, percentages, and buttons line up across every card.
mod event_row {
    /// Width reserved for the `NN%` percentage label.
    pub const PCT_W: f32 = 46.0;
    /// Width reserved for the tier (I-IV) selector column in tiered enchant rows,
    /// so rows without a selector (e.g. "Default volume:") reserve the same gap
    /// and keep every slider in one column.
    pub const TIER_W: f32 = 66.0;
    /// Width reserved for the `📂` custom-sound picker button.
    pub const PICKER_W: f32 = 30.0;
    /// Icon box size for the leading boss sprite.
    pub const ICON: f32 = 20.0;
    /// Upper bound for the auto-measured event-label column width.
    pub const LABEL_MAX: f32 = 260.0;
    /// Row height for a single-line row (no custom sound).
    pub const ROW_H: f32 = 30.0;
}

/// Leading icon kind drawn beside an entry in a search picker / slang editor.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PickerIcon {
    None,
    Portal,
    Encounter,
}

/// Truncate `s` to at most `max` characters, appending an ellipsis when cut.
fn truncate_ellipsis(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{}…", head)
    }
}

fn format_relative_time(
    now: chrono::DateTime<chrono::Utc>,
    then: chrono::DateTime<chrono::Utc>,
) -> String {
    let delta = now.signed_duration_since(then);
    let mins = delta.num_minutes();
    if mins < 1 {
        "just now".to_string()
    } else if mins < 60 {
        format!("{mins}m ago")
    } else if delta.num_hours() < 24 {
        format!("{}h ago", delta.num_hours())
    } else {
        format!("{}d ago", delta.num_days())
    }
}

/// Open a folder in the system file explorer.
#[cfg(windows)]
fn open_in_file_explorer(path: &std::path::Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;

    let operation: Vec<u16> = "explore\0".encode_utf16().collect();
    let wide_path: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        ShellExecuteW(
            0,
            operation.as_ptr(),
            wide_path.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        );
    }
}

#[cfg(not(windows))]
fn open_in_file_explorer(_path: &std::path::Path) {}

/// Roman-numeral label (I-IV) for an enchantment tier number 1-4.
fn roman_tier(tier: u8) -> &'static str {
    match tier {
        1 => "I",
        2 => "II",
        3 => "III",
        4 => "IV",
        _ => "?",
    }
}

/// A compact tier (I-IV) dropdown for a tiered enchant row.
///
/// Built from an `egui::Area` at `Order::Tooltip` with real button widgets
/// rather than a shadcn/egui select. Those selects draw their popup with raw
/// painters and manual hit-testing, so the popup never registers an interactive
/// rect: the click that selects an option falls through to whatever widget sits
/// behind it (e.g. another row's tier trigger). Real buttons on a top layer
/// consume the click, and `Order::Tooltip` keeps the popup above the settings
/// dialog. Returns `Some(tier)` when the user picks a tier.
fn tier_dropdown(ui: &mut egui::Ui, id_source: &str, current: u8) -> Option<u8> {
    let popup_id = ui.make_persistent_id(("rh_tier_popup", id_source));
    let mut open = ui.data(|d| d.get_temp::<bool>(popup_id).unwrap_or(false));

    let trigger = ui.add_sized(
        egui::vec2(event_row::TIER_W, 24.0),
        egui::Button::new(format!("{}  \u{25be}", roman_tier(current))),
    );
    if trigger.clicked() {
        open = !open;
    }

    let mut chosen: Option<u8> = None;
    if open {
        let area = egui::Area::new(popup_id)
            .fixed_pos(trigger.rect.left_bottom() + egui::vec2(0.0, 2.0))
            .order(egui::Order::Tooltip)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(trigger.rect.width());
                    for t in 1u8..=4 {
                        if ui
                            .add_sized(
                                egui::vec2(ui.available_width().max(trigger.rect.width()), 22.0),
                                egui::Button::new(roman_tier(t)).selected(t == current),
                            )
                            .clicked()
                        {
                            chosen = Some(t);
                        }
                    }
                });
            });
        ui.ctx().move_to_top(area.response.layer_id);

        if chosen.is_some() {
            open = false;
        } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            open = false;
        } else if ui.input(|i| i.pointer.any_click()) {
            let inside = ui
                .input(|i| i.pointer.interact_pos())
                .is_some_and(|p| trigger.rect.contains(p) || area.response.rect.contains(p));
            if !inside {
                open = false;
            }
        }
    }

    ui.data_mut(|d| d.insert_temp(popup_id, open));
    chosen
}

/// A generic on-top dropdown for use inside the settings dialog.
///
/// Same rationale as [`tier_dropdown`]: a raw `egui::ComboBox`/shadcn select
/// draws its popup below the settings dialog and its clicks fall through to the
/// widget behind it, so the selector appears unresponsive. This renders the
/// popup as real `Button` widgets in an `egui::Area` at `Order::Tooltip`, which
/// shows above the dialog and consumes its own clicks. Disabled options are
/// shown greyed and are not selectable. Returns the picked option index.
fn on_top_dropdown(
    ui: &mut egui::Ui,
    id_source: &str,
    width: f32,
    current: usize,
    options: &[(&str, bool)],
) -> Option<usize> {
    let popup_id = ui.make_persistent_id(("rh_dropdown", id_source));
    let mut open = ui.data(|d| d.get_temp::<bool>(popup_id).unwrap_or(false));

    let selected_text = options.get(current).map(|(l, _)| *l).unwrap_or("");
    let trigger = ui.add_sized(
        egui::vec2(width, 24.0),
        egui::Button::new(format!("{}  \u{25be}", selected_text)),
    );
    if trigger.clicked() {
        open = !open;
    }

    let mut chosen: Option<usize> = None;
    if open {
        let area = egui::Area::new(popup_id)
            .fixed_pos(trigger.rect.left_bottom() + egui::vec2(0.0, 2.0))
            .order(egui::Order::Tooltip)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    let w = trigger.rect.width();
                    ui.set_min_width(w);
                    for (i, (label, enabled)) in options.iter().enumerate() {
                        let btn = ui.add_enabled(
                            *enabled,
                            egui::Button::new(*label)
                                .selected(i == current)
                                .min_size(egui::vec2(w, 22.0)),
                        );
                        if btn.clicked() {
                            chosen = Some(i);
                        }
                    }
                });
            });
        ui.ctx().move_to_top(area.response.layer_id);

        if chosen.is_some() {
            open = false;
        } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            open = false;
        } else if ui.input(|i| i.pointer.any_click()) {
            let inside = ui
                .input(|i| i.pointer.interact_pos())
                .is_some_and(|p| trigger.rect.contains(p) || area.response.rect.contains(p));
            if !inside {
                open = false;
            }
        }
    }

    ui.data_mut(|d| d.insert_temp(popup_id, open));
    chosen
}

/// Lay out `add` inside a fixed-size cell (fixed `width`, fixed `height`) with
/// its content vertically centered, so following widgets start at a predictable
/// x and share the row's vertical midline. Returns the closure's value.
fn fixed_cell<R>(
    ui: &mut egui::Ui,
    width: f32,
    height: f32,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_size(egui::vec2(width, height));
            ui.set_max_width(width);
            // Zero horizontal spacing so the leading spacer below does not push
            // the content right; cells add their own gaps as needed.
            ui.spacing_mut().item_spacing.x = 0.0;
            // A zero-width, full-height spacer establishes the row height up
            // front so short content (a label, icon) is vertically centered
            // rather than pinned to the top.
            ui.allocate_exact_size(egui::vec2(0.0, height), egui::Sense::hover());
            add(ui)
        },
    )
    .inner
}

/// Cache key for the projected Taskbar items. When every field matches the
/// previous frame the cached `taskbar_items_cache` is reused instead of
/// reprojecting missions/quests (and their tooltips) again.
#[derive(PartialEq)]
struct TaskbarCacheSig {
    mission_gen: u64,
    account_revision: u64,
    quests_revision: u64,
    live_char_id: Option<i32>,
    hidden_missions: std::collections::HashSet<i64>,
    taskbar: realmhound_core::settings::TaskbarSettings,
}

/// A recoverable failure while constructing the selected-profile application.
#[derive(Debug)]
pub enum AppStartError {
    /// A validated storage operation failed.
    Storage(StorageError),
    /// The selected account snapshot could not be loaded (e.g. corrupt).
    AccountData(String),
    /// The selected profile database writers could not be opened.
    Processor(String),
    /// A selected profile history database reader could not be opened after its
    /// strict writer initialized (a damaged/inaccessible profile).
    Reader(String),
    /// The processing worker thread could not be created.
    Worker(String),
    /// The validated-launch metadata could not be recorded.
    LaunchMetadata(String),
}

impl std::fmt::Display for AppStartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppStartError::Storage(error) => write!(f, "storage error: {error}"),
            AppStartError::AccountData(detail) => {
                write!(f, "the account snapshot could not be loaded: {detail}")
            }
            AppStartError::Processor(detail) => {
                write!(
                    f,
                    "the account history databases could not be opened: {detail}"
                )
            }
            AppStartError::Reader(detail) => {
                write!(
                    f,
                    "the account history databases could not be read: {detail}"
                )
            }
            AppStartError::Worker(detail) => {
                write!(f, "the processing worker could not start: {detail}")
            }
            AppStartError::LaunchMetadata(detail) => {
                write!(f, "launch metadata could not be recorded: {detail}")
            }
        }
    }
}

impl std::error::Error for AppStartError {}

impl From<StorageError> for AppStartError {
    fn from(error: StorageError) -> Self {
        AppStartError::Storage(error)
    }
}

/// Per-operation-kind request generations for async account refreshes.
///
/// Each async request kind takes a strictly increasing generation at dispatch;
/// the worker echoes it back on completion and applies a result only when it is
/// newer than the last one it applied, so stale or out-of-order completions are
/// rejected. Values only ever climb within a process (account switching is
/// restart-based), so they never collide with a prior account's generations.
#[derive(Debug, Default, Clone, Copy)]
struct OperationGenerations {
    character: u64,
    mission: u64,
}

impl OperationGenerations {
    fn next_character(&mut self) -> u64 {
        self.character += 1;
        self.character
    }

    fn next_mission(&mut self) -> u64 {
        self.mission += 1;
        self.mission
    }
}

#[cfg(feature = "latency-diagnostics")]
#[derive(Default)]
struct UiHealth {
    started: Option<Instant>,
    updates: u64,
    capped_drain_frames: u64,
    peak_oldest_wait: Duration,
    peak_remaining: usize,
    peak_frame_gap: Duration,
}

#[cfg(feature = "latency-diagnostics")]
struct UiHealthSummary {
    window: Duration,
    updates: u64,
    capped_drain_frames: u64,
    peak_oldest_wait: Duration,
    peak_remaining: usize,
    peak_frame_gap: Duration,
}

#[cfg(feature = "latency-diagnostics")]
impl UiHealth {
    fn record(
        &mut self,
        now: Instant,
        updates: usize,
        capped: bool,
        oldest_wait: Duration,
        remaining: usize,
        frame_gap: Duration,
    ) -> Option<UiHealthSummary> {
        if updates == 0 {
            return None;
        }
        let started = *self.started.get_or_insert(now);
        self.updates = self.updates.saturating_add(updates as u64);
        self.capped_drain_frames = self.capped_drain_frames.saturating_add(u64::from(capped));
        self.peak_oldest_wait = self.peak_oldest_wait.max(oldest_wait);
        self.peak_remaining = self.peak_remaining.max(remaining);
        self.peak_frame_gap = self.peak_frame_gap.max(frame_gap);
        let window = now.duration_since(started);
        if window < Duration::from_secs(30) {
            return None;
        }
        let summary = UiHealthSummary {
            window,
            updates: self.updates,
            capped_drain_frames: self.capped_drain_frames,
            peak_oldest_wait: self.peak_oldest_wait,
            peak_remaining: self.peak_remaining,
            peak_frame_gap: self.peak_frame_gap,
        };
        *self = Self::default();
        Some(summary)
    }
}

#[cfg(all(test, feature = "latency-diagnostics"))]
mod ui_health_tests {
    use super::*;

    #[test]
    fn ui_health_requires_updates_and_resets_after_actual_window() {
        let now = Instant::now();
        let mut health = UiHealth::default();
        assert!(health
            .record(now, 0, false, Duration::ZERO, 0, Duration::ZERO)
            .is_none());
        assert!(health
            .record(
                now,
                3,
                false,
                Duration::from_millis(20),
                2,
                Duration::from_millis(16),
            )
            .is_none());
        let summary = health
            .record(
                now + Duration::from_secs(34),
                4,
                true,
                Duration::from_millis(40),
                5,
                Duration::from_millis(80),
            )
            .unwrap();
        assert_eq!(summary.window, Duration::from_secs(34));
        assert_eq!(summary.updates, 7);
        assert_eq!(summary.capped_drain_frames, 1);
        assert_eq!(summary.peak_remaining, 5);
        assert_eq!(summary.peak_frame_gap, Duration::from_millis(80));
        assert!(health.started.is_none());
    }
}

/// Main application state.
pub struct RealmHoundApp {
    /// Packet capture lifecycle manager
    capture: CaptureManager,
    /// Active tab
    active_tab: ActiveTab,
    /// Active tab on the previous frame, used to detect tab activation so a
    /// panel can reset transient view state (e.g. scroll position) on entry.
    last_active_tab: ActiveTab,
    /// Chat panel
    chat_panel: ChatPanel,
    /// Party panel
    party_panel: PartyPanel,
    /// Live feed panel (unified activity stream)
    live_feed_panel: LiveFeedPanel,
    /// Characters panel (grid layout with stats modal)
    characters_panel: CharactersPanel,
    /// Exaltations panel (account-wide exaltation overview by class)
    exaltations_panel: ExaltationsPanel,
    /// Vault panel
    vault_panel: VaultPanel,
    /// Quest panel
    quest_panel: QuestPanel,
    /// Loot panel
    loot_panel: LootPanel,
    /// Combat History panel
    combat_panel: CombatHistoryPanel,
    /// Treasury panel (consolidated view of all owned items)
    treasury_panel: TreasuryPanel,
    /// Trophy Hall panel (item collections unified across loot history, RealmShark, and account data)
    trophy_hall_panel: TrophyHallPanel,
    /// Pet Yard diagnostics panel (read-only pet identity/inventory cache view)
    pet_yard_panel: PetYardPanel,
    /// Seasonal missions panel.
    missions_panel: crate::panels::missions::MissionsPanel,
    /// Captured access token from HELLO packet (for API calls)
    captured_access_token: Option<String>,
    /// Time the current access token was captured (if known). Set to now on a
    /// fresh live capture; loaded from disk otherwise so a deferred persist keeps
    /// the real capture time.
    token_captured_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Whether we've already fetched exalt stats (only do once per app session)
    exalts_fetched: bool,
    /// Handle to the packet-processing worker thread (owns the model: session,
    /// reassembler, router, loot tracker, account/vault data).
    worker: WorkerHandle,
    /// Active-tab signal shared with the worker so it only wakes the UI for
    /// updates the current tab (or a global widget) displays. Written at the end
    /// of each frame to reflect the tab actually rendered.
    active_tab_signal: Arc<AtomicU8>,
    /// Audio thread handle (owns the `!Send` `SoundEngine`). Kept alive so the
    /// thread isn't torn down; dropped on app exit.
    audio: AudioHandle,
    /// Sender for audio commands (volume slider, test button).
    audio_tx: AudioSender,
    #[cfg(feature = "latency-diagnostics")]
    latency_last_frame: Instant,
    #[cfg(feature = "latency-diagnostics")]
    latency_last_ui_warn: Option<Instant>,
    #[cfg(feature = "latency-diagnostics")]
    latency_ui_health: UiHealth,
    /// Owned snapshot of the worker model, refreshed each frame from the shared
    /// snapshot. Panels read from these (the worker is the source of truth).
    account_data: AccountData,
    /// Owned snapshot of live vault data.
    live_vault_data: LiveVaultData,
    /// Owned snapshot of the currently playing character id.
    live_char_id: Option<i32>,
    /// Last character id seen live, retained so the Current-Character widget can
    /// keep showing (dimmed) the last-seen character after logout.
    last_live_char_id: Option<i32>,
    /// Owned snapshot of the verified main game-client connection socket. Used
    /// to map the connection to its owning OS process for the forge-fire
    /// relaunch safeguard.
    main_connection: Option<realmhound_core::stream::ConnectionKey>,
    /// Last time a client-launch check ran, to throttle the OS process lookup.
    last_launch_check_at: Option<std::time::Instant>,
    /// Owned snapshot of the coalesced view state (connection + generations).
    view_state: ViewState,
    /// Read-only loot DB connection for the loot history panel (worker owns the
    /// writer; this is a separate WAL reader connection).
    loot_reader: Option<LootDatabase>,
    /// Read-only combat history DB connection (worker owns the writer). Opened
    /// strictly at startup after the writer initializes; a failure aborts startup.
    combat_reader: Option<CombatDatabase>,
    /// Account-scoped persistence paths resolved once at startup and shared with
    /// the worker. Used here for the UI reader retry and the short-lived writers
    /// behind the clear/delete flows.
    persistence: AccountPersistencePaths,
    /// The immutable selected-account context for the process lifetime. Holds the
    /// exclusive profile lock (shared with the worker) and is the authority for
    /// account identity, persistence paths, and credentials.
    account: Arc<AccountContext>,
    /// Per-operation-kind request generations for async account refreshes. Each
    /// dispatch takes a strictly increasing value that the worker echoes back so
    /// stale/out-of-order completions can be rejected. Monotonic for the process
    /// lifetime (account switching is restart-based), so they only ever climb.
    account_op_generations: OperationGenerations,
    /// Cached last confirmed game-client launch (Unix seconds) for the selected
    /// profile. Seeded from the registry entry and updated in lock-step with the
    /// atomic registry write, so the forge-fire gate reads it without disk I/O.
    client_launch_unix: i64,
    /// A default session passed to transient panels' `handle_event` (they ignore
    /// it; the worker owns the real session).
    event_session: GameSession,
    /// Sprite renderer for item icons (shared across panels)
    sprite_renderer: SpriteRenderer,
    /// Per-section autocomplete state for the realm-event boss picker,
    /// keyed by section prefix ("veteran"/"adept"/"seasonal").
    event_pickers:
        std::collections::HashMap<&'static str, crate::realm_event_sound::EventPickerState>,
    /// User settings (shared, thread-safe)
    settings: Arc<RwLock<Settings>>,
    /// Modal/dialog visibility state
    modals: ModalState,
    /// Update checker for version notifications
    update_checker: UpdateChecker,
    /// Self-updater for in-place exe updates
    self_updater: SelfUpdater,
    /// Set when a relaunch is requested; polled each frame and executed once.
    pending_relaunch: Option<RelaunchReason>,
    /// Update notification panel
    update_panel: UpdatePanel,
    /// Last synced character cache generation (avoids cloning every frame)
    treasury_cache_generation: u64,
    /// Last applied characters-mirror generation (avoids cloning every frame)
    last_char_view_gen: u64,
    /// Last applied live-vault generation (avoids cloning every frame)
    last_vault_view_gen: u64,
    /// Owned-snapshot epoch: last (char_view_gen, account_generation) cloned into
    /// `account_data` from the worker snapshot.
    owned_account_epoch: (u64, u64),
    /// Monotonic revision bumped whenever `account_data` is re-cloned. Exposed as
    /// `PanelContext::account_epoch` for use as a memo key.
    account_data_revision: u64,
    /// Owned-snapshot epoch: last vault_view_gen cloned into `live_vault_data`.
    owned_vault_gen: u64,
    /// Owned snapshot of the seasonal mission tracker view.
    mission_view: crate::processing::MissionView,
    /// Last applied mission_view generation (avoids cloning every frame).
    owned_mission_gen: u64,
    /// Cached Taskbar items, rebuilt only when their inputs change so the row
    /// (and its tooltips) is not reprojected every frame.
    taskbar_items_cache: Vec<crate::panels::taskbar::TaskbarItem>,
    /// Signature of the inputs that produced `taskbar_items_cache`.
    taskbar_cache_sig: Option<TaskbarCacheSig>,
    /// Themed shadcn widget helpers (owns the design-token theme).
    shadcn: crate::shadcn_ui::Shadcn,
    /// Current theme selection (preset or custom).
    theme_selection: crate::shadcn_ui::ThemeSelection,
    /// Selection last written to disk; the live theme rebuilds instantly for
    /// preview, but we only persist once the user stops scrubbing the picker.
    persisted_theme_selection: crate::shadcn_ui::ThemeSelection,
    /// Last-used custom brand color and mode (remembered while a preset is
    /// active so switching back to "Custom" restores them).
    custom_primary: egui::Color32,
    custom_mode: crate::shadcn_ui::Mode,
    /// Widget Bar drag-reorder: the widget kind currently being dragged.
    widget_drag: Option<realmhound_core::settings::WidgetKind>,
    /// Widget Bar drag-reorder: the computed drop target (row + index) during a drag.
    widget_drop: Option<WidgetDropTarget>,
    /// Widget Bar: cached intrinsic chip width per kind, measured from the last
    /// rendered frame and used to decide row-1 -> row-2 spill on the next frame.
    widget_widths: std::collections::HashMap<realmhound_core::settings::WidgetKind, f32>,
    /// Currently selected sub-tab within the Sound settings panel.
    sound_sub_tab: SoundSubTab,
    /// Account key of a pending switch confirmation.
    switch_confirm_target: Option<realmhound_core::account::AccountKey>,
    /// Cached registry entries for the profile list, refreshed on settings open.
    cached_registry_entries: Vec<realmhound_core::account::AccountRegistryEntry>,
    /// Whether the registry cache has been loaded this settings-open cycle.
    registry_cache_loaded: bool,
    /// Inline error shown when a switch validation fails.
    switch_error: Option<String>,
    /// Whether the discovery confirmation card is active.
    discovery_confirm_active: bool,
    /// Inline error shown when a discovery attempt fails.
    discovery_error: Option<String>,
    /// Account key of a pending delete confirmation.
    delete_confirm_target: Option<realmhound_core::account::AccountKey>,
    /// Inline error shown when an account deletion fails.
    delete_error: Option<String>,
}

/// Sub-tabs of the Sound settings panel, splitting the previously crowded
/// single view into focused sections.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum SoundSubTab {
    #[default]
    General,
    LootBags,
    RealmBosses,
    Enchantments,
}

/// Where a dragged Widget Bar chip would be dropped: which row (0 = top,
/// 1 = bottom) and the widget it should be inserted before (`None` = end of row).
#[derive(Clone, Copy)]
struct WidgetDropTarget {
    row: usize,
    before: Option<realmhound_core::settings::WidgetKind>,
}

/// Object type of the "3D Built Block" sprite drawn as the Widget Bar's leading
/// marker (game object `id="3D Built Block" type="0xbb19"`).
const WIDGET_BAR_ICON_ID: i32 = 0xbb19;
/// Cell width the Widget Bar's leading marker occupies (matches the Taskbar
/// compass cell), including no trailing gap.
const WIDGET_BAR_ICON_CELL: f32 = 22.0;

/// Precomputed, immutable values shared by every Widget Bar chip in a frame, so
/// a single chip can be rendered by [`RealmHoundApp::render_widget_chip`] without
/// re-reading locks or account data per widget.
struct WidgetChipCtx {
    bg: Color32,
    border: Color32,
    value_color: Color32,
    label_color: Color32,
    is_seasonal: bool,
    dust_mode: realmhound_core::settings::DustDisplayMode,
    materials_mode: realmhound_core::settings::MaterialsDisplayMode,
    stats: realmhound_core::account_stats::AccountStats,
    season_cfg: realmhound_core::settings::SeasonConfig,
    account_name: Option<String>,
    max_num_chars: i32,
    skins: i32,
    alive_count: i32,
    playtime: i32,
    now: chrono::DateTime<chrono::Utc>,
}

// PendingCreate is now in realmhound_core::session
// SettingsCategory and ModalState are in modal_state.rs

impl RealmHoundApp {
    /// Open the selected profile's read-only history connections.
    ///
    /// Called after the strict writers create each database, so a reader failure
    /// here means a damaged/inaccessible profile and is propagated (never
    /// swallowed or silently retried, which would hide a broken profile).
    fn open_selected_readers(
        loot_path: &std::path::Path,
        combat_path: &std::path::Path,
    ) -> Result<(Option<LootDatabase>, Option<CombatDatabase>), AppStartError> {
        let loot_reader = LootDatabase::open_reader_at(loot_path)
            .map_err(|e| AppStartError::Reader(format!("loot: {e}")))?;
        let combat_reader = CombatDatabase::open_reader_at(combat_path)
            .map_err(|e| AppStartError::Reader(format!("combat: {e}")))?;
        Ok((Some(loot_reader), Some(combat_reader)))
    }

    /// Create a new application instance bound to one validated, exclusively
    /// locked account profile.
    ///
    /// Every account-scoped resource is derived from `account` (the single
    /// immutable [`AccountContext`]); no component resolves a persistence path
    /// or credential itself, and the flat legacy layout is never read or written.
    pub fn new(
        egui_ctx: &egui::Context,
        settings: Arc<RwLock<Settings>>,
        account: Arc<AccountContext>,
    ) -> Result<Self, AppStartError> {
        // Set dark theme
        egui_ctx.set_visuals(Self::dark_visuals());

        // Configure custom fonts with emoji support
        let mut fonts = egui::FontDefinitions::default();

        // Add Noto Sans as the primary font
        fonts.font_data.insert(
            "NotoSans".to_owned(),
            egui::FontData::from_static(include_bytes!("../assets/NotoSans-Regular.ttf")).into(),
        );

        // Add Noto Emoji for emoji support
        fonts.font_data.insert(
            "NotoEmoji".to_owned(),
            egui::FontData::from_static(include_bytes!("../assets/NotoEmoji-Regular.ttf")).into(),
        );

        // Try to load Segoe UI Symbol from Windows system fonts for arrows/symbols
        // This is legal - we're using the system font, not redistributing it
        let segoe_symbol_loaded =
            if let Ok(font_data) = fs::read("C:\\Windows\\Fonts\\seguisym.ttf") {
                fonts.font_data.insert(
                    "SegoeUISymbol".to_owned(),
                    egui::FontData::from_owned(font_data).into(),
                );
                tracing::info!("[FONTS] Loaded Segoe UI Symbol from system fonts");
                true
            } else {
                tracing::warn!("[FONTS] Segoe UI Symbol not found, some symbols may not display");
                false
            };

        // Set Noto Sans as the primary proportional font, with symbols and emoji as fallback
        if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
            family.insert(0, "NotoSans".to_owned());
            if segoe_symbol_loaded {
                family.push("SegoeUISymbol".to_owned());
            }
            family.push("NotoEmoji".to_owned());
        }

        // Also add symbols and emoji support to monospace family as fallback
        if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
            if segoe_symbol_loaded {
                family.push("SegoeUISymbol".to_owned());
            }
            family.push("NotoEmoji".to_owned());
        }

        egui_ctx.set_fonts(fonts);

        // Initialize assets (extracts from game if needed)
        match get_asset_manager().initialize(Some(&settings)) {
            Ok(_) => {
                let stats = get_asset_manager().stats();
                tracing::info!(
                    "[ASSETS] Loaded {} objects, {} tiles, {} sprites",
                    stats.objects_loaded,
                    stats.tiles_loaded,
                    stats.sprites_loaded
                );
            }
            Err(e) => {
                tracing::warn!("[ASSETS] Failed to load assets: {}", e);
            }
        };

        // Try to enumerate interfaces
        let interfaces =
            realmhound_core::capture::NetworkInterface::list_all().map_err(|e| e.to_string());

        // Every account-scoped path comes from the immutable account context.
        // Global installation paths (watchlist) stay under the storage root.
        let storage_root = account.storage_root().clone();
        let persistence = account.persistence().clone();
        let watchlist_path = storage_root.path().join("watchlist.txt");
        let account_repository = persistence.account_data_repository();

        // Create panels with settings reference
        let chat_panel = {
            let s = settings.read().unwrap();
            ChatPanel::new_with_settings(&s.chat, persistence.chat_logs().to_path_buf())
        };

        let loot_panel = {
            let s = settings.read().unwrap();
            let mut panel = LootPanel::new_with_settings(&s.loot);
            panel.set_database_paths(
                persistence.loot_database().to_path_buf(),
                persistence.combat_database().to_path_buf(),
            );
            panel
        };

        // The selected profile's verified identity is the runtime authority for
        // multi-client isolation, not the legacy `settings.account` (which is now
        // migration input/compat only). The display name comes from the registry.
        let saved_account_id = Some(account.account_id().to_string());
        let detected_account_name = account.display_name().map(str::to_string);
        // A selected profile always has a main account for RealmShark selection.
        let has_main_account = true;
        let trophy_realmshark_path = persistence.realmshark_import().to_path_buf();

        // Load persisted appearance/theme selection.
        let (theme_selection, custom_primary, custom_mode) = {
            use crate::shadcn_ui::{color_from_hex, Mode, ThemePreset, ThemeSelection};
            let s = settings.read().unwrap();
            let primary = color_from_hex(&s.appearance.custom_primary);
            let mode = Mode::from_key(&s.appearance.mode);
            let selection = if s.appearance.preset == "custom" {
                ThemeSelection::Custom { primary, mode }
            } else {
                ThemePreset::from_key(&s.appearance.preset)
                    .map(ThemeSelection::Preset)
                    .unwrap_or(ThemeSelection::Custom { primary, mode })
            };
            (selection, primary, mode)
        };
        let compact_ui = settings.read().unwrap().appearance.compact_ui;
        let mut shadcn = crate::shadcn_ui::Shadcn::from_selection(theme_selection);
        shadcn.set_compact(compact_ui);
        Self::apply_theme(egui_ctx, &shadcn);

        // Strict, non-destructive snapshot load: a corrupt snapshot is an error
        // (surfaced to startup), never silently replaced with empty state.
        let account_data = account
            .load_account_data_strict()
            .map_err(|e| AppStartError::AccountData(e.to_string()))?;
        tracing::info!(
            "[ACCOUNT] Loaded account data: {} chars, gen={}",
            account_data.characters.characters.len(),
            account_data.generation()
        );

        let mut live_feed_panel = LiveFeedPanel::new();
        live_feed_panel.init_dust_from_account_data(&account_data);
        if let Ok(s) = settings.read() {
            live_feed_panel.apply_live_feed_settings(&s.live_feed);
            live_feed_panel.apply_season_config(&s.season);
        }

        let mut party_panel = PartyPanel::new(
            watchlist_path,
            Some(persistence.watchlist_detections().to_path_buf()),
        );
        if let Ok(s) = settings.read() {
            party_panel.apply_party_settings(&s.party);
        }

        // Live vault data initialized from account_data (single source of truth).
        let live_vault_data = LiveVaultData {
            regular: account_data.regular_vault.clone(),
            seasonal: account_data.seasonal_vault.clone(),
        };

        let mut characters_panel = CharactersPanel::with_cache(account_data.characters.clone());
        if let Ok(s) = settings.read() {
            characters_panel.apply_settings(&s.characters);
        }
        let vault_panel = VaultPanel::with_live_vault_data(
            live_vault_data.clone(),
            persistence.char_list_cache().to_path_buf(),
        );
        let mut missions_panel = crate::panels::missions::MissionsPanel::new();
        if let Ok(s) = settings.read() {
            missions_panel.apply_settings(&s.missions);
            missions_panel.apply_taskbar_settings(&s.taskbar);
        }
        let mut quest_panel = QuestPanel::new(persistence.quests().to_path_buf());
        if let Ok(s) = settings.read() {
            quest_panel.apply_settings(&s.quests);
            quest_panel.apply_taskbar_settings(&s.taskbar);
        }

        // Build the packet processor for the selected profile. Database writers
        // open before any UI reader (combat before loot, since the loot backfill
        // consults the combat DB), and a writer failure propagates instead of
        // degrading to in-memory.
        let loot_db_path = persistence.loot_database().to_path_buf();
        let combat_db_path = persistence.combat_database().to_path_buf();
        let processor = PacketProcessor::new_selected(
            settings.clone(),
            account_data,
            account_repository,
            loot_db_path.clone(),
            combat_db_path.clone(),
            saved_account_id,
            detected_account_name,
        )
        .map_err(|e| AppStartError::Processor(e.to_string()))?;

        // Open the read-only history connections for the UI. The strict writers
        // above created each file + schema + WAL, so a reader failure signals a
        // damaged/inaccessible profile and propagates instead of being swallowed.
        let (loot_reader, combat_reader) =
            Self::open_selected_readers(&loot_db_path, &combat_db_path)?;

        // Spawn the audio thread (owns the !Send SoundEngine).
        let audio = crate::audio::spawn(settings.clone());
        let audio_tx = audio.sender();

        // Spawn the processing worker staged/inactive: it holds the account
        // context (and its lock) but processes and persists nothing until it is
        // activated. Creating the thread must succeed before any launch metadata
        // is recorded, so a spawn failure counts no launch.
        let active_tab_signal = Arc::new(AtomicU8::new(ActiveTab::LiveFeed as u8));
        let worker = spawn_worker(
            processor,
            egui_ctx.clone(),
            audio_tx.clone(),
            active_tab_signal.clone(),
            account.clone(),
        )
        .map_err(|e| AppStartError::Worker(e.to_string()))?;

        // Record launch metadata only after the worker exists but before it is
        // activated: last-opened first, then the retention increment. Any failure
        // cancels and joins the inactive worker (no detached lock holder, no
        // persistence) so an aborted startup counts zero launches.
        let registry_store = AccountRegistryStore::new(account.storage_root().clone());
        let client_launch_unix =
            match registry_store.update_last_opened(account.account_key(), chrono::Utc::now()) {
                Ok(entry) => entry.last_client_launch_unix(),
                Err(e) => {
                    worker.cancel_staged();
                    return Err(AppStartError::LaunchMetadata(e.to_string()));
                }
            };
        if let Err(e) = account.record_validated_launch() {
            worker.cancel_staged();
            return Err(AppStartError::LaunchMetadata(e.to_string()));
        }

        // Metadata is durable: activate the worker so it begins processing.
        worker.activate();

        // Seed owned snapshots from the worker's initial published snapshot.
        let (init_view, init_account, init_vault, init_live_char) = {
            let snap = worker.snapshot.lock().unwrap();
            (
                snap.view.clone(),
                snap.account_data.clone(),
                snap.live_vault_data.clone(),
                snap.live_char_id,
            )
        };

        // The selected credential comes only from the credential store, never
        // the flat `access_token.txt`. A store error is a non-blocking "no token".
        let saved_credential = account.read_credential();
        let captured_access_token = saved_credential.as_ref().map(|c| c.token().to_string());
        let token_captured_at = saved_credential.and_then(|c| c.captured_at());

        let mut app = Self {
            capture: CaptureManager::new(interfaces),
            active_tab: ActiveTab::LiveFeed, // Default to Live Feed tab
            last_active_tab: ActiveTab::LiveFeed,
            chat_panel,
            party_panel,
            live_feed_panel,
            characters_panel,
            exaltations_panel: ExaltationsPanel::new(),
            vault_panel,
            quest_panel,
            loot_panel,
            combat_panel: {
                let mut panel = CombatHistoryPanel::new();
                panel.set_database_path(persistence.combat_database().to_path_buf());
                panel
            },
            treasury_panel: TreasuryPanel::new(),
            trophy_hall_panel: {
                let mut panel = TrophyHallPanel::new();
                // Flat compatibility exposes RealmShark imports only for a
                // configured main account.
                if has_main_account {
                    panel.set_realmshark_import_path(Some(trophy_realmshark_path));
                }
                panel
            },
            pet_yard_panel: PetYardPanel::new(),
            missions_panel,
            captured_access_token,
            token_captured_at,
            exalts_fetched: false,
            worker,
            active_tab_signal,
            audio,
            audio_tx,
            #[cfg(feature = "latency-diagnostics")]
            latency_last_frame: Instant::now(),
            #[cfg(feature = "latency-diagnostics")]
            latency_last_ui_warn: None,
            #[cfg(feature = "latency-diagnostics")]
            latency_ui_health: UiHealth::default(),
            account_data: init_account,
            live_vault_data: init_vault,
            live_char_id: init_live_char,
            last_live_char_id: init_live_char.or_else(|| {
                settings
                    .read()
                    .ok()
                    .and_then(|s| s.characters.last_live_char_id)
            }),
            main_connection: None,
            last_launch_check_at: None,
            view_state: init_view,
            loot_reader,
            combat_reader,
            persistence,
            account,
            account_op_generations: OperationGenerations::default(),
            client_launch_unix,
            event_session: GameSession::new(),
            sprite_renderer: SpriteRenderer::new(),
            event_pickers: std::collections::HashMap::new(),
            settings: settings.clone(),
            modals: ModalState::new(),
            update_checker: UpdateChecker::new(),
            self_updater: SelfUpdater::new(),
            pending_relaunch: None,
            update_panel: UpdatePanel::new(),
            treasury_cache_generation: u64::MAX, // Force initial sync
            last_char_view_gen: u64::MAX,        // Force initial sync
            last_vault_view_gen: u64::MAX,       // Force initial sync
            owned_account_epoch: (u64::MAX, u64::MAX),
            account_data_revision: 0,
            owned_vault_gen: u64::MAX,
            mission_view: crate::processing::MissionView::default(),
            owned_mission_gen: 0,
            taskbar_items_cache: Vec::new(),
            taskbar_cache_sig: None,
            shadcn,
            theme_selection,
            persisted_theme_selection: theme_selection,
            custom_primary,
            custom_mode,
            widget_drag: None,
            widget_drop: None,
            widget_widths: std::collections::HashMap::new(),
            sound_sub_tab: SoundSubTab::default(),
            switch_confirm_target: None,
            cached_registry_entries: Vec::new(),
            registry_cache_loaded: false,
            switch_error: None,
            discovery_confirm_active: false,
            discovery_error: None,
            delete_confirm_target: None,
            delete_error: None,
        };

        // Apply persisted settings to panels. (Volume is applied by the audio
        // thread from shared settings at startup.)
        {
            let s = settings.read().unwrap();
            // Apply persisted treasury section order
            app.treasury_panel
                .apply_section_order_from_keys(&s.treasury.section_order);
            // Apply persisted sort mode
            if let Some(ref key) = s.treasury.sort_mode {
                if let Some(mode) = crate::panels::treasury::TreasurySortMode::from_key(key) {
                    app.treasury_panel.set_sort_mode(mode);
                }
            }
            // Apply persisted custom category order
            app.treasury_panel
                .apply_custom_category_order_from_keys(&s.treasury.custom_category_order);
            // Apply persisted tab-header display settings
            app.sprite_renderer
                .set_tab_display(s.appearance.tab_labels, s.appearance.tab_tooltips);
            // Apply persisted Trophy Hall data source so a RealmShark/Both choice
            // survives restarts (RealmShark data auto-loads on first refresh).
            if let Some(ref key) = s.trophy_hall.data_source {
                if let Some(src) = crate::panels::trophy_hall::DataSource::from_key(key) {
                    app.trophy_hall_panel.set_data_source(src);
                }
            }
            *app.trophy_hall_panel.show_no_collection_dungeons_mut() =
                s.trophy_hall.show_no_collection_dungeons;
            *app.trophy_hall_panel.compact_view_mut() = s.trophy_hall.compact_view;
        }

        // Forward cached vault data to treasury panel (vault panel loads its own cache)
        app.treasury_panel.set_live_vault_data(live_vault_data);

        // Auto-start packet capture
        app.start_capture();

        Ok(app)
    }

    /// Persist the selected account's access token through the credential store,
    /// keyed by the deterministic target. Never writes the legacy flat token file.
    fn save_account_credential(
        &self,
        token: &str,
        captured_at: Option<chrono::DateTime<chrono::Utc>>,
    ) {
        let credential = SavedCredential::new(token, captured_at);
        if let Err(e) = self
            .account
            .credentials()
            .write(self.account.credential_target(), &credential)
        {
            tracing::warn!("[TOKEN] Failed to store account credential: {e}");
        }
    }

    /// Update window state in settings from current egui context.
    fn update_window_state(&mut self, ctx: &egui::Context) {
        ctx.input(|i| {
            if let Some(rect) = i.viewport().inner_rect {
                let mut settings = match self.settings.write() {
                    Ok(s) => s,
                    Err(_) => return,
                };

                // Update size
                settings.window.width = rect.width();
                settings.window.height = rect.height();

                // Update position
                if let Some(pos) = i.viewport().outer_rect {
                    settings.window.x = Some(pos.min.x);
                    settings.window.y = Some(pos.min.y);
                }

                // Check maximized state
                if let Some(maximized) = i.viewport().maximized {
                    settings.window.maximized = maximized;
                }
            }
        });
    }

    /// Save loot filter settings and persist to disk.
    pub fn save_loot_settings(&self, filters: &crate::panels::loot_filters::LootFilters) {
        if let Ok(mut settings) = self.settings.write() {
            // Map LootFilters to LootSettings
            settings.loot.visible_bags.insert(0, filters.show_brown);
            settings.loot.visible_bags.insert(1, filters.show_pink);
            settings.loot.visible_bags.insert(2, filters.show_purple);
            settings.loot.visible_bags.insert(3, filters.show_teal);
            settings.loot.visible_bags.insert(4, filters.show_blue);
            settings.loot.visible_bags.insert(5, filters.show_gold);
            settings.loot.visible_bags.insert(6, filters.show_orange);
            settings.loot.visible_bags.insert(7, filters.show_white);
            settings.loot.visible_bags.insert(8, filters.show_red);
            settings.loot.visible_bags.insert(9, filters.show_egg);
            settings.save();
        }
    }

    /// Save chat filter settings and persist to disk.
    pub fn save_chat_settings(&self, filters: &crate::panels::chat::ChatFilters) {
        if let Ok(mut settings) = self.settings.write() {
            settings.chat.show_public = filters.show_normal;
            settings.chat.show_whisper = filters.show_whisper;
            settings.chat.show_guild = filters.show_guild;
            settings.chat.show_party = filters.show_party;
            settings.save();
        }
    }

    /// Persist the Trophy Hall data source so it survives restarts.
    fn persist_trophy_hall_source(&self) {
        if let Ok(mut settings) = self.settings.write() {
            settings.trophy_hall.data_source =
                Some(self.trophy_hall_panel.data_source().as_key().to_string());
            settings.save();
        }
    }

    /// Persist the Trophy Hall view toggles so they survive restarts.
    fn persist_trophy_hall_view(&self) {
        if let Ok(mut settings) = self.settings.write() {
            settings.trophy_hall.show_no_collection_dungeons =
                self.trophy_hall_panel.show_no_collection_dungeons();
            settings.trophy_hall.compact_view = self.trophy_hall_panel.compact_view();
            settings.save();
        }
    }

    /// Start packet capture with auto-detection of active interface.
    fn start_capture(&mut self) {
        match self.capture.start() {
            Ok(()) => {
                // Hand the fresh capture source to the worker, which resets the
                // session/reassembler and begins draining it on its own thread.
                if let Some(rx) = self.capture.take_receiver() {
                    self.worker.send_control(ControlMsg::StartCapture(rx));
                }
                self.live_feed_panel.clear_realm_state(); // Clear realm status bar
            }
            Err(e) => {
                tracing::error!("{}", e);
            }
        }
    }

    /// Gracefully shut down and relaunch, or surface a failure and quit.
    /// Every persistence step (worker shutdown, settings, registry) must succeed
    /// before swapping the exe or spawning a successor, so no account state is lost.
    fn relaunch(&mut self, reason: RelaunchReason) {
        use std::time::Duration;

        let report = self.worker.shutdown(Duration::from_secs(5));

        let settings_saved = match self.settings.read() {
            Ok(settings) => settings.save_result().map_err(|e| e.to_string()),
            Err(_) => Err("settings lock poisoned".to_string()),
        };

        let registry_saved = {
            let store = AccountRegistryStore::new(self.account.storage_root().clone());
            let now = chrono::Utc::now();
            match &reason {
                RelaunchReason::AccountSwitch(target_key) => {
                    let _ = store.update_last_opened(self.account.account_key(), now);
                    store
                        .select(*target_key, now)
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                }
                RelaunchReason::AccountDiscovery => store
                    .commit_discovery()
                    .map(|_| ())
                    .map_err(|e| e.to_string()),
                // Deletion happens after preflight and resource cleanup.
                RelaunchReason::AccountDeletion => Ok(()),
                _ => store
                    .update_last_opened(self.account.account_key(), now)
                    .map(|_| ())
                    .map_err(|e| e.to_string()),
            }
        };

        if let Err(why) = relaunch_preflight(&report, settings_saved, registry_saved) {
            self.abort_relaunch(&why);
            return;
        }

        // Active-account deletion: close readers first, then delete.
        if reason == RelaunchReason::AccountDeletion {
            self.loot_reader.take();
            self.combat_reader.take();

            let store = AccountRegistryStore::new(self.account.storage_root().clone());
            if let Err(e) = store.delete_account(
                self.account.account_key(),
                self.account.credentials().as_ref(),
            ) {
                self.abort_relaunch(&format!("account deletion failed: {e}"));
                return;
            }
        }

        // Swap the new exe into place; roll back if the spawn below fails.
        let staged = match reason {
            RelaunchReason::Updated => match self.self_updater.stage_swap() {
                Ok(staged) => Some(staged),
                Err(e) => {
                    self.abort_relaunch(&format!("staging update failed: {e}"));
                    return;
                }
            },
            _ => None,
        };

        let exe = match &staged {
            Some(staged) => staged.exe_path().to_path_buf(),
            None => match std::env::current_exe() {
                Ok(path) => path,
                Err(e) => {
                    self.abort_relaunch(&format!("cannot resolve current exe: {e}"));
                    return;
                }
            },
        };

        if let Err(e) = spawn_replacement(&exe, std::process::id()) {
            if let Some(staged) = &staged {
                rollback_executable_swap(staged);
            }
            self.abort_relaunch(&format!("relaunch spawn failed: {e}"));
            return;
        }

        std::process::exit(0);
    }

    /// Surface a fatal relaunch failure and quit; the worker is already stopped.
    fn abort_relaunch(&self, reason: &str) {
        tracing::error!("[RELAUNCH] aborted: {reason}");
        #[cfg(windows)]
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
            let title: Vec<u16> = "RealmHound\0".encode_utf16().collect();
            let msg: Vec<u16> = format!("Restart failed and RealmHound must close:\n\n{reason}\0")
                .encode_utf16()
                .collect();
            unsafe {
                MessageBoxW(0, msg.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR);
            }
        }
        std::process::exit(1);
    }

    /// Drain pending worker [`UiUpdate`]s and apply them to the UI panels.
    ///
    /// Returns `true` if the per-frame cap was hit (more may remain), so the
    /// caller can request another repaint to keep draining.
    fn drain_ui_updates(&mut self, _ctx: &egui::Context) -> bool {
        #[cfg(feature = "latency-diagnostics")]
        let frame_started = Instant::now();
        #[cfg(feature = "latency-diagnostics")]
        let frame_gap = frame_started.duration_since(self.latency_last_frame);
        #[cfg(feature = "latency-diagnostics")]
        {
            self.latency_last_frame = frame_started;
        }
        #[cfg(feature = "latency-diagnostics")]
        let mut oldest_wait = Duration::ZERO;

        // Take a bounded number per frame to avoid stalling the UI if a huge
        // backlog arrives; the rest are applied next frame.
        let mut applied = 0usize;
        while applied < 4096 {
            match self.worker.ui_rx.try_recv() {
                Some(update) => {
                    #[cfg(feature = "latency-diagnostics")]
                    if let Some(enqueued_at) = update.enqueued_at {
                        oldest_wait = oldest_wait.max(frame_started.duration_since(enqueued_at));
                    }
                    self.apply_update(update);
                    applied += 1;
                }
                None => break,
            }
        }
        #[cfg(feature = "latency-diagnostics")]
        let remaining = self.worker.ui_rx.len();
        #[cfg(feature = "latency-diagnostics")]
        if oldest_wait >= Duration::from_millis(500)
            && self
                .latency_last_ui_warn
                .is_none_or(|last| frame_started.duration_since(last) >= Duration::from_secs(2))
        {
            self.latency_last_ui_warn = Some(frame_started);
            let (focused, minimized) = _ctx.input(|input| {
                (
                    input.viewport().focused.unwrap_or(false),
                    input.viewport().minimized,
                )
            });
            let minimized = minimized
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            tracing::warn!(
                "[LATENCY][UI] oldest_queued_ms={} queue_remaining={} frame_gap_ms={} \
                 active_tab={} viewport_focused={} viewport_minimized={}",
                oldest_wait.as_millis(),
                remaining,
                frame_gap.as_millis(),
                self.active_tab.name(),
                focused,
                minimized
            );
        }
        #[cfg(feature = "latency-diagnostics")]
        if let Some(health) = self.latency_ui_health.record(
            frame_started,
            applied,
            applied == 4096,
            oldest_wait,
            remaining,
            frame_gap,
        ) {
            let (focused, minimized) = _ctx.input(|input| {
                (
                    input.viewport().focused.unwrap_or(false),
                    input.viewport().minimized,
                )
            });
            let minimized = minimized
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            tracing::info!(
                "[LATENCY][UI] health window_ms={} updates={} capped_drain_frames={} \
                 peak_oldest_queued_ms={} peak_queue_remaining={} peak_frame_gap_ms={} \
                 active_tab_at_emit={} viewport_focused={} viewport_minimized={}",
                health.window.as_millis(),
                health.updates,
                health.capped_drain_frames,
                health.peak_oldest_wait.as_millis(),
                health.peak_remaining,
                health.peak_frame_gap.as_millis(),
                self.active_tab.name(),
                focused,
                minimized
            );
        }
        applied == 4096
    }

    /// Apply a single processor [`UiUpdate`] to the UI panels.
    fn apply_update(&mut self, update: UiUpdate) {
        match update.payload {
            UiPayload::Broadcast(event) => {
                // Re-broadcast to the session-independent transient panels. These
                // panels ignore the session arg (the worker owns the real one);
                // chat/characters are resolved by the worker as their own updates.
                let mut actions = Vec::new();
                {
                    let session = &self.event_session;
                    actions.extend(self.quest_panel.handle_event(&event, session));
                    actions.extend(self.party_panel.handle_event(&event, session));
                    actions.extend(self.live_feed_panel.handle_event(&event, session));
                    actions.extend(self.vault_panel.handle_event(&event, session));
                }
                self.process_actions(actions);
            }
            UiPayload::Chat(msg) => {
                if msg.chat_type == crate::panels::chat::ChatType::Whisper
                    && !msg.is_outgoing
                    && self.live_feed_panel.dm_display_enabled()
                {
                    if !msg.sender.is_empty() {
                        self.live_feed_panel
                            .push_dm(msg.sender.clone(), msg.text.clone());
                    }
                }
                self.chat_panel.add_message(msg);
            }
            UiPayload::BossCall { boss_id, text } => {
                let should_sound = self.live_feed_panel.push_boss_call(boss_id, text.clone());
                self.maybe_play_realm_event(boss_id, &text, should_sound);
            }
            UiPayload::KeyperEvent { towers } => {
                use realmhound_core::keyper::{KEYPER_BOSS_ID, KEYPER_TOWER_SPRITE_ID};
                let (boss_id, name) = if towers {
                    (KEYPER_TOWER_SPRITE_ID, "Keyper Towers")
                } else {
                    (KEYPER_BOSS_ID, "The Keyper")
                };
                // Route through the shared realm-event notification path so the
                // "Seasonal and special bosses" section (The Keyper / Keyper
                // Towers rows) drives the sound, exactly like any other boss call.
                let should_sound = self
                    .live_feed_panel
                    .push_boss_call(boss_id, name.to_string());
                self.maybe_play_realm_event(boss_id, name, should_sound);
            }
            UiPayload::RealmClosed => {
                use crate::panels::live_feed::WarningSeverity;
                self.live_feed_panel.force_realm_closed();
                self.live_feed_panel.push_warning(
                    "Realm closed!".to_string(),
                    WarningSeverity::Orange,
                    false,
                );
            }
            UiPayload::OryxLagWarning => {
                use crate::panels::live_feed::WarningSeverity;
                if self.live_feed_panel.is_in_dungeon() {
                    self.live_feed_panel.push_warning(
                        "Oryx lag incoming, careful!".to_string(),
                        WarningSeverity::Red,
                        true,
                    );
                }
            }
            UiPayload::SetServerName(name) => {
                self.live_feed_panel.set_server_name(name);
            }
            UiPayload::DungeonTimerFrozen {
                map_seed,
                elapsed_ms,
            } => {
                self.live_feed_panel
                    .apply_dungeon_freeze(map_seed, elapsed_ms);
            }
            UiPayload::PushLoot(drop) => {
                self.live_feed_panel.push_loot(&drop);
                // A new drop was recorded by the worker; refresh the history view.
                self.loot_panel.mark_history_dirty();
            }
            UiPayload::LootBatchAdded => {
                self.loot_panel.invalidate_autocomplete_cache();
                self.loot_panel.mark_history_dirty();
            }
            UiPayload::AddEncounter {
                object_id,
                object_type,
                name,
            } => {
                self.live_feed_panel
                    .add_encounter(object_id, object_type, name);
            }
            UiPayload::RemoveEncounter(object_id) => {
                self.live_feed_panel.remove_encounter(object_id);
            }
            UiPayload::PartyMemberLeft(name) => {
                if self.party_panel.remove_member_by_name(&name) {
                    tracing::debug!("[PARTY] Member left (via chat): {}", name);
                }
            }
            UiPayload::KeyPop {
                opener,
                key_id,
                dungeon_name,
                thankable,
            } => {
                self.live_feed_panel
                    .push_key_pop(key_id, dungeon_name, opener, thankable);
            }
            UiPayload::AreaUnlock {
                title,
                contributions,
                callout,
            } => {
                self.live_feed_panel
                    .push_area_unlock(title, contributions, callout);
            }
            UiPayload::PartyLocalLeaderStatus(is_leader) => {
                self.party_panel.set_local_leader(is_leader);
            }
            UiPayload::PartyLocalPlayerName(name) => {
                self.party_panel.set_local_player_name(name);
            }
            UiPayload::PartyCleared => {
                self.party_panel.clear();
            }
            UiPayload::UpdateVaultPetSlot {
                pet_instance_id,
                seasonal,
                slot,
                item_id,
                stack_count,
            } => {
                self.vault_panel.update_pet_inventory_slot(
                    pet_instance_id,
                    seasonal,
                    slot,
                    item_id,
                    stack_count,
                );
            }
            UiPayload::SetVaultActivePet {
                char_id,
                instance_id,
            } => {
                self.vault_panel.set_active_pet(char_id, instance_id);
            }
            UiPayload::AccessTokenCaptured(token) => {
                // Note: we deliberately do NOT mark the client as "seen" here.
                // A HELLO can come from a mule/alt client, whereas the safety
                // gate is per-account; only the verified main's live
                // character (stamped in refresh_snapshot) may unlock refresh.
                // Only a changed token counts as a fresh capture; a repeated HELLO
                // with the same (aging) token must not renew its lifetime.
                let token_changed = self.captured_access_token.as_deref() != Some(token.as_str());
                self.captured_access_token = Some(token.clone());
                if token_changed {
                    self.token_captured_at = Some(chrono::Utc::now());
                    self.characters_panel.reset_on_new_token();
                    self.vault_panel.reset_on_new_token();
                }

                // A freshly captured token's owning account is not yet verified,
                // so it is never persisted here. Persistence is deferred to a
                // fetch that confirms the token belongs to the selected account
                // (see check_api_result handling), and always goes through the
                // credential store -- never the legacy flat token file.

                // Fetch exalt stats once per app session
                if !self.exalts_fetched {
                    self.exalts_fetched = true;
                    let token_for_exalts = token.clone();
                    std::thread::spawn(move || {
                        tracing::debug!("[API] Fetching exalt stats...");
                        let client = realmhound_core::api::RotmgApiClient::new(token_for_exalts);
                        match client.get_power_up_stats() {
                            Ok(xml) => {
                                tracing::info!("[EXALTS] Loaded exalt stats ({} bytes)", xml.len())
                            }
                            Err(e) => tracing::warn!("[EXALTS] Failed to load: {}", e),
                        }
                    });
                }
            }
            UiPayload::Audio(cmd) => {
                // The worker routes audio directly to the audio thread, so this
                // arm is normally unreachable; forward as a safety net.
                self.audio_tx.send(cmd);
            }
        }
    }

    /// Refresh owned model snapshots from the worker's shared snapshot.
    ///
    /// Cheap fields are copied every frame; heavy fields (account/vault) are
    /// cloned only when their generation changed.
    fn refresh_snapshot(&mut self) {
        let snap = match self.worker.snapshot.lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        self.view_state = snap.view.clone();
        self.live_char_id = snap.live_char_id;
        // Remember the last-seen live character (persisted) so the
        // Current-Character widget can restore it, dimmed, after a restart.
        if let Some(id) = self.live_char_id {
            if self.last_live_char_id != Some(id) {
                self.last_live_char_id = Some(id);
                if let Ok(mut s) = self.settings.write() {
                    s.characters.last_live_char_id = Some(id);
                    s.save();
                }
                tracing::info!(char_id = id, "[WIDGET] Persisted last-seen live character");
            }
        }
        // Track the main game-client socket so we can map it to its owning OS
        // process (forge-fire relaunch safeguard). A new socket appears on a
        // cold launch AND on every area change, so a change alone proves
        // nothing -- the owning process's start time is what distinguishes a
        // post-reset relaunch from a session that merely spanned midnight.
        let main_conn = snap.main_connection;
        let conn_changed = self.main_connection != main_conn;
        self.main_connection = main_conn;

        let epoch = (snap.view.char_view_gen, snap.view.account_generation);
        if epoch != self.owned_account_epoch {
            self.account_data = snap.account_data.clone();
            self.owned_account_epoch = epoch;
            self.account_data_revision = self.account_data_revision.wrapping_add(1);
        }
        if snap.view.vault_view_gen != self.owned_vault_gen {
            self.live_vault_data = snap.live_vault_data.clone();
            self.owned_vault_gen = snap.view.vault_view_gen;
        }
        if snap.view.mission_view_gen != self.owned_mission_gen {
            self.mission_view = snap.mission_view.clone();
            self.owned_mission_gen = snap.view.mission_view_gen;
        }
        drop(snap);
        self.maybe_confirm_client_launch(conn_changed);
    }

    /// Attempt to confirm that the main game client was relaunched on the
    /// current UTC day and persist the confirmed launch timestamp.
    ///
    /// Two independent signals feed this, in priority order:
    ///
    /// 1. The tracked main connection -> its owning process's start time. This
    ///    is account-specific (it's *our* main's socket), so it unlocks
    ///    precisely, even if an older mule client is also running. Requires a
    ///    Hello to have registered the connection (connect or area change).
    /// 2. A scan of every RotMG client's game-server connection, taking the
    ///    earliest process start. This needs no Hello, so it works when
    ///    RealmHound is launched mid-game -- and being the *earliest* start it
    ///    can never unlock while any client predates the reset, so it never
    ///    claims the grant on a multiboxer's behalf.
    ///
    /// Throttled: runs when the connection changed, or at most every 30s while
    /// still unconfirmed for the day.
    fn maybe_confirm_client_launch(&mut self, conn_changed: bool) {
        if self.client_relaunched_today() {
            return;
        }
        let due = conn_changed
            || self
                .last_launch_check_at
                .map(|t| t.elapsed() >= std::time::Duration::from_secs(30))
                .unwrap_or(true);
        if !due {
            return;
        }
        self.last_launch_check_at = Some(std::time::Instant::now());

        // Prefer the account-specific connection signal; fall back to the
        // conservative earliest-client scan when we have no tracked connection
        // (e.g. RealmHound started mid-game, before any area change).
        let start = self
            .main_connection
            .and_then(|conn| crate::client_process::connection_process_start_unix(&conn))
            .filter(|&s| s > 0)
            .or_else(|| {
                crate::client_process::earliest_rotmg_client_start_unix().filter(|&s| s > 0)
            });
        let Some(start) = start else {
            return;
        };

        let started_today = chrono::DateTime::<chrono::Utc>::from_timestamp(start, 0)
            .map(|d| d.date_naive() == chrono::Utc::now().date_naive())
            .unwrap_or(false);
        if !started_today {
            return;
        }
        if start != self.client_launch_unix {
            // Runtime last-client-launch lives on the selected registry entry
            // (authoritative identity), not `settings.account`. Cache in
            // lock-step with the atomic write so the gate reads without disk I/O.
            let store = AccountRegistryStore::new(self.account.storage_root().clone());
            match store.update_last_client_launch(self.account.account_key(), start) {
                Ok(_) => self.client_launch_unix = start,
                Err(e) => tracing::warn!("[ACCOUNT] Failed to record client launch: {e}"),
            }
        }
    }

    /// Whether the main game client was launched (a fresh process, not a mere
    /// area change) on the current UTC day. The daily forge-fire grant only
    /// lands on such a relaunch, so account API refreshes are gated on this so
    /// RealmHound never claims the day's grant on the user's behalf. When false,
    /// refresh is withheld and framed to the user as an expired access token.
    fn client_relaunched_today(&self) -> bool {
        let ts = self.client_launch_unix;
        if ts <= 0 {
            return false;
        }
        chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0)
            .map(|launched| launched.date_naive() == chrono::Utc::now().date_naive())
            .unwrap_or(false)
    }

    /// Create dark theme visuals.
    fn dark_visuals() -> Visuals {
        let mut visuals = Visuals::dark();

        // Customize colors for RotMG theme
        visuals.override_text_color = Some(Color32::from_rgb(220, 220, 220));
        visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(30, 30, 35);
        visuals.widgets.inactive.bg_fill = Color32::from_rgb(45, 45, 50);
        visuals.widgets.hovered.bg_fill = Color32::from_rgb(60, 60, 70);
        visuals.widgets.active.bg_fill = Color32::from_rgb(80, 80, 100);

        // Window background
        visuals.panel_fill = Color32::from_rgb(25, 25, 30);
        visuals.window_fill = Color32::from_rgb(30, 30, 35);

        visuals
    }

    /// Render the About window (modal).
    fn render_about_window(&mut self, ctx: &egui::Context) {
        let shadcn = self.shadcn.clone();
        let mut open = self.modals.show_about;
        let mut open_changelog = false;
        let mut open_update_details = false;

        egui::Area::new(egui::Id::new("about_dialog_host"))
            .fixed_pos(egui::pos2(0.0, 0.0))
            .show(ctx, |ui| {
                shadcn.dialog(ui, "about_modal", &mut open, PKG_NAME, 360.0, 520.0, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(format!("Version {}", VERSION)).size(16.0));
                        ui.add_space(6.0);
                        ui.label(PKG_DESCRIPTION);
                        ui.add_space(10.0);
                        shadcn.separator(ui);
                        ui.add_space(6.0);
                        ui.label(format!("Authors: {}", PKG_AUTHORS));
                        ui.add_space(6.0);
                        if shadcn.btn(ui, "📋 Changelog").clicked() {
                            open_changelog = true;
                        }
                        ui.collapsing("Licenses", |ui| {
                            egui::ScrollArea::vertical()
                                .max_height(180.0)
                                .show(ui, |ui| {
                                    ui.label(PROJECT_LICENSE);
                                    ui.add_space(12.0);
                                    ui.label(THIRD_PARTY_LICENSES);
                                });
                        });
                        ui.add_space(10.0);
                        shadcn.separator(ui);
                        ui.add_space(10.0);

                        // Discord join card, styled after Discord's own embedded
                        // server-invite card (server icon, title, subtitle, Join button).
                        let icon_px = 64.0;
                        let (icon_rect, _) = ui.allocate_exact_size(
                            egui::vec2(icon_px, icon_px),
                            egui::Sense::hover(),
                        );
                        self.sprite_renderer.draw_embedded_icon(
                            ui,
                            EmbeddedIcon::Discord,
                            icon_rect,
                        );

                        ui.add_space(6.0);
                        ui.label(RichText::new("Join our Discord!").size(18.0).strong());

                        ui.add_space(4.0);
                        // Subtitle line: Discord logo to the LEFT of "RealmHound".
                        // Allocated as one fixed-size block so vertical_centered centers it.
                        {
                            let sub_color = Color32::from_rgb(185, 190, 200);
                            let logo_h = 15.0;
                            let logo_w = logo_h * 528.0 / 400.0;
                            let gap = 6.0;
                            let galley = ui.painter().layout_no_wrap(
                                "RealmHound".to_owned(),
                                egui::FontId::proportional(14.0),
                                sub_color,
                            );
                            let total_w = logo_w + gap + galley.size().x;
                            let total_h = logo_h.max(galley.size().y);
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(total_w, total_h),
                                egui::Sense::hover(),
                            );
                            let logo_rect = egui::Rect::from_min_size(
                                egui::pos2(rect.left(), rect.center().y - logo_h / 2.0),
                                egui::vec2(logo_w, logo_h),
                            );
                            self.sprite_renderer.draw_embedded_icon(
                                ui,
                                EmbeddedIcon::DiscordLogo,
                                logo_rect,
                            );
                            let text_pos = egui::pos2(
                                rect.left() + logo_w + gap,
                                rect.center().y - galley.size().y / 2.0,
                            );
                            ui.painter().galley(text_pos, galley, sub_color);
                        }

                        ui.add_space(10.0);
                        let join = ui
                            .scope(|ui| {
                                ui.spacing_mut().button_padding = egui::vec2(28.0, 6.0);
                                ui.add(
                                    egui::Button::new(
                                        RichText::new("Join").color(Color32::WHITE).strong(),
                                    )
                                    .fill(Color32::from_rgb(88, 101, 242))
                                    .corner_radius(6.0),
                                )
                            })
                            .inner;
                        if join.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if join.clicked() {
                            ui.ctx()
                                .open_url(egui::OpenUrl::new_tab("https://discord.gg/zXAbEATmy"));
                        }

                        ui.add_space(10.0);
                        shadcn.separator(ui);
                        ui.add_space(6.0);

                        use realmhound_core::update::UpdateState;
                        match self.update_checker.state() {
                            UpdateState::Unknown => {
                                if shadcn.btn(ui, "Check for Updates").clicked() {
                                    self.update_checker.force_check();
                                }
                            }
                            UpdateState::Checking => {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label("Checking for updates...");
                                });
                            }
                            UpdateState::UpToDate => {
                                ui.label(
                                    RichText::new("You're up to date!")
                                        .color(Color32::from_rgb(100, 200, 100)),
                                );
                                ui.add_space(4.0);
                                if shadcn.btn(ui, "Check again").clicked() {
                                    self.update_checker.force_check();
                                }
                            }
                            UpdateState::UpdateAvailable {
                                manifest,
                                new_releases,
                            } => {
                                ui.label(
                                    RichText::new(format!(
                                        "Update available: v{}",
                                        manifest.latest_version
                                    ))
                                    .color(Color32::from_rgb(255, 200, 100)),
                                );
                                ui.add_space(4.0);

                                // Show brief changelog summary
                                if !new_releases.is_empty() {
                                    if let Some(latest) = new_releases.first() {
                                        for change in latest.changes.iter().take(3) {
                                            ui.label(format!("  • {}", change));
                                        }
                                        if latest.changes.len() > 3 || new_releases.len() > 1 {
                                            if ui.small_button("See all changes...").clicked() {
                                                open_update_details = true;
                                            }
                                        }
                                    }
                                    ui.add_space(4.0);
                                }

                                use realmhound_core::update::SelfUpdateState;
                                match self.self_updater.state() {
                                    SelfUpdateState::Idle => {
                                        if shadcn
                                            .btn(
                                                ui,
                                                format!("⬇ Update to v{}", manifest.latest_version),
                                            )
                                            .clicked()
                                        {
                                            self.self_updater.start_download(
                                                manifest.download_url.clone(),
                                                manifest.sha256.clone(),
                                            );
                                        }
                                    }
                                    SelfUpdateState::Downloading(progress) => {
                                        ui.horizontal(|ui| {
                                            ui.spinner();
                                            if let Some(total) = progress.total {
                                                let pct = (progress.downloaded as f32
                                                    / total as f32
                                                    * 100.0)
                                                    as u32;
                                                ui.label(format!("Downloading... {}%", pct));
                                            } else {
                                                ui.label("Downloading...");
                                            }
                                        });
                                    }
                                    SelfUpdateState::ReadyToApply => {
                                        if shadcn.btn(ui, "✓ Restart to apply update").clicked() {
                                            self.pending_relaunch = Some(RelaunchReason::Updated);
                                        }
                                    }
                                    SelfUpdateState::Failed(msg) => {
                                        ui.label(
                                            RichText::new(format!("Update failed: {}", msg))
                                                .color(Color32::from_rgb(200, 100, 100))
                                                .small(),
                                        );
                                        if shadcn.btn(ui, "Retry").clicked() {
                                            self.self_updater.reset();
                                        }
                                    }
                                }
                            }
                            UpdateState::Error(err) => {
                                ui.label(
                                    RichText::new(format!("Update check failed: {}", err.message))
                                        .color(Color32::from_rgb(200, 150, 100))
                                        .small(),
                                );
                                ui.add_space(4.0);
                                if shadcn.btn(ui, "Retry").clicked() {
                                    self.update_checker.force_check();
                                }
                            }
                        }
                    });
                });
            });

        self.modals.show_about = open;
        if open_changelog {
            self.modals.show_about = false;
            self.modals.show_changelog = true;
        }
        if open_update_details {
            self.modals.show_about = false;
            self.update_panel.open_details();
        }
    }

    /// Render the changelog window showing all known releases.
    fn render_changelog_window(&mut self, ctx: &egui::Context) {
        let shadcn = self.shadcn.clone();
        let mut open = self.modals.show_changelog;

        egui::Area::new(egui::Id::new("changelog_dialog_host"))
            .fixed_pos(egui::pos2(0.0, 0.0))
            .show(ctx, |ui| {
                shadcn.dialog(
                    ui,
                    "changelog_modal",
                    &mut open,
                    "Changelog",
                    450.0,
                    500.0,
                    |ui| {
                        let releases = self.update_checker.all_releases();
                        if releases.is_empty() {
                            ui.label("No changelog data available yet.");
                            ui.add_space(4.0);
                            if shadcn.btn(ui, "Check for updates").clicked() {
                                self.update_checker.force_check();
                            }
                        } else {
                            egui::ScrollArea::vertical()
                                .max_height(420.0)
                                .show(ui, |ui| {
                                    for release in releases {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                RichText::new(format!("v{}", release.version))
                                                    .strong()
                                                    .color(Color32::from_rgb(150, 180, 255)),
                                            );
                                            ui.label(
                                                RichText::new(format!("({})", release.date))
                                                    .weak()
                                                    .small(),
                                            );
                                        });
                                        for change in &release.changes {
                                            ui.label(format!("  • {}", change));
                                        }
                                        ui.add_space(8.0);
                                    }
                                });
                        }
                    },
                );
            });

        self.modals.show_changelog = open;
    }

    /// Render the Settings modal as a fixed-size shadcn dialog.
    ///
    /// The dialog has an explicit width/height so it no longer resizes when the
    /// user switches tabs; a vertical scroll area absorbs taller tabs.
    fn render_settings_modal(&mut self, ctx: &egui::Context) {
        // Clone the theme helper so the content closures can borrow `&mut self`
        // (for the per-tab render methods) and `&shadcn` at once. Cloning copies
        // the resolved palette without re-running `Theme::new`.
        let shadcn = self.shadcn.clone();
        let mut open = self.modals.show_settings;

        egui::Area::new(egui::Id::new("settings_dialog_host"))
            .fixed_pos(egui::pos2(0.0, 0.0))
            .show(ctx, |ui| {
                shadcn.dialog_sticky(
                    ui,
                    "settings_modal",
                    &mut open,
                    "Settings",
                    1040.0,
                    580.0,
                    |ui| {
                        let mut active = self.modals.settings_category.to_key().to_string();
                        let tab_items = [
                            ("live_feed", "📡 Live Feed"),
                            ("widget_bar", "📊 Widget Bar"),
                            ("taskbar", "🧭 Taskbar"),
                            ("loot", "🎒 Loot History"),
                            ("combat_history", "⚔ Combat History"),
                            ("trophy_hall", "🏆 Trophy Hall"),
                            ("treasury", "💰 Treasury"),
                            ("party", "🎉 Party"),
                            ("appearance", "🎨 Appearance"),
                            ("sound", "🔊 Sound"),
                            ("account", "👤 Account"),
                        ];
                        ui.horizontal_top(|ui| {
                            // Left: vertical tab sidebar (Discord-style).
                            let sidebar_width = 172.0;
                            ui.allocate_ui_with_layout(
                                egui::vec2(sidebar_width, ui.available_height()),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    ui.set_width(sidebar_width);
                                    for (key, label) in tab_items {
                                        let selected = active == key;
                                        if shadcn.nav_item(ui, label, selected).clicked() {
                                            active = key.to_string();
                                        }
                                        ui.add_space(2.0);

                                        // Nested Sound sub-tabs (Discord-style)
                                        // shown while the Sound tab is active.
                                        if key == "sound" && active == "sound" {
                                            for (sub, sub_label) in [
                                                (SoundSubTab::General, "General"),
                                                (SoundSubTab::LootBags, "Loot bags"),
                                                (SoundSubTab::RealmBosses, "Realm Bosses"),
                                                (SoundSubTab::Enchantments, "Enchantments"),
                                            ] {
                                                let sub_selected = self.sound_sub_tab == sub;
                                                if shadcn
                                                    .nav_subitem(ui, sub_label, sub_selected)
                                                    .clicked()
                                                {
                                                    self.sound_sub_tab = sub;
                                                }
                                                ui.add_space(2.0);
                                            }
                                        }
                                    }
                                },
                            );

                            // Reflect the click immediately so the content matches
                            // the selected tab on the same frame.
                            self.modals.settings_category = SettingsCategory::from_key(&active);

                            ui.add_space(8.0);
                            ui.separator();
                            ui.add_space(8.0);

                            // Right: scrollable content for the active tab.
                            ui.allocate_ui_with_layout(
                                egui::vec2(ui.available_width(), ui.available_height()),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    egui::ScrollArea::vertical()
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            // Re-add a right gutter (the dialog
                                            // frame's right padding was zeroed so
                                            // the scroll bar hugs the window
                                            // border) so cards keep breathing room
                                            // from the bar.
                                            egui::Frame::NONE
                                                .inner_margin(egui::Margin {
                                                    left: 0,
                                                    right: 16,
                                                    top: 0,
                                                    bottom: 0,
                                                })
                                                .show(ui, |ui| {
                                                    match self.modals.settings_category {
                                                        SettingsCategory::Loot => self
                                                            .render_loot_tracking_settings(
                                                                ui, &shadcn,
                                                            ),
                                                        SettingsCategory::CombatHistory => self
                                                            .render_combat_history_settings(
                                                                ui, &shadcn,
                                                            ),
                                                        SettingsCategory::Sound => {
                                                            self.render_sound_settings(ui, &shadcn)
                                                        }
                                                        SettingsCategory::LiveFeed => self
                                                            .render_live_feed_settings(ui, &shadcn),
                                                        SettingsCategory::Party => {
                                                            self.render_party_settings(ui, &shadcn)
                                                        }
                                                        SettingsCategory::TrophyHall => self
                                                            .render_trophy_hall_settings(
                                                                ui, &shadcn,
                                                            ),
                                                        SettingsCategory::Account => self
                                                            .render_account_settings(ui, &shadcn),
                                                        SettingsCategory::Treasury => self
                                                            .render_treasury_settings(ui, &shadcn),
                                                        SettingsCategory::Appearance => self
                                                            .render_appearance_settings(
                                                                ui, &shadcn,
                                                            ),
                                                        SettingsCategory::WidgetBar => self
                                                            .render_widget_bar_settings(
                                                                ui, &shadcn,
                                                            ),
                                                        SettingsCategory::Taskbar => self
                                                            .render_taskbar_settings(ui, &shadcn),
                                                    }
                                                });
                                        });
                                },
                            );
                        });
                        self.modals.settings_category = SettingsCategory::from_key(&active);

                        // Up/Down arrow keys cycle between tabs, unless a text
                        // field has focus (so typing arrows in inputs still works).
                        let text_focused = ui.ctx().memory(|m| m.focused()).is_some();
                        if !text_focused {
                            let dir = ui.ctx().input(|i| {
                                if i.key_pressed(egui::Key::ArrowDown) {
                                    1i32
                                } else if i.key_pressed(egui::Key::ArrowUp) {
                                    -1
                                } else {
                                    0
                                }
                            });
                            if dir != 0 {
                                if let Some(cur) = tab_items.iter().position(|(k, _)| *k == active)
                                {
                                    let n = tab_items.len() as i32;
                                    let next = ((cur as i32 + dir) % n + n) % n;
                                    self.modals.settings_category =
                                        SettingsCategory::from_key(tab_items[next as usize].0);
                                }
                            }
                        }
                    },
                );
            });

        self.modals.show_settings = open;
        if !self.modals.show_settings {
            self.modals.show_combat_clear_confirm = false;
            self.cached_registry_entries.clear();
            self.registry_cache_loaded = false;
            self.switch_confirm_target = None;
            self.switch_error = None;
            self.discovery_confirm_active = false;
            self.discovery_error = None;
            self.delete_confirm_target = None;
            self.delete_error = None;
        } else if self.modals.show_combat_clear_confirm {
            // Rendered here (after the settings dialog area closes) so it layers
            // on top of the dialog. The dialog content sits on `Order::Tooltip`;
            // matching that order and being created afterwards keeps this on top
            // instead of hidden behind the opaque settings panel.
            self.render_combat_clear_confirm(ctx, &shadcn);
        }

        // Rebuild the live theme as soon as the selection changes so the preview
        // updates instantly (e.g. while dragging the custom color picker).
        if self.theme_selection != self.shadcn.selection() {
            let compact = self.shadcn.is_compact();
            self.shadcn = crate::shadcn_ui::Shadcn::from_selection(self.theme_selection);
            self.shadcn.set_compact(compact);
            Self::apply_theme(ctx, &self.shadcn);
        }

        // Persist only once the user settles (pointer released), so scrubbing the
        // color picker doesn't write settings.json on every frame.
        if self.theme_selection != self.persisted_theme_selection && !ctx.is_using_pointer() {
            if let Ok(mut s) = self.settings.write() {
                s.appearance.preset = self.theme_selection.key().to_string();
                s.appearance.custom_primary = crate::shadcn_ui::color_to_hex(self.custom_primary);
                s.appearance.mode = self.custom_mode.key().to_string();
                s.save();
            }
            self.persisted_theme_selection = self.theme_selection;
        }
    }

    /// Build egui visuals matching the active shadcn palette and mode, so panels
    /// and chrome that aren't shadcn widgets still follow the theme.
    fn theme_visuals(shadcn: &crate::shadcn_ui::Shadcn) -> Visuals {
        let p = shadcn.colors();
        let mut visuals = if shadcn.mode().is_dark() {
            Visuals::dark()
        } else {
            Visuals::light()
        };
        visuals.override_text_color = Some(p.foreground);
        visuals.panel_fill = p.background;
        visuals.window_fill = p.card;
        visuals.extreme_bg_color = p.input;
        visuals.widgets.noninteractive.bg_fill = p.card;
        visuals.widgets.noninteractive.fg_stroke.color = p.foreground;
        visuals.widgets.inactive.bg_fill = p.secondary;
        visuals.widgets.inactive.fg_stroke.color = p.foreground;
        // Hover/active use neutral surfaces (not raw accent/primary) so the
        // forced foreground text stays readable in light and dark alike.
        visuals.widgets.hovered.bg_fill = p.muted;
        visuals.widgets.active.bg_fill = p.border;
        visuals.window_stroke.color = p.border;
        visuals.hyperlink_color = p.primary;
        // Selected widgets (tab buttons via `interact_selectable`, text
        // selection, etc.) use the theme accent instead of egui's default cyan.
        visuals.selection.bg_fill = p.accent;
        visuals.selection.stroke = egui::Stroke::new(1.0_f32, p.accent_foreground);
        visuals
    }

    /// Pin egui to Dark; without this, light-mode-OS users get a white UI after
    /// restart since egui's per-theme styles aren't persisted, only the preference.
    fn apply_theme(ctx: &egui::Context, shadcn: &crate::shadcn_ui::Shadcn) {
        ctx.set_theme(egui::ThemePreference::Dark);
        ctx.set_visuals_of(egui::Theme::Dark, Self::theme_visuals(shadcn));
    }

    /// Render the Appearance settings panel (theme preset or custom primary).
    /// Push the current tab-header display settings into the sprite renderer so
    /// tab and sub-tab rendering reflects the latest appearance settings.
    fn sync_tab_display(&mut self) {
        if let Ok(s) = self.settings.read() {
            self.sprite_renderer
                .set_tab_display(s.appearance.tab_labels, s.appearance.tab_tooltips);
        }
    }

    fn render_appearance_settings(&mut self, ui: &mut egui::Ui, shadcn: &crate::shadcn_ui::Shadcn) {
        use crate::shadcn_ui::{ThemePreset, ThemeSelection};

        ui.add_space(10.0);
        ui.heading("Appearance");
        ui.add_space(5.0);
        ui.label(RichText::new("Pick a theme for the application.").weak());
        ui.add_space(15.0);

        shadcn.card(ui, "appearance_theme", "Theme", |ui| {
            // Light-mode themes and Custom are disabled until the UI fully
            // supports light backgrounds. Use DARK_ONLY subset for now.
            let theme_opts: Vec<(&str, &str)> = ThemePreset::DARK_ONLY
                .iter()
                .map(|p| (p.key(), p.label()))
                .collect();
            // TODO: re-enable when light mode is supported:
            // let mut theme_opts: Vec<(&str, &str)> =
            //     ThemePreset::ALL.iter().map(|p| (p.key(), p.label())).collect();
            // theme_opts.push(("custom", "Custom"));
            let mut theme_sel: Option<String> = Some(self.theme_selection.key().to_string());
            if shadcn
                .select(
                    ui,
                    "appearance_theme_select",
                    &mut theme_sel,
                    200.0,
                    &theme_opts,
                )
                .changed()
            {
                if let Some(key) = theme_sel.as_deref() {
                    self.theme_selection = if key == "custom" {
                        ThemeSelection::Custom {
                            primary: self.custom_primary,
                            mode: self.custom_mode,
                        }
                    } else {
                        ThemePreset::from_key(key)
                            .map(ThemeSelection::Preset)
                            .unwrap_or(self.theme_selection)
                    };
                }
            }

            // Custom theme controls - disabled until light mode is supported.
            // if matches!(self.theme_selection, ThemeSelection::Custom { .. }) {
            //     ui.add_space(14.0);
            //
            //     let mut dark = self.custom_mode.is_dark();
            //     if shadcn.switch(ui, &mut dark, "Dark mode").changed() {
            //         self.custom_mode = if dark {
            //             crate::shadcn_ui::Mode::Dark
            //         } else {
            //             crate::shadcn_ui::Mode::Light
            //         };
            //     }
            //
            //     ui.add_space(12.0);
            //     ui.label(RichText::new("Primary color").strong());
            //     ui.label(
            //         RichText::new("Drives toggles, focus rings and the active tab; the rest is derived.")
            //             .small()
            //             .weak(),
            //     );
            //     ui.add_space(4.0);
            //     shadcn.field_row(ui, |ui| {
            //         shadcn.color_picker(ui, "appearance_custom_primary", &mut self.custom_primary);
            //         ui.label(RichText::new(crate::shadcn_ui::color_to_hex(self.custom_primary)).monospace());
            //     });
            //
            //     // Keep the live selection in sync with the custom controls.
            //     self.theme_selection = ThemeSelection::Custom {
            //         primary: self.custom_primary,
            //         mode: self.custom_mode,
            //     };
            // }
        });

        ui.add_space(12.0);

        shadcn.card(ui, "appearance_preview", "Preview", |ui| {
            let mut on = true;
            shadcn.switch(ui, &mut on, "Toggle (on)");
            ui.add_space(6.0);
            let mut off = false;
            shadcn.switch(ui, &mut off, "Toggle (off)");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                shadcn.btn(ui, "Panel Button");
                let mut tgl_on = true;
                shadcn.tgl(ui, &mut tgl_on, "Filter");
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                shadcn.button(ui, "Dialog Button");
                shadcn.button_destructive(ui, "Destructive", true);
            });
        });

        ui.add_space(12.0);

        shadcn.card(ui, "appearance_density", "Density", |ui| {
            let mut compact = self.shadcn.is_compact();
            if shadcn.switch(ui, &mut compact, "Compact UI").changed() {
                self.shadcn.set_compact(compact);
                if let Ok(mut s) = self.settings.write() {
                    s.appearance.compact_ui = compact;
                    s.save();
                }
            }
            ui.label(
                RichText::new("Use smaller buttons and toggles in panel toolbars.")
                    .small()
                    .weak(),
            );
        });

        ui.add_space(12.0);

        shadcn.card(ui, "appearance_tabs", "Tab Headers", |ui| {
            use realmhound_core::settings::TabLabelMode;

            let current = self
                .settings
                .read()
                .map(|s| s.appearance.tab_labels)
                .unwrap_or_default();
            let mut sel: Option<String> = Some(
                match current {
                    TabLabelMode::IconsAndText => "both",
                    TabLabelMode::IconsOnly => "icons",
                    TabLabelMode::TextOnly => "text",
                }
                .to_string(),
            );
            let opts: [(&str, &str); 3] = [
                ("both", "Both icons and text"),
                ("icons", "Only icons"),
                ("text", "Only text"),
            ];
            if shadcn
                .select(ui, "appearance_tabs_select", &mut sel, 200.0, &opts)
                .changed()
            {
                if let Some(mode) = sel.as_deref().map(|k| match k {
                    "icons" => TabLabelMode::IconsOnly,
                    "text" => TabLabelMode::TextOnly,
                    _ => TabLabelMode::IconsAndText,
                }) {
                    if let Ok(mut s) = self.settings.write() {
                        s.appearance.tab_labels = mode;
                        s.save();
                    }
                    self.sync_tab_display();
                }
            }
            ui.label(
                RichText::new(
                    "How main tabs and sub-tabs (Vault, Characters) show their icon and name.",
                )
                .small()
                .weak(),
            );
        });

        ui.add_space(12.0);

        shadcn.card(ui, "appearance_tab_tooltips", "Tab Tooltips", |ui| {
            let mut on = self.settings.read().map(|s| s.appearance.tab_tooltips).unwrap_or(true);
            if shadcn.switch(ui, &mut on, "Show tab tooltips").changed() {
                if let Ok(mut s) = self.settings.write() {
                    s.appearance.tab_tooltips = on;
                    s.save();
                }
                self.sync_tab_display();
            }
            ui.label(
                RichText::new("Show hover tooltips on tab and sub-tab headers. In \"Only icons\" mode these reveal the tab name.")
                    .small()
                    .weak(),
            );
        });

        ui.add_space(12.0);

        shadcn.card(ui, "appearance_confirm_close", "Confirm on Exit", |ui| {
            let mut on = self
                .settings
                .read()
                .map(|s| s.appearance.confirm_on_close)
                .unwrap_or(true);
            if shadcn.switch(ui, &mut on, "Ask before closing").changed() {
                if let Ok(mut s) = self.settings.write() {
                    s.appearance.confirm_on_close = on;
                    s.save();
                }
            }
            ui.label(
                RichText::new("Show a confirmation dialog when closing the RealmHound window.")
                    .small()
                    .weak(),
            );
        });

        ui.add_space(12.0);

        #[cfg(debug_assertions)]
        shadcn.card(ui, "appearance_resolution", "Resolution", |ui| {
            ui.label(
                RichText::new("Resize the app window to a preset resolution.")
                    .small()
                    .weak(),
            );
            ui.add_space(6.0);
            shadcn.field_row(ui, |ui| {
                ui.label("Window size:");
                let resolutions: [(&str, f32, f32); 3] = [
                    ("1366x768", 1366.0, 768.0),
                    ("1920x1080", 1920.0, 1080.0),
                    ("2560x1440", 2560.0, 1440.0),
                ];
                let current = self
                    .settings
                    .read()
                    .ok()
                    .map(|s| (s.window.width, s.window.height));
                let mut sel: Option<String> = current.and_then(|(w, h)| {
                    resolutions
                        .iter()
                        .find(|(_, rw, rh)| (*rw - w).abs() < 1.0 && (*rh - h).abs() < 1.0)
                        .map(|(key, _, _)| key.to_string())
                });
                let options: Vec<(&str, &str)> =
                    resolutions.iter().map(|(key, _, _)| (*key, *key)).collect();
                // Opens upward (issue: this is the last card in the Appearance
                // tab, so opening downward leaves too little room at smaller
                // window heights and the popup fails to open at all).
                if shadcn
                    .select_up(
                        ui,
                        "appearance_resolution_select",
                        &mut sel,
                        140.0,
                        &options,
                    )
                    .changed()
                {
                    if let Some(key) = sel.as_deref() {
                        if let Some((_, w, h)) = resolutions.iter().find(|(k, _, _)| *k == key) {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::InnerSize(
                                egui::vec2(*w, *h),
                            ));
                            if let Ok(mut s) = self.settings.write() {
                                s.window.width = *w;
                                s.window.height = *h;
                                s.window.maximized = false;
                                s.save();
                            }
                        }
                    }
                }
            });
        });
    }

    /// Render the Combat History settings panel (which boss groups are recorded).
    fn render_combat_history_settings(
        &mut self,
        ui: &mut egui::Ui,
        shadcn: &crate::shadcn_ui::Shadcn,
    ) {
        use realmhound_core::assets::BossGroup;

        ui.add_space(10.0);
        ui.heading("Combat History");
        ui.add_space(5.0);
        ui.label(
            RichText::new(
                "Choose which boss groups are recorded to the Combat History database. \
             Changes apply to fights recorded from now on.",
            )
            .weak(),
        );
        ui.add_space(15.0);

        let mut settings_changed = false;
        let mut current = {
            let s = self.settings.read().unwrap();
            s.combat_history.clone()
        };

        shadcn.card(ui, "combat_track_groups", "Track boss groups", |ui| {
            ui.label(
                RichText::new("Dungeon bosses are grouped by grave difficulty.")
                    .small()
                    .weak(),
            );
            ui.add_space(8.0);
            ui.spacing_mut().item_spacing.y = 6.0;
            for group in BossGroup::ALL {
                let toggle = current.track_mut(group);
                settings_changed |= shadcn.switch(ui, toggle, group.label()).changed();
            }
        });

        ui.add_space(12.0);

        shadcn.card(ui, "combat_track_solo", "Solo fights", |ui| {
            settings_changed |= shadcn
                .switch(ui, &mut current.track_solo, "Track solo fights")
                .changed();
            ui.label(
                RichText::new("Record fights where you are the only participant.")
                    .small()
                    .weak(),
            );
        });

        ui.add_space(12.0);

        shadcn.card(ui, "combat_clear_history", "Clear History", |ui| {
            if shadcn
                .button_destructive(ui, "Delete logs of all boss fights", true)
                .clicked()
            {
                self.modals.show_combat_clear_confirm = true;
            }
            ui.label(
                RichText::new(
                    "Permanently removes every recorded fight from the Combat History database.",
                )
                .small()
                .weak(),
            );
        });

        if settings_changed {
            if let Ok(mut s) = self.settings.write() {
                s.combat_history = current.clone();
                s.save();
            }
            self.worker
                .send_control(ControlMsg::SetCombatHistorySettings(current));
        }
    }

    /// Confirmation dialog for clearing all combat history. Rendered on the
    /// `Order::Tooltip` layer (matching the settings dialog) after the settings
    /// area closes, so it sits on top of the settings panel rather than behind
    /// it. On confirm, clears the database and refreshes the panel.
    fn render_combat_clear_confirm(
        &mut self,
        ctx: &egui::Context,
        shadcn: &crate::shadcn_ui::Shadcn,
    ) {
        let mut do_clear = false;
        let mut do_cancel = false;

        egui::Area::new(egui::Id::new("combat_clear_confirm"))
            .order(egui::Order::Tooltip)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(egui::Margin::same(16))
                    .show(ui, |ui| {
                        ui.set_max_width(360.0);
                        ui.heading("Delete all boss fight logs?");
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new("⚠ This permanently deletes ALL recorded boss fights.")
                                .color(Color32::from_rgb(255, 200, 100)),
                        );
                        ui.add_space(4.0);
                        ui.label("This cannot be undone.");
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            if shadcn.button(ui, "Cancel").clicked() {
                                do_cancel = true;
                            }
                            if shadcn
                                .button_destructive(ui, "Yes, delete all", true)
                                .clicked()
                            {
                                do_clear = true;
                            }
                        });
                    });
            });

        if do_clear {
            match CombatDatabase::open_writer(self.persistence.combat_database()) {
                Ok(mut writer) => match writer.clear_all() {
                    Ok(()) => {
                        self.combat_panel.request_reload();
                        tracing::info!("[COMBAT] all history cleared by user");
                    }
                    Err(e) => tracing::warn!("[COMBAT] clear_all failed: {}", e),
                },
                Err(e) => tracing::warn!("[COMBAT] open writer for clear_all failed: {}", e),
            }
            self.modals.show_combat_clear_confirm = false;
        }
        if do_cancel {
            self.modals.show_combat_clear_confirm = false;
        }
    }

    /// Render the Loot Tracking settings panel.
    fn render_loot_tracking_settings(
        &mut self,
        ui: &mut egui::Ui,
        shadcn: &crate::shadcn_ui::Shadcn,
    ) {
        ui.add_space(10.0);
        ui.heading("Loot Tracking");
        ui.add_space(5.0);
        ui.label(
            RichText::new("Configure which loot bags are captured and stored in the database.")
                .weak(),
        );
        ui.add_space(15.0);

        // Get current settings
        let mut settings_changed = false;
        let mut current_settings = {
            let s = self.settings.read().unwrap();
            s.loot_tracking.clone()
        };

        // -- Section 1: Bag types --
        shadcn.card(ui, "loot_bag_colors", "Track bags by color", |ui| {
            ui.label(
                RichText::new("Always track these bag types regardless of contents.")
                    .small()
                    .weak(),
            );
            ui.add_space(8.0);
            ui.columns(2, |cols| {
                let left = &mut cols[0];
                left.spacing_mut().item_spacing.y = 6.0;
                settings_changed |= shadcn
                    .switch(left, &mut current_settings.track_white, "White bags")
                    .changed();
                settings_changed |= shadcn
                    .switch(left, &mut current_settings.track_red, "Red bags")
                    .changed();
                settings_changed |= shadcn
                    .switch(left, &mut current_settings.track_blue, "Blue bags")
                    .changed();
                settings_changed |= shadcn
                    .switch(left, &mut current_settings.track_teal, "Teal bags")
                    .changed();
                settings_changed |= shadcn
                    .switch(left, &mut current_settings.track_pink, "Pink bags")
                    .changed();
                settings_changed |= shadcn
                    .switch(left, &mut current_settings.track_brown, "Brown bags")
                    .changed();

                let right = &mut cols[1];
                right.spacing_mut().item_spacing.y = 6.0;
                settings_changed |= shadcn
                    .switch(right, &mut current_settings.track_orange, "Orange bags")
                    .changed();
                settings_changed |= shadcn
                    .switch(right, &mut current_settings.track_gold, "Gold bags")
                    .changed();
                settings_changed |= shadcn
                    .switch(right, &mut current_settings.track_egg, "Egg bags")
                    .changed();
                settings_changed |= shadcn
                    .switch(right, &mut current_settings.track_purple, "Purple bags")
                    .changed();
                settings_changed |= shadcn
                    .switch(
                        right,
                        &mut current_settings.track_soulbound,
                        "Soulbound bags",
                    )
                    .changed();
            });
        });

        ui.add_space(12.0);

        // -- Section 2: Content filters --
        shadcn.card(
            ui,
            "loot_content_filters",
            "Also track any bag containing",
            |ui| {
                ui.label(
                    RichText::new("Track bags of any color if they contain a matching item.")
                        .small()
                        .weak(),
                );
                ui.add_space(8.0);

                settings_changed |= shadcn
                    .switch(ui, &mut current_settings.track_shiny, "Shiny items")
                    .changed();
                ui.add_space(6.0);

                ui.label(RichText::new("Items with special enchants:").weak());
                ui.indent("enchant_sub", |ui| {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    ui.horizontal(|ui| {
                        settings_changed |= shadcn
                            .switch(
                                ui,
                                &mut current_settings.track_loot_enchants,
                                "Loot enchants",
                            )
                            .changed();
                        ui.label(
                            RichText::new("(Lucky Streak, Loot Bonus III/IV)")
                                .small()
                                .weak(),
                        );
                    });
                    ui.horizontal(|ui| {
                        settings_changed |= shadcn
                            .switch(
                                ui,
                                &mut current_settings.track_unique_enchants,
                                "Unique enchants",
                            )
                            .changed();
                        ui.label(
                            RichText::new("(Candy Coated, Sandstone Resilience, etc.)")
                                .small()
                                .weak(),
                        );
                    });
                    settings_changed |= shadcn
                        .switch(
                            ui,
                            &mut current_settings.track_awakened_enchants,
                            "Awakened enchants",
                        )
                        .changed();
                });
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    settings_changed |= shadcn
                        .switch(
                            ui,
                            &mut current_settings.track_legendary_plus,
                            "Legendary+ rarity",
                        )
                        .changed();
                    ui.label(
                        RichText::new("(Legendary and Divine rarity items)")
                            .small()
                            .weak(),
                    );
                });
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    settings_changed |= shadcn
                        .switch(ui, &mut current_settings.track_ut_items, "UT items")
                        .changed();
                    ui.label(RichText::new("(Untiered items)").small().weak());
                });
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    settings_changed |= shadcn
                        .switch(ui, &mut current_settings.track_potions, "Potions")
                        .changed();
                    ui.label(
                        RichText::new("(Stat, soulbound, greater potions)")
                            .small()
                            .weak(),
                    );
                });
                ui.add_space(6.0);

                settings_changed |= shadcn
                    .switch(ui, &mut current_settings.track_marks, "Marks")
                    .changed();
                ui.add_space(6.0);

                // Tiered items + minimum-tier select.
                shadcn.field_row(ui, |ui| {
                    settings_changed |= shadcn
                        .switch(ui, &mut current_settings.track_tiered_items, "Tiered items")
                        .changed();
                    let tier_options: Vec<(String, String)> =
                        (0..=14).map(|t| (t.to_string(), format!("T{t}"))).collect();
                    let option_refs: Vec<(&str, &str)> = tier_options
                        .iter()
                        .map(|(v, l)| (v.as_str(), l.as_str()))
                        .collect();
                    let mut tier_sel = Some(current_settings.min_tiered_tier.to_string());
                    if shadcn
                        .select(ui, "min_tiered_tier", &mut tier_sel, 70.0, &option_refs)
                        .changed()
                    {
                        if let Some(parsed) =
                            tier_sel.as_deref().and_then(|s| s.parse::<i32>().ok())
                        {
                            current_settings.min_tiered_tier = parsed;
                            settings_changed = true;
                        }
                    }
                    ui.label("or above");
                });
            },
        );

        // Save settings if changed
        if settings_changed {
            // Clamp tier to valid range
            current_settings.min_tiered_tier = current_settings.min_tiered_tier.clamp(0, 14);
            // Update settings file
            if let Ok(mut s) = self.settings.write() {
                s.loot_tracking = current_settings.clone();
                s.save();
            }
            // Update loot tracker
            self.worker
                .send_control(ControlMsg::SetLootTrackingSettings(current_settings));
        }
    }

    /// Draw a dungeon's portal sprite into an inline `size`x`size` slot. Draws
    /// nothing (leaving the reserved space blank) when the dungeon has no known
    /// portal id.
    fn render_dungeon_portal_icon(&mut self, ui: &mut egui::Ui, name: &str, size: f32) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        if !ui.is_rect_visible(rect) {
            return;
        }
        if let Some(id) = realmhound_core::assets::get_dungeon_portal_map().get_portal_id(name) {
            self.sprite_renderer.draw_sprite_in_rect(ui, id, rect);
        }
    }

    /// Draw an event encounter's sprite into an inline `size`x`size` slot.
    /// Draws nothing when the encounter name has no known object id.
    fn render_event_encounter_icon(&mut self, ui: &mut egui::Ui, name: &str, size: f32) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        if !ui.is_rect_visible(rect) {
            return;
        }
        let id = {
            let mgr = realmhound_core::assets::get_asset_manager();
            mgr.object_id_for_name(name)
                .or_else(|| mgr.object_id_for_display_name(name))
        };
        if let Some(id) = id {
            self.sprite_renderer.draw_sprite_in_rect(ui, id, rect);
        }
    }

    /// Draw the appropriate leading icon for a picker/slang row.
    fn render_picker_icon(&mut self, ui: &mut egui::Ui, icon: PickerIcon, name: &str, size: f32) {
        match icon {
            PickerIcon::Portal => self.render_dungeon_portal_icon(ui, name, size),
            PickerIcon::Encounter => self.render_event_encounter_icon(ui, name, size),
            PickerIcon::None => {}
        }
    }

    /// A reusable searchable dropdown picker matching the Sound-tab enchant
    /// picker: a text box that opens a floating, scrollable popup of matching
    /// candidates with keyboard (arrow/enter) and click selection. `candidates`
    /// are `(key, display)`; returns the chosen `key` on selection. Popup state
    /// is stored per `picker_id` in `self.event_pickers`.
    fn render_search_picker(
        &mut self,
        ui: &mut egui::Ui,
        shadcn: &crate::shadcn_ui::Shadcn,
        picker_id: &'static str,
        label: &str,
        hint: &str,
        icon: PickerIcon,
        candidates: &[(String, String)],
    ) -> Option<String> {
        let state = self.event_pickers.entry(picker_id).or_default();
        if state.text != state.prev_text {
            state.selected = None;
            state.prev_text = state.text.clone();
        }

        let response = ui
            .horizontal(|ui| {
                if !label.is_empty() {
                    ui.label(label);
                }
                let r = ui.add(
                    egui::TextEdit::singleline(&mut state.text)
                        .hint_text(hint)
                        .desired_width(240.0),
                );
                if !state.text.is_empty() && shadcn.btn(ui, "\u{2715}").clicked() {
                    state.text.clear();
                    state.popup_open = false;
                    state.selected = None;
                }
                r
            })
            .inner;

        let enter_pressed = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if response.has_focus() {
            let state = self.event_pickers.entry(picker_id).or_default();
            state.popup_open = true;
        }

        let query = self
            .event_pickers
            .get(picker_id)
            .map(|s| s.text.trim().to_lowercase())
            .unwrap_or_default();
        let results: Vec<(String, String)> = candidates
            .iter()
            .filter(|(_, disp)| query.is_empty() || disp.to_lowercase().contains(&query))
            .cloned()
            .collect();
        let total = results.len();

        let state = self.event_pickers.entry(picker_id).or_default();
        let popup_open = state.popup_open;
        if popup_open && response.has_focus() && total > 0 {
            let key_down = ui.input(|i| i.key_pressed(egui::Key::ArrowDown));
            let key_up = ui.input(|i| i.key_pressed(egui::Key::ArrowUp));
            let key_escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
            if key_escape {
                state.popup_open = false;
                state.selected = None;
            } else if key_down {
                state.selected = Some(match state.selected {
                    None => 0,
                    Some(i) => (i + 1).min(total - 1),
                });
            } else if key_up {
                state.selected = match state.selected {
                    None => None,
                    Some(0) => None,
                    Some(i) => Some(i - 1),
                };
            }
        }

        let mut chosen: Option<String> = None;
        let state = self.event_pickers.entry(picker_id).or_default();
        if enter_pressed && state.popup_open && total > 0 {
            let idx = state.selected.unwrap_or(0).min(total - 1);
            chosen = results.get(idx).map(|(k, _)| k.clone());
        }
        let selected = state.selected;

        if popup_open && total > 0 {
            let popup_id = ui.make_persistent_id(format!("search_picker_{picker_id}"));
            let popup_pos = response.rect.left_bottom() + egui::vec2(0.0, 2.0);
            let area = egui::Area::new(popup_id)
                .fixed_pos(popup_pos)
                .order(egui::Order::Tooltip)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_min_width(260.0);
                        ui.set_max_height(320.0);
                        egui::ScrollArea::vertical()
                            .max_height(300.0)
                            .show(ui, |ui| {
                                for (row, (key, disp)) in results.iter().enumerate() {
                                    let is_selected = selected == Some(row);
                                    ui.horizontal(|ui| {
                                        if icon != PickerIcon::None {
                                            self.render_picker_icon(ui, icon, disp, 20.0);
                                        }
                                        let resp =
                                            ui.add(egui::Button::new(disp).selected(is_selected));
                                        if is_selected {
                                            resp.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        if resp.clicked() {
                                            chosen = Some(key.clone());
                                        }
                                    });
                                }
                            });
                    });
                });
            ui.ctx().move_to_top(area.response.layer_id);
            if ui.input(|i| i.pointer.any_click()) && !response.has_focus() && chosen.is_none() {
                let state = self.event_pickers.entry(picker_id).or_default();
                state.popup_open = false;
                state.selected = None;
            }
        }

        if chosen.is_some() {
            let state = self.event_pickers.entry(picker_id).or_default();
            state.text.clear();
            state.prev_text.clear();
            state.popup_open = false;
            state.selected = None;
        }
        chosen
    }

    /// Build the dungeon slang-editor entries `(display_name, default_short)`
    /// from the Trophy Hall dungeon list ([`ALL_DUNGEONS`]), so the dropdown
    /// matches the game's dungeon list exactly. Dungeons with no curated
    /// nickname get an empty default (still editable).
    fn dungeon_slang_entries() -> Vec<(String, String)> {
        realmhound_core::stats::ALL_DUNGEONS
            .iter()
            .filter(|d| {
                // Skip dungeons that never produce a key callout: instance-only
                // ones and the Oryx realm endgame.
                !crate::panels::dungeon_callout::is_non_callable(d.name)
                    && !d.name.starts_with("Oryx's")
            })
            .map(|d| {
                let short = crate::panels::dungeon_callout::dungeon_nickname(d.name)
                    .unwrap_or("")
                    .to_string();
                (d.name.to_string(), short)
            })
            .collect()
    }

    /// Build the event slang-editor entries `(display_name, default_short)` from
    /// the same encounter catalogs shown in the Sound-tab Encounter settings
    /// (Veteran + Adept + Seasonal), deduped and sorted A-Z. Encounters with no
    /// curated short name get an empty default (still editable).
    fn event_slang_entries() -> Vec<(String, String)> {
        use crate::realm_event_sound::{section_catalog, SectionKind};
        let mut seen = std::collections::HashSet::new();
        let mut out: Vec<(String, String)> = Vec::new();
        for kind in [
            SectionKind::Veteran,
            SectionKind::Adept,
            SectionKind::Seasonal,
        ] {
            for entry in section_catalog(kind) {
                if seen.insert(entry.name.to_lowercase()) {
                    let short = crate::panels::event_call::event_short_name(&entry.name)
                        .unwrap_or("")
                        .to_string();
                    out.push((entry.name.clone(), short));
                }
            }
        }
        out.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
        out
    }

    /// Render the searchable slang-name override editor used by both the Dungeon
    /// and Event callout cards. Presents a search box over all `entries`
    /// (display name, default short) A-Z; picking one reveals a single editable
    /// row with a short-name override field. Existing overrides are listed below
    /// so they stay visible and removable. Returns true if an override changed.
    fn render_slang_editor(
        &mut self,
        ui: &mut egui::Ui,
        shadcn: &crate::shadcn_ui::Shadcn,
        id_prefix: &str,
        picker_id: &'static str,
        entries: &[(String, String)],
        overrides: &mut std::collections::BTreeMap<String, String>,
        icon: PickerIcon,
        search_hint: &str,
    ) -> bool {
        let mut changed = false;
        let sel_id = egui::Id::new((id_prefix, "slang_selected"));
        let mut selected: String = ui.data_mut(|d| d.get_temp(sel_id).unwrap_or_default());

        // Searchable dropdown to pick the entry to edit. Reuses the scrollable
        // floating picker so clicks/scroll behave like the Sound-tab pickers.
        let candidates: Vec<(String, String)> = entries
            .iter()
            .map(|(disp, _)| (disp.clone(), disp.clone()))
            .collect();
        if let Some(pick) = self.render_search_picker(
            ui,
            shadcn,
            picker_id,
            "Find:",
            search_hint,
            icon,
            &candidates,
        ) {
            selected = pick;
        }

        // Single editable row for the current selection. The edit buffer lives
        // in temp memory (untrimmed) so multi-word overrides like "inf abyss"
        // keep their interim spaces while typing; only a trimmed value is
        // committed to the overrides map.
        if !selected.is_empty() {
            if let Some((disp, default_short)) = entries
                .iter()
                .find(|(d, _)| d.as_str() == selected.as_str())
            {
                let disp = disp.as_str();
                let default_short = default_short.as_str();
                let buf_id = egui::Id::new((id_prefix, "slang_buffer"));
                let buf_for_id = egui::Id::new((id_prefix, "slang_buffer_for"));
                let buf_for: String = ui.data_mut(|d| d.get_temp(buf_for_id).unwrap_or_default());
                let mut text: String = if buf_for == selected {
                    ui.data_mut(|d| d.get_temp(buf_id).unwrap_or_default())
                } else {
                    overrides.get(disp).cloned().unwrap_or_default()
                };
                ui.add_space(4.0);
                shadcn.field_row(ui, |ui| {
                    self.render_picker_icon(ui, icon, disp, 22.0);
                    ui.label(RichText::new(disp).strong());
                    let hint = if default_short.is_empty() {
                        "(no short name yet)"
                    } else {
                        default_short
                    };
                    if shadcn
                        .text_edit_counted(ui, &mut text, 24, 120.0, Some(hint))
                        .changed()
                    {
                        let t = text.trim();
                        if t.is_empty() {
                            overrides.remove(disp);
                        } else {
                            overrides.insert(disp.to_string(), t.to_string());
                        }
                        changed = true;
                    }
                    if shadcn.btn_small(ui, "Done").clicked() {
                        selected.clear();
                    }
                });
                ui.data_mut(|d| {
                    d.insert_temp(buf_id, text);
                    d.insert_temp(buf_for_id, selected.clone());
                });
            }
        }

        // Existing overrides remain visible and removable.
        if !overrides.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new("Custom names:").weak().small());
            let mut items: Vec<(String, String)> = overrides
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            items.sort();
            let mut to_remove: Option<String> = None;
            for (disp, val) in items {
                ui.horizontal(|ui| {
                    self.render_picker_icon(ui, icon, &disp, 18.0);
                    ui.label(RichText::new(format!("{disp}  ->  {val}")).small());
                    if shadcn.btn_small(ui, "x").clicked() {
                        to_remove = Some(disp.clone());
                    }
                });
            }
            if let Some(k) = to_remove {
                overrides.remove(&k);
                changed = true;
            }
        }

        ui.data_mut(|d| {
            d.insert_temp(sel_id, selected);
        });
        changed
    }

    /// Format a callout body with the join marker applied for the settings
    /// preview: `[j ]<body>[ j]` per the given placement.
    fn preview_with_join(body: &str, join: realmhound_core::settings::JoinPosition) -> String {
        use realmhound_core::settings::JoinPosition;
        match join {
            JoinPosition::Beginning => format!("j {body}"),
            JoinPosition::End => format!("{body} j"),
            JoinPosition::None => body.to_string(),
        }
    }

    /// Render the Live Feed settings panel.
    fn render_live_feed_settings(&mut self, ui: &mut egui::Ui, shadcn: &crate::shadcn_ui::Shadcn) {
        use realmhound_core::settings::{
            DungeonNameStyle, DustLabel, JoinPosition, LootLabel, XpLabel,
        };

        ui.add_space(10.0);
        ui.heading("Live Feed");
        ui.add_space(5.0);
        ui.label(
            RichText::new("Choose what appears in the Live Feed and how callouts are formatted.")
                .weak(),
        );
        ui.add_space(15.0);

        let mut settings_changed = false;
        let mut current = {
            let s = self.settings.read().unwrap();
            s.live_feed.clone()
        };
        // Party moderation is a mirror of the Party tab's Moderation Tools
        // toggle; edits here sync back to it.
        let mut mod_tools = {
            let s = self.settings.read().unwrap();
            s.party.mod_tools
        };
        let mut mod_tools_changed = false;

        shadcn.card(ui, "lf_notifications", "Notifications", |ui| {
            settings_changed |= shadcn
                .switch(ui, &mut current.show_dm_in_live_feed, "Direct messages")
                .hover_tip("Show incoming whispers as entries in the Live Feed.")
                .changed();
            ui.add_space(4.0);
            settings_changed |= shadcn
                .switch(ui, &mut current.show_dungeon_entries, "Dungeon entries")
                .hover_tip("Show the callout entry created when you enter a dungeon.")
                .changed();
            ui.add_space(4.0);
            settings_changed |= shadcn
                .switch(ui, &mut current.show_boss_calls, "Realm event bosses")
                .hover_tip(
                    "Show realm event and boss announcements (e.g. Skull Shrine, Cube God, \
                     Avatar of the Forgotten King).",
                )
                .changed();
            ui.add_space(4.0);
            settings_changed |= shadcn
                .switch(ui, &mut current.show_key_pops, "Key pops")
                .hover_tip(
                    "Show nearby players popping dungeon keys. Click an entry to copy a \
                     \"Thanks <name> for the key\" callout.",
                )
                .changed();
            if current.show_key_pops {
                ui.indent("lf_key_pop_tiers", |ui| {
                    ui.label(
                        RichText::new(
                            "Only show these difficulty tiers (unknown ratings always show):",
                        )
                        .weak()
                        .small(),
                    );
                    let f = &mut current.key_pop_tiers;
                    ui.horizontal_wrapped(|ui| {
                        settings_changed |= shadcn
                            .switch(ui, &mut f.rookie, "Rookie")
                            .hover_tip("Grave difficulty 2 or lower (e.g. Spider Den, The Hive).")
                            .changed();
                        settings_changed |= shadcn
                            .switch(ui, &mut f.adept, "Adept")
                            .hover_tip("Grave difficulty above 2 up to 4.5 (e.g. Snake Pit, Abyss of Demons).")
                            .changed();
                        settings_changed |= shadcn
                            .switch(ui, &mut f.expert, "Expert")
                            .hover_tip("Grave difficulty above 4.5 up to 6.5 (e.g. Lair of Draconis, Ocean Trench).")
                            .changed();
                        settings_changed |= shadcn
                            .switch(ui, &mut f.exaltation, "Exaltation")
                            .hover_tip("Grave difficulty above 6.5 (e.g. The Nest, Lost Halls, The Shatters).")
                            .changed();
                    });
                });
            }
            ui.add_space(4.0);
            settings_changed |= shadcn
                .switch(ui, &mut current.show_area_unlocks, "Area unlocks")
                .hover_tip(
                    "Show when nearby players pop area unlocks (Wine Cellar Incantation, Vial of \
                     Pure Darkness, Lost Halls rune monuments). Click an entry to copy a \
                     \"Thanks ...\" callout.",
                )
                .changed();
            ui.add_space(4.0);
            settings_changed |= shadcn
                .switch(ui, &mut current.show_who_roster, "Player list (/who)")
                .hover_tip(
                    "Click to copy the list of players inside your location, A-Z, one name per \
                     line.",
                )
                .changed();
            ui.add_space(4.0);
            settings_changed |= shadcn
                .switch(ui, &mut current.show_loot_drops, "Loot drops")
                .hover_tip(
                    "Show loot-drop entries (white/red/etc. bags) in the feed. Disabling only \
                     removes them from Live Feed, they still show up in Loot History.",
                )
                .changed();
        });

        ui.add_space(12.0);

        shadcn.card(ui, "lf_warnings", "Warnings", |ui| {
            settings_changed |= shadcn
                .switch(
                    ui,
                    &mut current.show_realm_warnings,
                    "Realm close & Oryx lag",
                )
                .hover_tip(
                    "Show the \"Realm closed!\" and \"Oryx lag incoming\" warnings in the feed.",
                )
                .changed();

            ui.add_space(4.0);
            settings_changed |= shadcn
                .switch(ui, &mut current.show_dust_full, "Dust cap full")
                .hover_tip("Show a warning when enchanting dust of a certain type reaches its cap.")
                .changed();

            ui.add_space(4.0);
            if shadcn
                .switch(ui, &mut mod_tools, "Party moderation")
                .hover_tip(
                    "Show a warning when a person from a banlist is in the party or requests to \
                     join. Synced with the Moderation Tools toggle in the Party tab.",
                )
                .changed()
            {
                mod_tools_changed = true;
            }

            ui.add_space(8.0);
            ui.label(RichText::new("End-of-cycle warnings").strong());
            ui.label(
                RichText::new(
                    "Pin a reminder to the top of the Live Feed on the final day before a reset. \
                     Season and battlepass end dates populate automatically from live game data \
                     when you refresh your account.",
                )
                .weak()
                .small(),
            );
            ui.add_space(4.0);
            settings_changed |= shadcn
                .switch(
                    ui,
                    &mut current.season_warnings.daily_calendar_enabled,
                    "Daily login calendar warning",
                )
                .hover_tip(
                    "Automatic: warns on the last day of each month (resets 00:00 UTC) to claim \
                     your daily login calendar items.",
                )
                .changed();
            ui.add_space(4.0);
            settings_changed |= shadcn
                .switch(
                    ui,
                    &mut current.season_warnings.battlepass_enabled,
                    "Battlepass ending warning",
                )
                .hover_tip("Warns on the last day before the configured battlepass reset.")
                .changed();
            ui.add_space(4.0);
            settings_changed |= shadcn
                .switch(
                    ui,
                    &mut current.season_warnings.season_enabled,
                    "Season ending warning",
                )
                .hover_tip(
                    "Warns on the last day before the configured season reset, with a hover \
                     tooltip explaining how seasonal storage transfers.",
                )
                .changed();
        });

        ui.add_space(12.0);

        shadcn.card(ui, "lf_dungeon_callouts", "Dungeon Callouts", |ui| {
            // Join marker placement (dungeon default: end).
            shadcn.field_row(ui, |ui| {
                ui.label("Join marker (j):");
                let cur = match current.dungeon_join_position {
                    JoinPosition::End => "end",
                    JoinPosition::Beginning => "start",
                    JoinPosition::None => "none",
                };
                let mut sel = Some(cur.to_string());
                if shadcn
                    .select(
                        ui,
                        "dungeon_join_position",
                        &mut sel,
                        140.0,
                        &[("start", "at start"), ("end", "at end"), ("none", "none")],
                    )
                    .hover_tip("Where the join marker \"j\" is placed in dungeon callouts.")
                    .changed()
                {
                    current.dungeon_join_position = match sel.as_deref() {
                        Some("start") => JoinPosition::Beginning,
                        Some("none") => JoinPosition::None,
                        _ => JoinPosition::End,
                    };
                    settings_changed = true;
                }
            });

            ui.add_space(6.0);

            shadcn.field_row(ui, |ui| {
                ui.label("Dungeon names:");
                let cur = match current.dungeon_name_style {
                    DungeonNameStyle::Short => "short",
                    DungeonNameStyle::Full => "full",
                };
                let mut sel = Some(cur.to_string());
                if shadcn
                    .select(
                        ui,
                        "dungeon_name_style",
                        &mut sel,
                        140.0,
                        &[("short", "short (slang)"), ("full", "full names")],
                    )
                    .hover_tip(
                        "Short uses curated nicknames (e.g. \"halls\"); Full uses the dungeon's \
                         full name lowercased (e.g. \"lost halls\"). Only affects clipboard text.",
                    )
                    .changed()
                {
                    current.dungeon_name_style = match sel.as_deref() {
                        Some("full") => DungeonNameStyle::Full,
                        _ => DungeonNameStyle::Short,
                    };
                    settings_changed = true;
                }
            });

            ui.add_space(4.0);
            ui.label(RichText::new("Edit slang names").strong());
            ui.label(
                RichText::new(
                    "Search a dungeon, then override its short name. Leave blank to keep the \
                     default (shown as a hint).",
                )
                .weak()
                .small(),
            );
            ui.add_space(4.0);
            {
                let entries = Self::dungeon_slang_entries();
                if self.render_slang_editor(
                    ui,
                    shadcn,
                    "dungeon",
                    "slang_dungeon",
                    &entries,
                    &mut current.dungeon_name_overrides,
                    PickerIcon::Portal,
                    "Type a dungeon name...",
                ) {
                    settings_changed = true;
                }
            }

            ui.add_space(8.0);

            // Reward-value label modes.
            shadcn.field_row(ui, |ui| {
                ui.label("Loot boost:");
                let cur = match current.loot_label {
                    LootLabel::Lb => "lb",
                    LootLabel::Loot => "loot",
                    LootLabel::None => "none",
                };
                let mut sel = Some(cur.to_string());
                if shadcn
                    .select(
                        ui,
                        "loot_label",
                        &mut sel,
                        120.0,
                        &[("lb", "lb"), ("loot", "loot"), ("none", "none")],
                    )
                    .changed()
                {
                    current.loot_label = match sel.as_deref() {
                        Some("loot") => LootLabel::Loot,
                        Some("none") => LootLabel::None,
                        _ => LootLabel::Lb,
                    };
                    settings_changed = true;
                }
            });

            ui.add_space(6.0);

            shadcn.field_row(ui, |ui| {
                ui.label("Dust boost:");
                let cur = match current.dust_label {
                    DustLabel::Db => "db",
                    DustLabel::Dust => "dust",
                    DustLabel::None => "none",
                };
                let mut sel = Some(cur.to_string());
                if shadcn
                    .select(
                        ui,
                        "dust_label",
                        &mut sel,
                        120.0,
                        &[("db", "db"), ("dust", "dust"), ("none", "none")],
                    )
                    .changed()
                {
                    current.dust_label = match sel.as_deref() {
                        Some("dust") => DustLabel::Dust,
                        Some("none") => DustLabel::None,
                        _ => DustLabel::Db,
                    };
                    settings_changed = true;
                }
            });

            ui.add_space(6.0);

            shadcn.field_row(ui, |ui| {
                ui.label("XP boost:");
                let cur = match current.xp_label {
                    XpLabel::Xp => "xp",
                    XpLabel::None => "none",
                };
                let mut sel = Some(cur.to_string());
                if shadcn
                    .select(
                        ui,
                        "xp_label",
                        &mut sel,
                        120.0,
                        &[("xp", "xp"), ("none", "none")],
                    )
                    .hover_tip("XP boosts are rarely called; off by default.")
                    .changed()
                {
                    current.xp_label = match sel.as_deref() {
                        Some("xp") => XpLabel::Xp,
                        _ => XpLabel::None,
                    };
                    settings_changed = true;
                }
            });

            ui.add_space(6.0);

            settings_changed |= shadcn
                .switch(
                    ui,
                    &mut current.callout_percent,
                    "Include % sign in loot/dust/xp values",
                )
                .hover_tip(
                    "When on, reward bonuses include the percent sign (e.g. \"15% lb\"). \
                     Off by default (e.g. \"15 lb\").",
                )
                .changed();

            ui.add_space(10.0);
            ui.label(RichText::new("Reward mods").strong());
            ui.label(
                RichText::new(
                    "These mods get a tag in callouts. Toggle one off to stop calling it, or edit \
                     its short call. Use the search to add any other mod. Mods with a blank call \
                     are never added.",
                )
                .weak()
                .small(),
            );
            ui.add_space(4.0);
            {
                let revealed_id = egui::Id::new("reward_mods_revealed");
                let mut revealed: Vec<String> =
                    ui.data_mut(|d| d.get_temp(revealed_id).unwrap_or_default());

                // Search box at the top: a scrollable floating picker of mods not
                // already shown. Picking one enables + reveals it.
                let candidates: Vec<(String, String)> = current
                    .reward_mods
                    .iter()
                    .filter(|e| e.id != "DIMITUS" && !revealed.contains(&e.id))
                    .map(|e| (e.id.clone(), e.name.clone()))
                    .collect();
                if let Some(id) = self.render_search_picker(
                    ui,
                    shadcn,
                    "reward_mods_add",
                    "Add mod:",
                    "Type a reward or unique mod name...",
                    PickerIcon::None,
                    &candidates,
                ) {
                    if let Some(e) = current.reward_mods.iter_mut().find(|e| e.id == id) {
                        e.enabled = true;
                    }
                    revealed.push(id);
                    settings_changed = true;
                }

                ui.add_space(6.0);

                // Dimitus leads the list and is always shown (even when off): its
                // toggle governs the whole callout, so it needs a permanent home.
                if let Some(dim) = current.reward_mods.iter_mut().find(|e| e.id == "DIMITUS") {
                    ui.horizontal(|ui| {
                        if shadcn.switch(ui, &mut dim.enabled, "").changed() {
                            settings_changed = true;
                        }
                        fixed_cell(ui, 200.0, 24.0, |ui| {
                            ui.label(RichText::new(&dim.name));
                        });
                        if shadcn
                            .text_edit_counted(ui, &mut dim.short, 16, 110.0, Some("(no call)"))
                            .changed()
                        {
                            settings_changed = true;
                        }
                        ui.label(RichText::new("\u{24d8}").weak()).on_hover_text(
                            "OFF disables the whole dungeon callout that has this mod",
                        );
                    });
                }

                // Rows for mods that are on (auto-revealed so they persist even
                // after being toggled off this session) plus any surfaced via
                // search. Layout: toggle, full name, then short call.
                for entry in current.reward_mods.iter_mut() {
                    if entry.id == "DIMITUS" {
                        continue;
                    }
                    if entry.enabled && !revealed.contains(&entry.id) {
                        revealed.push(entry.id.clone());
                    }
                    if !revealed.contains(&entry.id) {
                        continue;
                    }
                    ui.horizontal(|ui| {
                        if shadcn.switch(ui, &mut entry.enabled, "").changed() {
                            settings_changed = true;
                        }
                        fixed_cell(ui, 200.0, 24.0, |ui| {
                            ui.label(RichText::new(&entry.name));
                        });
                        if shadcn
                            .text_edit_counted(ui, &mut entry.short, 16, 110.0, Some("(no call)"))
                            .changed()
                        {
                            settings_changed = true;
                        }
                    });
                }

                ui.data_mut(|d| {
                    d.insert_temp(revealed_id, revealed);
                });
            }

            ui.add_space(8.0);
            // Live green preview mirroring a real clipboard callout.
            let preview = {
                let params = crate::panels::dungeon_callout::DungeonCalloutParams {
                    name_style: current.dungeon_name_style,
                    name_overrides: &current.dungeon_name_overrides,
                    loot_label: current.loot_label,
                    dust_label: current.dust_label,
                    xp_label: current.xp_label,
                    percent: current.callout_percent,
                    reward_mods: &current.reward_mods,
                };
                let tokens = [
                    "REWARDING".to_string(),
                    "GENEROUS".to_string(),
                    "KEYFAIRY".to_string(),
                ];
                let body = crate::panels::dungeon_callout::dungeon_callout_for(
                    "The Shatters",
                    &tokens,
                    &params,
                )
                .unwrap_or_default();
                Self::preview_with_join(&body, current.dungeon_join_position)
            };
            ui.label(RichText::new("Preview:").weak().small());
            ui.label(
                RichText::new(preview)
                    .monospace()
                    .color(Color32::from_rgb(120, 220, 120)),
            );
        });

        ui.add_space(12.0);

        shadcn.card(ui, "lf_event_callouts", "Event Callouts", |ui| {
            shadcn.field_row(ui, |ui| {
                ui.label("Join marker (j):");
                let cur = match current.event_join_position {
                    JoinPosition::End => "end",
                    JoinPosition::Beginning => "start",
                    JoinPosition::None => "none",
                };
                let mut sel = Some(cur.to_string());
                if shadcn
                    .select(
                        ui,
                        "event_join_position",
                        &mut sel,
                        140.0,
                        &[("start", "at start"), ("end", "at end"), ("none", "none")],
                    )
                    .hover_tip("Where the join marker \"j\" is placed in event callouts.")
                    .changed()
                {
                    current.event_join_position = match sel.as_deref() {
                        Some("start") => JoinPosition::Beginning,
                        Some("end") => JoinPosition::End,
                        _ => JoinPosition::None,
                    };
                    settings_changed = true;
                }
            });

            ui.add_space(6.0);

            shadcn.field_row(ui, |ui| {
                ui.label("Event names:");
                let cur = match current.event_name_style {
                    DungeonNameStyle::Short => "short",
                    DungeonNameStyle::Full => "full",
                };
                let mut sel = Some(cur.to_string());
                if shadcn
                    .select(
                        ui,
                        "event_name_style",
                        &mut sel,
                        140.0,
                        &[("short", "short (slang)"), ("full", "full names")],
                    )
                    .hover_tip("Only affects clipboard text. The feed always shows full names.")
                    .changed()
                {
                    current.event_name_style = match sel.as_deref() {
                        Some("full") => DungeonNameStyle::Full,
                        _ => DungeonNameStyle::Short,
                    };
                    settings_changed = true;
                }
            });

            ui.add_space(4.0);
            ui.label(RichText::new("Edit slang names").strong());
            ui.label(
                RichText::new(
                    "Search an event, then override its short name. Leave blank to keep the \
                     default (shown as a hint).",
                )
                .weak()
                .small(),
            );
            ui.add_space(4.0);
            {
                let entries = Self::event_slang_entries();
                if self.render_slang_editor(
                    ui,
                    shadcn,
                    "event",
                    "slang_event",
                    &entries,
                    &mut current.event_name_overrides,
                    PickerIcon::Encounter,
                    "Type an event boss name...",
                ) {
                    settings_changed = true;
                }
            }

            ui.add_space(8.0);

            settings_changed |= shadcn
                .switch(
                    ui,
                    &mut current.event_add_upcoming,
                    "Add upcoming dungeons to event calls",
                )
                .hover_tip(
                    "Appends the dungeon an event is about to drop (e.g. \"rav rot, halls soon\").",
                )
                .changed();

            ui.add_space(8.0);
            let preview = {
                let body = crate::panels::event_call::event_call_body(
                    "Ravenous Rot",
                    current.event_name_style,
                    current.event_add_upcoming,
                    &current.event_name_overrides,
                );
                Self::preview_with_join(&body, current.event_join_position)
            };
            ui.label(RichText::new("Preview:").weak().small());
            ui.label(
                RichText::new(preview)
                    .monospace()
                    .color(Color32::from_rgb(120, 220, 120)),
            );

            ui.add_space(12.0);
            shadcn.full_width_separator(ui);
            ui.add_space(8.0);
            ui.label(RichText::new("Alien Invasion callouts").strong());
            ui.label(
                RichText::new(
                    "Each wave is called as \"<template> <wave number>\". Edit the template text; \
                     the wave number is appended automatically.",
                )
                .weak()
                .small(),
            );
            ui.add_space(4.0);
            shadcn.field_row(ui, |ui| {
                ui.label("Adept template:");
                if shadcn
                    .text_edit_counted(
                        ui,
                        &mut current.alien_adept_prefix,
                        24,
                        160.0,
                        Some("adept wave"),
                    )
                    .changed()
                {
                    settings_changed = true;
                }
            });
            shadcn.field_row(ui, |ui| {
                ui.label("Veteran template:");
                if shadcn
                    .text_edit_counted(
                        ui,
                        &mut current.alien_veteran_prefix,
                        24,
                        160.0,
                        Some("veteran wave"),
                    )
                    .changed()
                {
                    settings_changed = true;
                }
            });
            ui.add_space(4.0);
            settings_changed |= shadcn
                .switch(
                    ui,
                    &mut current.alien_wave4_boss,
                    "Add upcoming boss to wave 4 callout",
                )
                .hover_tip(
                    "Appends the incoming boss on the final wave: \"UFO soon\" for Adept, \
                     \"calbrik soon\" for Veteran.",
                )
                .changed();
            ui.add_space(6.0);
            let alien_adept_preview = Self::preview_with_join(
                &crate::panels::event_call::alien_wave_call_body(
                    false,
                    4,
                    &current.alien_adept_prefix,
                    &current.alien_veteran_prefix,
                    current.alien_wave4_boss,
                ),
                current.event_join_position,
            );
            let alien_vet_preview = Self::preview_with_join(
                &crate::panels::event_call::alien_wave_call_body(
                    true,
                    4,
                    &current.alien_adept_prefix,
                    &current.alien_veteran_prefix,
                    current.alien_wave4_boss,
                ),
                current.event_join_position,
            );
            ui.label(RichText::new("Preview:").weak().small());
            ui.label(
                RichText::new(alien_adept_preview)
                    .monospace()
                    .color(Color32::from_rgb(120, 220, 120)),
            );
            ui.label(
                RichText::new(alien_vet_preview)
                    .monospace()
                    .color(Color32::from_rgb(120, 220, 120)),
            );
        });

        if settings_changed || mod_tools_changed {
            let mut party_after = None;
            if let Ok(mut s) = self.settings.write() {
                s.live_feed = current.clone();
                if mod_tools_changed {
                    s.party.mod_tools = mod_tools;
                    party_after = Some(s.party.clone());
                }
                s.save();
            }
            self.live_feed_panel.apply_live_feed_settings(&current);
            if let Some(p) = party_after {
                self.party_panel.apply_party_settings(&p);
            }
        }
    }

    /// Render the Taskbar settings panel: the row of mission/quest progress
    /// pills shown below the Widget Bar on every tab.
    fn render_taskbar_settings(&mut self, ui: &mut egui::Ui, shadcn: &crate::shadcn_ui::Shadcn) {
        ui.add_space(10.0);
        ui.heading("Taskbar");
        ui.add_space(5.0);
        ui.label(
            RichText::new(
                "A full-width row of mission and daily-quest progress pills, shown below the \
                 Widget Bar on every tab.",
            )
            .weak(),
        );
        ui.add_space(15.0);

        let mut current = {
            let s = self.settings.read().unwrap();
            s.taskbar.clone()
        };
        let mut settings_changed = false;

        shadcn.card(ui, "taskbar_opts", "Taskbar", |ui| {
            ui.label(
                RichText::new(
                    "Use the compass button on each Mission or Daily Quest card to add or remove \
                     it from the bar.",
                )
                .weak()
                .small(),
            );
            ui.add_space(6.0);
            settings_changed |= shadcn
                .switch(ui, &mut current.enabled, "Show Taskbar")
                .hover_tip("Show the mission/quest progress bar below the Widget Bar on every tab.")
                .changed();
            if current.enabled {
                ui.indent("taskbar_opts_inner", |ui| {
                    let missions_switch = shadcn
                        .switch(ui, &mut current.track_missions, "Track missions")
                        .hover_tip(
                            "Show every in-progress mission in the Taskbar with its compass \
                             button on. Turn off to hide all mission pills and switch their \
                             compass buttons off. Individual missions can still be added or \
                             removed with the compass button.",
                        );
                    if missions_switch.changed() {
                        // Master switch: flip every compass to match by dropping
                        // the per-card overrides.
                        current.mission_overrides.clear();
                        settings_changed = true;
                    }
                    ui.add_space(4.0);
                    let quests_switch = shadcn
                        .switch(ui, &mut current.track_quests, "Track daily quests")
                        .hover_tip(
                            "Show every Daily Quest Chest task in the Taskbar with its compass \
                             button on. Turn off to hide all quest pills and switch their \
                             compass buttons off. Individual quests can still be added or \
                             removed with the compass button.",
                        );
                    if quests_switch.changed() {
                        current.quest_overrides.clear();
                        settings_changed = true;
                    }
                    ui.add_space(4.0);
                    settings_changed |= shadcn
                        .switch(
                            ui,
                            &mut current.prioritize_seasonal_mark_progress,
                            "Prioritize seasonal mark progress",
                        )
                        .hover_tip(
                            "Quest pills show remaining seasonal marks even when the regular \
                             marks are already collected. Turn off to show regular mark \
                             progress instead.",
                        )
                        .changed();
                    ui.add_space(4.0);
                    settings_changed |= shadcn
                        .switch(
                            ui,
                            &mut current.show_all_choice_options,
                            "Show all options for multiple choice tasks",
                        )
                        .hover_tip(
                            "Multiple-choice tasks list every matching option in the pill and \
                             tooltip. Turn off to collapse them to just the most-progressed \
                             option.",
                        )
                        .changed();
                });
            }
        });

        if settings_changed {
            if let Ok(mut s) = self.settings.write() {
                s.taskbar = current.clone();
                s.save();
            }
            self.missions_panel.apply_taskbar_settings(&current);
            self.quest_panel.apply_taskbar_settings(&current);
        }
    }

    /// Render the Widget Bar settings panel: which widgets appear
    /// in the account-info bar below the tabs, plus the shared season /
    /// battlepass schedule that drives the timer widgets and the pinned
    /// end-of-cycle warnings.
    fn render_widget_bar_settings(&mut self, ui: &mut egui::Ui, shadcn: &crate::shadcn_ui::Shadcn) {
        use realmhound_core::settings::{
            DustDisplayMode, MaterialsDisplayMode, WidgetBarSettings, WidgetKind,
        };

        ui.add_space(10.0);
        ui.heading("Widget Bar");
        ui.add_space(5.0);
        ui.label(
            RichText::new("Choose which account widgets show below the tabs, and enter the current season schedule.")
                .weak(),
        );
        ui.add_space(15.0);

        let mut changed = false;
        let mut widget_bar = {
            let s = self.settings.read().unwrap();
            s.widget_bar.clone()
        };
        let season = {
            let s = self.settings.read().unwrap();
            s.season.clone()
        };
        let mut live_feed = {
            let s = self.settings.read().unwrap();
            s.live_feed.clone()
        };

        shadcn.card(ui, "wb_widgets", "Widgets", |ui| {
            ui.label(
                RichText::new("Toggle the widgets displayed across all tabs. This order is default but you can manually re-arrange them in the bar.")
                    .weak()
                    .small(),
            );
            ui.add_space(6.0);
            for kind in WidgetBarSettings::ALL {
                let mut enabled = widget_bar.is_enabled(kind);
                if shadcn
                    .switch(ui, &mut enabled, Self::widget_kind_label(kind))
                    .changed()
                {
                    widget_bar.set_enabled(kind, enabled);
                    // Enchanting dust toggle drives the dust display selector:
                    // turning it on returns the selector to "current character
                    // only"; turning it off shows "None".
                    if kind == WidgetKind::Dust && enabled {
                        live_feed.dust_display_mode = DustDisplayMode::CurrentOnly;
                    }
                    // Forge materials toggle drives its display selector the
                    // same way: turning it on returns the selector to "current
                    // character only"; turning it off shows "None".
                    if kind == WidgetKind::Materials && enabled {
                        live_feed.materials_display_mode = MaterialsDisplayMode::CurrentOnly;
                    }
                    changed = true;
                }
            }
        });

        ui.add_space(12.0);

        shadcn.card(ui, "wb_dust_display", "Enchanting dust display", |ui| {
            shadcn.field_row(ui, |ui| {
                ui.label("Dust layout:");
                // "None" is the disabled state: it mirrors the Enchanting dust
                // toggle above, so the selector and toggle stay in sync.
                let dust_on = widget_bar.is_enabled(WidgetKind::Dust);
                let cur = if !dust_on {
                    "none"
                } else {
                    match live_feed.dust_display_mode {
                        DustDisplayMode::OneLine => "one_line",
                        DustDisplayMode::TwoLines => "two_lines",
                        DustDisplayMode::CurrentOnly => "current_only",
                    }
                };
                let mut sel = Some(cur.to_string());
                if shadcn
                    .select(
                        ui,
                        "dust_display_mode",
                        &mut sel,
                        180.0,
                        &[
                            ("none", "None"),
                            ("one_line", "both on one line"),
                            ("two_lines", "stacked (two lines)"),
                            ("current_only", "current character only"),
                        ],
                    )
                    .hover_tip(
                        "How the enchanting-dust widget shows your seasonal and regular dust. \
                         The account matching your current character is shown first. \
                         Choosing None hides the widget.",
                    )
                    .changed()
                {
                    match sel.as_deref() {
                        Some("none") => widget_bar.set_enabled(WidgetKind::Dust, false),
                        Some(mode) => {
                            live_feed.dust_display_mode = match mode {
                                "two_lines" => DustDisplayMode::TwoLines,
                                "current_only" => DustDisplayMode::CurrentOnly,
                                _ => DustDisplayMode::OneLine,
                            };
                            // Picking any real layout re-enables the widget.
                            widget_bar.set_enabled(WidgetKind::Dust, true);
                        }
                        None => {}
                    }
                    changed = true;
                }
            });
        });

        ui.add_space(12.0);

        shadcn.card(
            ui,
            "wb_materials_display",
            "Forge materials display",
            |ui| {
                shadcn.field_row(ui, |ui| {
                    ui.label("Materials layout:");
                    // "None" mirrors the Forge materials toggle above, so the
                    // selector and toggle stay in sync.
                    let materials_on = widget_bar.is_enabled(WidgetKind::Materials);
                    let cur = if !materials_on {
                        "none"
                    } else {
                        match live_feed.materials_display_mode {
                            MaterialsDisplayMode::OneLine => "one_line",
                            MaterialsDisplayMode::TwoLines => "two_lines",
                            MaterialsDisplayMode::CurrentOnly => "current_only",
                        }
                    };
                    let mut sel = Some(cur.to_string());
                    if shadcn
                    .select(
                        ui,
                        "materials_display_mode",
                        &mut sel,
                        180.0,
                        &[
                            ("none", "None"),
                            ("one_line", "both on one line"),
                            ("two_lines", "stacked (two lines)"),
                            ("current_only", "current character only"),
                        ],
                    )
                    .hover_tip(
                        "How the forge-materials widget shows your seasonal and regular materials. \
                         The account matching your current character is shown first. \
                         Choosing None hides the widget.",
                    )
                    .changed()
                {
                    match sel.as_deref() {
                        Some("none") => widget_bar.set_enabled(WidgetKind::Materials, false),
                        Some(mode) => {
                            live_feed.materials_display_mode = match mode {
                                "two_lines" => MaterialsDisplayMode::TwoLines,
                                "current_only" => MaterialsDisplayMode::CurrentOnly,
                                _ => MaterialsDisplayMode::OneLine,
                            };
                            // Picking any real layout re-enables the widget.
                            widget_bar.set_enabled(WidgetKind::Materials, true);
                        }
                        None => {}
                    }
                    changed = true;
                }
                });
            },
        );

        if changed {
            if let Ok(mut s) = self.settings.write() {
                s.widget_bar = widget_bar;
                s.live_feed = live_feed.clone();
                s.save();
            }
            self.live_feed_panel.apply_live_feed_settings(&live_feed);
            self.live_feed_panel.apply_season_config(&season);
        }
    }

    /// Human-readable label for a Widget Bar widget kind.
    fn widget_kind_label(kind: realmhound_core::settings::WidgetKind) -> &'static str {
        use realmhound_core::settings::WidgetKind;
        match kind {
            WidgetKind::Dust => "Enchanting dust",
            WidgetKind::ForgeFire => "Forge fire",
            WidgetKind::Materials => "Forge materials",
            WidgetKind::Fame => "Account fame",
            WidgetKind::Gold => "Account gold",
            WidgetKind::Rank => "Account rank and name",
            WidgetKind::CharSlots => "Character slots",
            WidgetKind::Skins => "Skins owned",
            WidgetKind::Playtime => "Total active time",
            WidgetKind::SeasonTimer => "Season timer",
            WidgetKind::BattlepassTimer => "Battlepass timer",
            WidgetKind::CurrentCharacter => "Current character (attributes on hover)",
        }
    }

    /// Render the Party settings panel.
    fn render_party_settings(&mut self, ui: &mut egui::Ui, shadcn: &crate::shadcn_ui::Shadcn) {
        use realmhound_core::settings::PartyViewMode;

        ui.add_space(10.0);
        ui.heading("Party");
        ui.add_space(5.0);
        ui.label(RichText::new("Configure the Party tab's member list.").weak());
        ui.add_space(15.0);

        let mut settings_changed = false;
        let mut current = {
            let s = self.settings.read().unwrap();
            s.party.clone()
        };

        shadcn.card(ui, "party_mod_tools", "Moderation Tools", |ui| {
            if shadcn
                .switch(ui, &mut current.mod_tools, "Moderation tools")
                .hover_tip(
                    "When on, shows Kick and Promote buttons next to each party \
                     member, the party status indicator in the bottom status bar, \
                     and a Ban List button in the party header. Off by default.",
                )
                .changed()
            {
                settings_changed = true;
            }
        });

        ui.add_space(12.0);

        shadcn.card(ui, "party_view_mode", "Member Detail", |ui| {
            shadcn.field_row(ui, |ui| {
                ui.label("View mode:");
                let cur = match current.view_mode {
                    PartyViewMode::Simplified => "simplified",
                    PartyViewMode::Advanced => "advanced",
                };
                let mut sel = Some(cur.to_string());
                if shadcn
                    .select(
                        ui,
                        "party_view_mode",
                        &mut sel,
                        160.0,
                        &[
                            ("simplified", "Simplified view"),
                            ("advanced", "Advanced information"),
                        ],
                    )
                    .hover_tip(
                        "Simplified view hides guild name, Realmeye link, and Realmscope link \
                         for each party member.",
                    )
                    .changed()
                {
                    current.view_mode = match sel.as_deref() {
                        Some("simplified") => PartyViewMode::Simplified,
                        _ => PartyViewMode::Advanced,
                    };
                    settings_changed = true;
                }
            });
        });

        ui.add_space(12.0);

        if settings_changed {
            if let Ok(mut s) = self.settings.write() {
                s.party = current.clone();
                s.save();
            }
            self.party_panel.apply_party_settings(&current);
        }
    }

    /// Render the Trophy Hall settings panel.
    fn render_trophy_hall_settings(
        &mut self,
        ui: &mut egui::Ui,
        shadcn: &crate::shadcn_ui::Shadcn,
    ) {
        use crate::panels::trophy_hall::DataSource;

        ui.add_space(10.0);
        ui.heading("Trophy Hall");
        ui.add_space(5.0);
        ui.label(RichText::new("Configure dungeon collection tracking and view options.").weak());
        ui.add_space(15.0);

        shadcn.card(ui, "trophy_hall_source", "Data Source", |ui| {
            // RealmShark import, autoload, and selection are only available when
            // an account is selected (import path bound) and currently verified.
            let realmshark_gate =
                self.trophy_hall_panel.has_realmshark_path() && self.view_state.account_verified;
            shadcn.field_row(ui, |ui| {
                ui.label("Source:").hover_tip("Controls which tracked-loot sources feed drop counts and the Unlocked status, alongside account data (which always counts).");
                let prev_source = self.trophy_hall_panel.data_source();
                let rs_enabled = self.trophy_hall_panel.realmshark_loaded() && realmshark_gate;
                let options = [
                    ("RealmHound", true),
                    ("RealmShark", rs_enabled),
                    ("Both", rs_enabled),
                ];
                let current = match prev_source {
                    DataSource::RealmHound => 0,
                    DataSource::RealmShark => 1,
                    DataSource::Both => 2,
                };
                if let Some(idx) = on_top_dropdown(ui, "dungeon_data_source", 120.0, current, &options) {
                    let source = match idx {
                        1 => DataSource::RealmShark,
                        2 => DataSource::Both,
                        _ => DataSource::RealmHound,
                    };
                    if source != prev_source {
                        self.trophy_hall_panel.set_data_source(source);
                        self.persist_trophy_hall_source();
                    }
                }
            });

            ui.add_space(8.0);
            let import = ui.add_enabled(realmshark_gate, egui::Button::new("📂 Import RealmShark"));
            if !realmshark_gate {
                import.clone().on_hover_text(
                    "Select and verify an account (launch the game) to import RealmShark stats.",
                );
            }
            if import.clicked() {
                self.trophy_hall_panel.import_realmshark();
                // Import switches the source to Both on success; persist it.
                self.persist_trophy_hall_source();
            }
        });

        ui.add_space(12.0);

        shadcn.card(ui, "trophy_hall_view", "View Options", |ui| {
            let compact_resp = shadcn
                .switch(ui, self.trophy_hall_panel.compact_view_mut(), "Compact view")
                .hover_tip("Hide dungeon name column and collection section names.");
            if compact_resp.changed() {
                self.persist_trophy_hall_view();
            }
            let resp = shadcn
                .switch(
                    ui,
                    self.trophy_hall_panel.show_no_collection_dungeons_mut(),
                    "Show dungeons without collections",
                )
                .hover_tip("Show dungeons whose collection is intentionally empty because their drops fully duplicate another dungeon's collection.");
            if resp.changed() {
                self.persist_trophy_hall_view();
            }
        });
    }

    /// Render the Sound settings panel.
    fn render_sound_settings(&mut self, ui: &mut egui::Ui, shadcn: &crate::shadcn_ui::Shadcn) {
        ui.add_space(10.0);
        ui.heading("Sound");
        ui.add_space(5.0);
        ui.label(RichText::new("Configure notification sounds for loot drops and events.").weak());
        ui.add_space(15.0);

        // Check if audio is available
        if !self.audio.is_available() {
            ui.colored_label(Color32::from_rgb(255, 150, 100), "⚠️ Audio not available");
            ui.label(
                RichText::new("Could not initialize audio output. Check your audio device.").weak(),
            );
            return;
        }

        // Get current settings
        let mut settings_changed = false;
        let (mut current_settings, mut log_event_notifications) = {
            let s = self.settings.read().unwrap();
            (s.sound.clone(), s.live_feed.log_event_notifications)
        };

        // Sub-tab selection is driven by the nested sidebar nav items; render
        // only the active section's content here.
        match self.sound_sub_tab {
            SoundSubTab::General => {
                settings_changed |= self.render_sound_general(ui, shadcn, &mut current_settings);
            }
            SoundSubTab::LootBags => {
                settings_changed |= self.render_sound_loot_bags(ui, shadcn, &mut current_settings);
            }
            SoundSubTab::RealmBosses => {
                settings_changed |= self.render_realm_event_sounds(
                    ui,
                    shadcn,
                    &mut current_settings,
                    &mut log_event_notifications,
                );
            }
            SoundSubTab::Enchantments => {
                settings_changed |=
                    self.render_sound_enchantments(ui, shadcn, &mut current_settings);
            }
        }

        // Save settings if changed
        if settings_changed {
            if let Ok(mut s) = self.settings.write() {
                s.sound = current_settings;
                s.live_feed.log_event_notifications = log_event_notifications;
                s.save();
            }
        }
    }

    /// General sound sub-tab: master volume and dungeon alerts.
    fn render_sound_general(
        &mut self,
        ui: &mut egui::Ui,
        shadcn: &crate::shadcn_ui::Shadcn,
        current_settings: &mut realmhound_core::settings::SoundSettings,
    ) -> bool {
        let mut settings_changed = false;

        // Master volume with slider and test button
        shadcn.card(ui, "snd_master_volume", "Master Volume", |ui| {
            shadcn.field_row(ui, |ui| {
                if shadcn
                    .slider_value(ui, "master_volume", &mut current_settings.volume, 0.0, 1.0)
                    .changed()
                {
                    settings_changed = true;
                    self.audio_tx
                        .send(AudioCommand::SetVolume(current_settings.volume));
                }
                ui.label(format!("{:.0}%", current_settings.volume * 100.0));
                if shadcn.btn(ui, "🔊 Test").clicked() {
                    self.audio_tx.send(AudioCommand::Play(SoundType::WhiteBag));
                }
            });
        });

        ui.add_space(12.0);

        // Social & message notification sounds (RealmShark parity).
        shadcn.card(ui, "snd_social", "Social & Messages", |ui| {
            egui::Grid::new("social_sounds_grid")
                .num_columns(5)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    settings_changed |= shadcn
                        .switch(ui, &mut current_settings.pm, "Direct message")
                        .hover_tip("Plays when you receive a private message (whisper) from another player.")
                        .changed();
                    settings_changed |= self.render_sound_row_controls(ui, shadcn, SoundType::Pm, current_settings);
                    ui.end_row();

                    settings_changed |= shadcn
                        .switch(ui, &mut current_settings.party, "Party message")
                        .hover_tip("Plays when a party member sends a party chat message.")
                        .changed();
                    settings_changed |= self.render_sound_row_controls(ui, shadcn, SoundType::Party, current_settings);
                    ui.end_row();

                    settings_changed |= shadcn
                        .switch(ui, &mut current_settings.guild, "Guild message")
                        .hover_tip("Plays when a guild member sends a guild chat message.")
                        .changed();
                    settings_changed |= self.render_sound_row_controls(ui, shadcn, SoundType::Guild, current_settings);
                    ui.end_row();

                    settings_changed |= shadcn
                        .switch(ui, &mut current_settings.trade, "Trade request")
                        .hover_tip("Plays when another player sends you a trade request.")
                        .changed();
                    settings_changed |= self.render_sound_row_controls(ui, shadcn, SoundType::Trade, current_settings);
                    ui.end_row();
                });
        });

        ui.add_space(12.0);

        // Dungeon alerts section
        let dangerous_mods_tip = format!(
            "Plays a warning sound when entering a dungeon with a dangerous \
             (red-outline) modifier:\n{}",
            realmhound_core::dungeon_modifiers::DANGEROUS_MODIFIER_NAMES.join(", ")
        );
        shadcn.card(ui, "snd_dungeon_alerts", "Dungeon Alerts", |ui| {
            egui::Grid::new("dungeon_alerts_grid")
                .num_columns(5)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    settings_changed |= shadcn
                        .switch(ui, &mut current_settings.dimitus_dungeon, "Dimitus mod")
                        .hover_tip("Plays the Dimitus alert sound when entering a dungeon that has the Dimitus modifier.")
                        .changed();
                    settings_changed |= self.render_sound_row_controls(ui, shadcn, SoundType::DimitusAlert, current_settings);
                    ui.end_row();

                    settings_changed |= shadcn
                        .switch(ui, &mut current_settings.bad_mod_warning, "Dangerous mods")
                        .hover_tip(dangerous_mods_tip.as_str())
                        .changed();
                    settings_changed |= self.render_sound_row_controls(ui, shadcn, SoundType::BadModWarning, current_settings);
                    ui.end_row();
                });
        });

        ui.add_space(12.0);

        // Public key-pop notification: sound toggle plus the difficulty-tier
        // filter that decides which pops notify at all.
        shadcn.card(ui, "snd_key_pops", "Key Pops", |ui| {
            ui.label(
                RichText::new(
                    "The base Key pop sound plays for dungeons with no known difficulty rating. \
                     Each difficulty tier below is a separate on/off toggle with its own volume \
                     and custom sound. The Live Feed has its own separate tier filter.",
                )
                .weak()
                .small(),
            );
            ui.add_space(6.0);
            egui::Grid::new("key_pops_sound_grid")
                .num_columns(5)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    settings_changed |= shadcn
                        .switch(ui, &mut current_settings.keypop, "Key pop sound")
                        .hover_tip("Master toggle for public key-pop sounds. Plays for unknown-difficulty dungeons; the per-tier rows below cover rated dungeons.")
                        .changed();
                    settings_changed |= self.render_sound_row_controls(ui, shadcn, SoundType::KeyPop, current_settings);
                    ui.end_row();

                    settings_changed |= shadcn
                        .switch(ui, &mut current_settings.keypop_tiers.rookie, "Rookie dungeons")
                        .hover_tip("Grave difficulty 2 or lower (e.g. Spider Den, The Hive, Forbidden Jungle).")
                        .changed();
                    settings_changed |= self.render_sound_row_controls(ui, shadcn, SoundType::KeyPopRookie, current_settings);
                    ui.end_row();

                    settings_changed |= shadcn
                        .switch(ui, &mut current_settings.keypop_tiers.adept, "Adept dungeons")
                        .hover_tip("Grave difficulty above 2 up to 4.5 (e.g. Snake Pit, Abyss of Demons, Mad Lab).")
                        .changed();
                    settings_changed |= self.render_sound_row_controls(ui, shadcn, SoundType::KeyPopAdept, current_settings);
                    ui.end_row();

                    settings_changed |= shadcn
                        .switch(ui, &mut current_settings.keypop_tiers.expert, "Expert dungeons")
                        .hover_tip("Grave difficulty above 4.5 up to 6.5 (e.g. Lair of Draconis, Secluded Thicket, Ocean Trench).")
                        .changed();
                    settings_changed |= self.render_sound_row_controls(ui, shadcn, SoundType::KeyPopExpert, current_settings);
                    ui.end_row();

                    settings_changed |= shadcn
                        .switch(ui, &mut current_settings.keypop_tiers.exaltation, "Exaltation dungeons")
                        .hover_tip("Grave difficulty above 6.5 (e.g. The Nest, Lost Halls, The Shatters).")
                        .changed();
                    settings_changed |= self.render_sound_row_controls(ui, shadcn, SoundType::KeyPopExaltation, current_settings);
                    ui.end_row();
                });
        });

        settings_changed
    }

    /// Loot bags sound sub-tab: per-bag loot notification sounds.
    fn render_sound_loot_bags(
        &mut self,
        ui: &mut egui::Ui,
        shadcn: &crate::shadcn_ui::Shadcn,
        current_settings: &mut realmhound_core::settings::SoundSettings,
    ) -> bool {
        let mut settings_changed = false;
        shadcn.card(ui, "snd_loot_sounds", "Loot Sounds", |ui| {
            egui::Grid::new("loot_sounds_grid")
                .num_columns(5)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    settings_changed |= shadcn
                        .switch(ui, &mut current_settings.whitebag, "White bag")
                        .changed();
                    settings_changed |= self.render_sound_row_controls(
                        ui,
                        shadcn,
                        SoundType::WhiteBag,
                        current_settings,
                    );
                    ui.end_row();

                    settings_changed |= shadcn
                        .switch(ui, &mut current_settings.redbag, "Red bag")
                        .changed();
                    settings_changed |= self.render_sound_row_controls(
                        ui,
                        shadcn,
                        SoundType::RedBag,
                        current_settings,
                    );
                    ui.end_row();

                    settings_changed |= shadcn
                        .switch(ui, &mut current_settings.orangebag, "Orange bag")
                        .changed();
                    settings_changed |= self.render_sound_row_controls(
                        ui,
                        shadcn,
                        SoundType::OrangeBag,
                        current_settings,
                    );
                    ui.end_row();

                    settings_changed |= shadcn
                        .switch(ui, &mut current_settings.bluebag, "Blue bag")
                        .changed();
                    settings_changed |= self.render_sound_row_controls(
                        ui,
                        shadcn,
                        SoundType::BlueBag,
                        current_settings,
                    );
                    ui.end_row();

                    settings_changed |= shadcn
                        .switch(ui, &mut current_settings.goldbag, "Gold bag")
                        .changed();
                    settings_changed |= self.render_sound_row_controls(
                        ui,
                        shadcn,
                        SoundType::GoldBag,
                        current_settings,
                    );
                    ui.end_row();

                    settings_changed |= shadcn
                        .switch(ui, &mut current_settings.eggbag, "Egg bag")
                        .changed();
                    settings_changed |= self.render_sound_row_controls(
                        ui,
                        shadcn,
                        SoundType::EggBag,
                        current_settings,
                    );
                    ui.end_row();
                });
        });

        settings_changed
    }

    /// Enchantments sound sub-tab: play a notification when a matching enchant
    /// appears in a loot bag. Two categories (tiered and unique/awakened),
    /// each off by default with its own sound and per-entry rows. Returns `true`
    /// if any setting changed.
    fn render_sound_enchantments(
        &mut self,
        ui: &mut egui::Ui,
        shadcn: &crate::shadcn_ui::Shadcn,
        current: &mut realmhound_core::settings::SoundSettings,
    ) -> bool {
        use crate::enchant_sound::EnchantCategory;

        ui.label(
            RichText::new(
                "Play a sound when a matching enchantment appears in a loot bag. Enable a \
                 category, then add the enchantments you want to hear. Each enchant's own \
                 slider sets how loud it plays; the category default only sets the starting \
                 volume for newly added enchants. All volumes are scaled by Master Volume.",
            )
            .weak(),
        );
        ui.add_space(8.0);

        let catalog = realmhound_core::assets::get_asset_manager().enchant_catalog();
        let label_w = Self::enchant_label_col_width(ui, &catalog);
        let mut changed = false;
        changed |= self.render_enchant_section_card(
            ui,
            shadcn,
            current,
            EnchantCategory::Tiered,
            "Tiered enchantments",
            &catalog,
            label_w,
        );
        ui.add_space(12.0);
        changed |= self.render_enchant_section_card(
            ui,
            shadcn,
            current,
            EnchantCategory::Special,
            "Unique and awakened enchantments",
            &catalog,
            label_w,
        );
        changed
    }

    /// One enchantment notification category card (tiered or unique/awakened).
    fn render_enchant_section_card(
        &mut self,
        ui: &mut egui::Ui,
        shadcn: &crate::shadcn_ui::Shadcn,
        current: &mut realmhound_core::settings::SoundSettings,
        cat: crate::enchant_sound::EnchantCategory,
        title: &str,
        catalog: &[realmhound_core::assets::EnchantCatalogEntry],
        label_w: f32,
    ) -> bool {
        use crate::enchant_sound::{override_key, EnchantCategory, ENCHANT_SOUND};

        let prefix = cat.key_prefix();
        let is_tiered = cat == EnchantCategory::Tiered;
        let mut changed = false;
        let card_id = format!("snd_enchant_{}", prefix);

        shadcn.card(ui, &card_id, title, |ui| {
            let section = match cat {
                EnchantCategory::Tiered => &mut current.enchantments.tiered,
                EnchantCategory::Special => &mut current.enchantments.special,
            };

            // Migrate legacy special entries whose key was stored as an internal
            // id (e.g. "FLURRY_OF_BLOWS") to the display name used by the picker,
            // then drop any resulting duplicates.
            if !is_tiered {
                for entry in section.entries.iter_mut() {
                    if let Some(hit) = catalog
                        .iter()
                        .find(|e| e.tier_class.is_special() && e.internal_id == entry.key)
                    {
                        entry.key = hit.display_name.clone();
                        entry.name = hit.display_name.clone();
                        if entry.icon_id == 0 {
                            entry.icon_id = hit.type_id;
                        }
                        changed = true;
                    }
                }
                let mut seen = std::collections::HashSet::new();
                let before = section.entries.len();
                section.entries.retain(|e| seen.insert(e.key.clone()));
                if section.entries.len() != before {
                    changed = true;
                }
            }

            changed |= shadcn
                .switch(ui, &mut section.enabled, "Enable notifications")
                .changed();

            if !section.enabled {
                return;
            }

            ui.add_space(6.0);

            // Category default volume + test.
            shadcn.field_row(ui, |ui| {
                ui.set_min_height(event_row::ROW_H);
                fixed_cell(ui, label_w, event_row::ROW_H, |ui| {
                    ui.label("Default volume:").on_hover_text(
                        "Starting volume for enchants you add next. Does not change \
                             enchants already in the list -- adjust those with their own sliders.",
                    );
                });
                if is_tiered {
                    fixed_cell(ui, event_row::TIER_W, event_row::ROW_H, |_ui| {});
                }
                if shadcn
                    .slider_value(
                        ui,
                        &format!("{}_def_vol", prefix),
                        &mut section.default_volume,
                        0.0,
                        1.0,
                    )
                    .on_hover_cursor(egui::CursorIcon::Default)
                    .changed()
                {
                    changed = true;
                }
                fixed_cell(ui, event_row::PCT_W, event_row::ROW_H, |ui| {
                    ui.label(format!("{:.0}%", section.default_volume * 100.0));
                });
                fixed_cell(ui, event_row::PICKER_W, event_row::ROW_H, |_ui| {});
                if ui
                    .small_button("🔊 Test")
                    .hover_tip("Test default sound")
                    .clicked()
                {
                    self.audio_tx.send(AudioCommand::PlayEvent {
                        sound: ENCHANT_SOUND,
                        custom_key: format!("enchant:{}:default", prefix),
                        volume: section.default_volume,
                    });
                }
            });

            ui.add_space(8.0);

            let hint = if is_tiered {
                "Add an enchantment, then pick the minimum tier (I-IV) that triggers the sound:"
            } else {
                "Add the unique or awakened enchantments you want to hear:"
            };
            ui.label(RichText::new(hint).weak().small());

            // Type-ahead enchant picker.
            changed |= self.render_enchant_picker(ui, shadcn, cat, prefix, section, catalog);

            // Entry rows.
            if !section.entries.is_empty() {
                ui.add_space(6.0);
                let mut remove: Option<usize> = None;
                for (idx, entry) in section.entries.iter_mut().enumerate() {
                    let key = override_key(cat, &entry.key);
                    let has_custom = current.custom_sounds.contains_key(&key);
                    shadcn.field_row(ui, |ui| {
                        ui.set_min_height(event_row::ROW_H);
                        fixed_cell(ui, label_w, event_row::ROW_H, |ui| {
                            let (r, _) = ui.allocate_exact_size(
                                egui::vec2(event_row::ICON, event_row::ICON),
                                egui::Sense::hover(),
                            );
                            let icon_id = if entry.icon_id != 0 {
                                entry.icon_id
                            } else if is_tiered {
                                crate::enchant_sound::family_tier_icon(
                                    catalog,
                                    &entry.key,
                                    entry.tier.unwrap_or(1),
                                )
                            } else {
                                catalog
                                    .iter()
                                    .find(|e| {
                                        e.display_name == entry.key || e.internal_id == entry.key
                                    })
                                    .map(|e| e.type_id)
                                    .unwrap_or(0)
                            };
                            if icon_id != 0 {
                                self.sprite_renderer
                                    .draw_enchant_sprite_in_rect(ui, icon_id, r);
                            }
                            ui.add_space(6.0);
                            ui.label(RichText::new(&entry.name).strong());
                        });
                        // Tier selector (tiered category only). A custom
                        // Order::Tooltip dropdown so the popup shows above the
                        // settings dialog and consumes its own click (a raw
                        // ComboBox/shadcn select popup falls through to the row
                        // behind it).
                        if is_tiered {
                            let cur = entry.tier.unwrap_or(1).clamp(1, 4);
                            let picked =
                                fixed_cell(ui, event_row::TIER_W, event_row::ROW_H, |ui| {
                                    tier_dropdown(
                                        ui,
                                        &format!("{}_tier_{}", prefix, entry.key),
                                        cur,
                                    )
                                });
                            if let Some(tier) = picked {
                                if entry.tier != Some(tier) {
                                    entry.tier = Some(tier);
                                    entry.icon_id = crate::enchant_sound::family_tier_icon(
                                        catalog, &entry.key, tier,
                                    );
                                    changed = true;
                                }
                            }
                        }
                        if shadcn
                            .slider_value(
                                ui,
                                &format!("{}_ent_{}", prefix, entry.key),
                                &mut entry.volume,
                                0.0,
                                1.0,
                            )
                            .on_hover_cursor(egui::CursorIcon::Default)
                            .changed()
                        {
                            changed = true;
                        }
                        fixed_cell(ui, event_row::PCT_W, event_row::ROW_H, |ui| {
                            ui.label(format!("{:.0}%", entry.volume * 100.0));
                        });
                        fixed_cell(ui, event_row::PICKER_W, event_row::ROW_H, |ui| {
                            if !has_custom {
                                changed |= self.render_custom_sound_picker(
                                    ui,
                                    &key,
                                    &mut current.custom_sounds,
                                    &mut current.custom_sound_names,
                                );
                            }
                        });
                        if ui.small_button("🔊 Test").hover_tip("Test").clicked() {
                            self.audio_tx.send(AudioCommand::PlayEvent {
                                sound: ENCHANT_SOUND,
                                custom_key: key.clone(),
                                volume: entry.volume,
                            });
                        }
                        if ui.small_button("🗑").hover_tip("Remove").clicked() {
                            remove = Some(idx);
                        }
                    });
                    if has_custom {
                        ui.horizontal(|ui| {
                            ui.set_min_height(22.0);
                            ui.add_space(event_row::ICON + 6.0);
                            changed |= self.render_custom_sound_reset(
                                ui,
                                &key,
                                &mut current.custom_sounds,
                                &mut current.custom_sound_names,
                            );
                        });
                    }
                }
                if let Some(idx) = remove {
                    let entry = section.entries.remove(idx);
                    let key = override_key(cat, &entry.key);
                    if let Some(filename) = current.custom_sounds.remove(&key) {
                        if let Some(dir) = custom_sounds_dir() {
                            let _ = fs::remove_file(dir.join(filename));
                        }
                    }
                    current.custom_sound_names.remove(&key);
                    changed = true;
                }
            }
        });

        changed
    }

    /// Type-ahead enchant picker for an enchantment notification category, modeled
    /// on the realm-event boss picker. Adds a new entry when a candidate is chosen.
    fn render_enchant_picker(
        &mut self,
        ui: &mut egui::Ui,
        shadcn: &crate::shadcn_ui::Shadcn,
        cat: crate::enchant_sound::EnchantCategory,
        prefix: &'static str,
        section: &mut realmhound_core::settings::EnchantSection,
        catalog: &[realmhound_core::assets::EnchantCatalogEntry],
    ) -> bool {
        use crate::enchant_sound::{special_candidates, tiered_candidates, EnchantCategory};

        let mut changed = false;
        let picker_id: &'static str = if cat == EnchantCategory::Tiered {
            "enchant_tiered"
        } else {
            "enchant_special"
        };

        let already: std::collections::HashSet<String> =
            section.entries.iter().map(|e| e.key.clone()).collect();

        // Build candidates as (key, name, icon_id) regardless of category.
        let candidates: Vec<(String, String, u16)> = if cat == EnchantCategory::Tiered {
            tiered_candidates(catalog, &already)
                .into_iter()
                .map(|c| (c.family, c.name, c.icon_id))
                .collect()
        } else {
            special_candidates(catalog, &already)
                .into_iter()
                .map(|c| (c.key, c.name, c.icon_id))
                .collect()
        };

        let state = self.event_pickers.entry(picker_id).or_default();
        if state.text != state.prev_text {
            state.selected = None;
            state.prev_text = state.text.clone();
        }

        let response = ui.horizontal(|ui| {
            ui.label("Add enchant:");
            let r = ui.add(
                egui::TextEdit::singleline(&mut state.text)
                    .hint_text("Type or select an enchant name…")
                    .desired_width(220.0),
            );
            if !state.text.is_empty() && shadcn.btn(ui, "\u{2715}").clicked() {
                state.text.clear();
                state.popup_open = false;
                state.selected = None;
            }
            r
        });
        let response = response.inner;

        let enter_pressed = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if response.has_focus() {
            state.popup_open = true;
        }

        let query = state.text.trim().to_lowercase();
        let results: Vec<(String, String, u16)> = candidates
            .into_iter()
            .filter(|(_, name, _)| query.is_empty() || name.to_lowercase().contains(&query))
            .collect();
        let total = results.len();

        let mut chosen: Option<(String, String, u16)> = None;
        if enter_pressed && state.popup_open && total > 0 {
            let idx = state.selected.unwrap_or(0).min(total - 1);
            chosen = results.get(idx).cloned();
        }

        if state.popup_open && response.has_focus() && total > 0 {
            let key_down = ui.input(|i| i.key_pressed(egui::Key::ArrowDown));
            let key_up = ui.input(|i| i.key_pressed(egui::Key::ArrowUp));
            let key_escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
            if key_escape {
                state.popup_open = false;
                state.selected = None;
            } else if key_down {
                state.selected = Some(match state.selected {
                    None => 0,
                    Some(i) => (i + 1).min(total - 1),
                });
            } else if key_up {
                state.selected = match state.selected {
                    None => None,
                    Some(0) => None,
                    Some(i) => Some(i - 1),
                };
            }
        }

        if state.popup_open && total > 0 {
            let selected = state.selected;
            let popup_id = ui.make_persistent_id(format!("enchant_popup_{}", prefix));
            let popup_pos = response.rect.left_bottom() + egui::vec2(0.0, 2.0);
            let area = egui::Area::new(popup_id)
                .fixed_pos(popup_pos)
                .order(egui::Order::Tooltip)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_min_width(240.0);
                        ui.set_max_height(320.0);
                        egui::ScrollArea::vertical()
                            .max_height(300.0)
                            .show(ui, |ui| {
                                for (row, (key, name, icon_id)) in results.iter().enumerate() {
                                    let is_selected = selected == Some(row);
                                    ui.horizontal(|ui| {
                                        let (rect, _) = ui.allocate_exact_size(
                                            egui::vec2(20.0, 20.0),
                                            egui::Sense::hover(),
                                        );
                                        if *icon_id != 0 {
                                            self.sprite_renderer
                                                .draw_enchant_sprite_in_rect(ui, *icon_id, rect);
                                        }
                                        let resp =
                                            ui.add(egui::Button::new(name).selected(is_selected));
                                        if is_selected {
                                            resp.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        if resp.clicked() {
                                            chosen = Some((key.clone(), name.clone(), *icon_id));
                                        }
                                    });
                                }
                            });
                    });
                });
            ui.ctx().move_to_top(area.response.layer_id);
            if ui.input(|i| i.pointer.any_click()) && !response.has_focus() && chosen.is_none() {
                let state = self.event_pickers.entry(picker_id).or_default();
                state.popup_open = false;
                state.selected = None;
            }
        }

        if let Some((key, name, icon_id)) = chosen {
            let tier = if cat == EnchantCategory::Tiered {
                Some(1)
            } else {
                None
            };
            section
                .entries
                .push(realmhound_core::settings::EnchantEntry {
                    key,
                    name,
                    tier,
                    volume: section.default_volume,
                    icon_id,
                });
            let state = self.event_pickers.entry(picker_id).or_default();
            state.text.clear();
            state.prev_text.clear();
            state.popup_open = false;
            state.selected = None;
            changed = true;
        }

        changed
    }

    /// Render the "Realm event notifications" settings section.
    /// Returns `true` if any setting changed.
    fn render_realm_event_sounds(
        &mut self,
        ui: &mut egui::Ui,
        shadcn: &crate::shadcn_ui::Shadcn,
        current: &mut realmhound_core::settings::SoundSettings,
        log_event_notifications: &mut bool,
    ) -> bool {
        use crate::realm_event_sound::SectionKind;

        ui.label(
            RichText::new(
                "Play a sound when an event boss appears in the Live Feed. Enable a section to \
                 notify for its whole category, then add individual bosses to override their \
                 sound or volume. Volumes are scaled by Master Volume.",
            )
            .weak(),
        );
        ui.add_space(8.0);

        let mut changed = false;
        let label_w = Self::event_label_col_width(ui);
        for (kind, title) in [
            (SectionKind::Veteran, "Veteran Encounters"),
            (SectionKind::Adept, "Adept Encounters"),
            (SectionKind::Seasonal, "Seasonal and special bosses"),
        ] {
            changed |= self.render_event_section_card(ui, shadcn, current, kind, title, label_w);
            ui.add_space(12.0);
        }
        changed |= self.render_alien_section_card(ui, shadcn, current, label_w);
        ui.add_space(12.0);
        shadcn.card(ui, "realm_event_history", "Diagnostic History", |ui| {
            if shadcn
                .switch(
                    ui,
                    log_event_notifications,
                    "Write event notification history",
                )
                .hover_tip("Store boss-call diagnostics for the selected account. Off by default.")
                .changed()
            {
                changed = true;
            }
        });
        changed
    }

    /// Measure the fixed width for the event-label column: the widest encounter
    /// name across every category (plus the Alien rows and "Default volume:"),
    /// so sliders and buttons align in one column. Clamped to a sane maximum.
    fn event_label_col_width(ui: &egui::Ui) -> f32 {
        use crate::realm_event_sound::{section_catalog, SectionKind};
        let font = egui::TextStyle::Body.resolve(ui.style());
        let measure = |text: &str| -> f32 {
            ui.painter()
                .layout_no_wrap(text.to_string(), font.clone(), egui::Color32::WHITE)
                .size()
                .x
        };
        let mut max_w = 0.0_f32;
        for kind in [
            SectionKind::Veteran,
            SectionKind::Adept,
            SectionKind::Seasonal,
        ] {
            for entry in section_catalog(kind) {
                max_w = max_w.max(measure(&entry.name));
            }
        }
        for label in [
            "Alien wave start",
            "Commander Calbrik",
            "Alien UFO",
            "Default volume:",
        ] {
            max_w = max_w.max(measure(label));
        }
        // Icon box + spacing + bold-render slack + right padding.
        let total = event_row::ICON + 6.0 + max_w * 1.06 + 12.0;
        total.clamp(120.0, event_row::LABEL_MAX)
    }

    /// Measure the fixed width for the enchantment-label column: the widest
    /// enchant row label across both categories (tiered family names and
    /// unique/awakened display names, matching what the pickers show) plus the
    /// "Default volume:" label, so every tier selector, slider, percent and
    /// button aligns in one column. Clamped to a sane maximum.
    fn enchant_label_col_width(
        ui: &egui::Ui,
        catalog: &[realmhound_core::assets::EnchantCatalogEntry],
    ) -> f32 {
        use crate::enchant_sound::{special_candidates, tiered_candidates};
        let empty = std::collections::HashSet::new();
        let font = egui::TextStyle::Body.resolve(ui.style());
        let measure = |text: &str| -> f32 {
            ui.painter()
                .layout_no_wrap(text.to_string(), font.clone(), egui::Color32::WHITE)
                .size()
                .x
        };
        let mut max_w = measure("Default volume:");
        for c in tiered_candidates(catalog, &empty) {
            max_w = max_w.max(measure(&c.name));
        }
        for c in special_candidates(catalog, &empty) {
            max_w = max_w.max(measure(&c.name));
        }
        // Icon box + spacing + bold-render slack + right padding.
        let total = event_row::ICON + 6.0 + max_w * 1.06 + 12.0;
        total.clamp(160.0, 360.0)
    }

    /// One catalog-backed realm-event section card (Veteran/Adept/Seasonal).
    /// Type-ahead boss picker for a realm-event section, modeled on the
    /// Treasury dungeon-category selector: a search field with a
    /// floating suggestion list (icon + name), keyboard navigation, and
    /// click/Enter to add an override. Returns true when an override was added.
    fn render_event_boss_picker(
        &mut self,
        ui: &mut egui::Ui,
        shadcn: &crate::shadcn_ui::Shadcn,
        kind: crate::realm_event_sound::SectionKind,
        prefix: &'static str,
        default_volume: f32,
        overrides: &mut Vec<realmhound_core::settings::EventOverride>,
    ) -> bool {
        use crate::realm_event_sound::section_catalog;

        let mut changed = false;
        let catalog = section_catalog(kind);
        let already: std::collections::HashSet<i32> = overrides.iter().map(|o| o.boss_id).collect();

        let state = self.event_pickers.entry(prefix).or_default();

        // Reset keyboard selection when the query text changes.
        if state.text != state.prev_text {
            state.selected = None;
            state.prev_text = state.text.clone();
        }

        let response = ui.horizontal(|ui| {
            ui.label("Add boss:");
            let r = ui.add(
                egui::TextEdit::singleline(&mut state.text)
                    .hint_text("Type or select a boss name…")
                    .desired_width(220.0),
            );
            if !state.text.is_empty() && shadcn.btn(ui, "\u{2715}").clicked() {
                state.text.clear();
                state.popup_open = false;
                state.selected = None;
            }
            r
        });
        let response = response.inner;

        let enter_pressed = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        // Open the list whenever the field is focused, so clicking it (even
        // empty) expands the full category list.
        if response.has_focus() {
            state.popup_open = true;
        }

        // Build suggestions: (boss_id, name) not already added. An empty query
        // lists the whole category; otherwise filter by substring match.
        let query = state.text.trim().to_lowercase();
        let results: Vec<(i32, String)> = catalog
            .iter()
            .filter_map(|entry| entry.sprite_ids.last().copied().map(|id| (id, entry)))
            .filter(|(id, entry)| {
                !already.contains(id)
                    && (query.is_empty() || entry.name.to_lowercase().contains(&query))
            })
            .map(|(id, entry)| (id, entry.name.clone()))
            .collect();
        let total = results.len();

        let mut chosen: Option<(i32, String)> = None;

        // Enter applies the selected (or first) suggestion.
        if enter_pressed && state.popup_open && total > 0 {
            let idx = state.selected.unwrap_or(0).min(total - 1);
            chosen = results.get(idx).cloned();
        }

        // Keyboard navigation.
        if state.popup_open && response.has_focus() && total > 0 {
            let key_down = ui.input(|i| i.key_pressed(egui::Key::ArrowDown));
            let key_up = ui.input(|i| i.key_pressed(egui::Key::ArrowUp));
            let key_escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
            if key_escape {
                state.popup_open = false;
                state.selected = None;
            } else if key_down {
                state.selected = Some(match state.selected {
                    None => 0,
                    Some(i) => (i + 1).min(total - 1),
                });
            } else if key_up {
                state.selected = match state.selected {
                    None => None,
                    Some(0) => None,
                    Some(i) => Some(i - 1),
                };
            }
        }

        // Floating suggestion popup.
        if state.popup_open && total > 0 {
            let selected = state.selected;
            let popup_id = ui.make_persistent_id(format!("event_boss_popup_{}", prefix));
            let popup_pos = response.rect.left_bottom() + egui::vec2(0.0, 2.0);
            let area = egui::Area::new(popup_id)
                .fixed_pos(popup_pos)
                .order(egui::Order::Tooltip)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_min_width(240.0);
                        ui.set_max_height(320.0);
                        egui::ScrollArea::vertical()
                            .max_height(300.0)
                            .show(ui, |ui| {
                                for (row, (id, name)) in results.iter().enumerate() {
                                    let is_selected = selected == Some(row);
                                    ui.horizontal(|ui| {
                                        let (rect, _) = ui.allocate_exact_size(
                                            egui::vec2(20.0, 20.0),
                                            egui::Sense::hover(),
                                        );
                                        self.sprite_renderer.draw_sprite_in_rect(ui, *id, rect);
                                        let resp =
                                            ui.add(egui::Button::new(name).selected(is_selected));
                                        if is_selected {
                                            resp.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        if resp.clicked() {
                                            chosen = Some((*id, name.clone()));
                                        }
                                    });
                                }
                            });
                    });
                });
            // The settings dialog also lives on the Tooltip layer, so force the
            // popup to the top of that layer or it renders behind the modal.
            ui.ctx().move_to_top(area.response.layer_id);

            // Close the popup on any outside click.
            if ui.input(|i| i.pointer.any_click()) && !response.has_focus() && chosen.is_none() {
                let state = self.event_pickers.entry(prefix).or_default();
                state.popup_open = false;
                state.selected = None;
            }
        }

        if let Some((id, name)) = chosen {
            overrides.push(realmhound_core::settings::EventOverride {
                boss_id: id,
                name,
                volume: default_volume,
            });
            let state = self.event_pickers.entry(prefix).or_default();
            state.text.clear();
            state.prev_text.clear();
            state.popup_open = false;
            state.selected = None;
            changed = true;
        }

        changed
    }

    fn render_event_section_card(
        &mut self,
        ui: &mut egui::Ui,
        shadcn: &crate::shadcn_ui::Shadcn,
        current: &mut realmhound_core::settings::SoundSettings,
        kind: crate::realm_event_sound::SectionKind,
        title: &str,
        label_w: f32,
    ) -> bool {
        use crate::realm_event_sound::{override_key, SectionKind};
        use realmhound_core::settings::EventMode;

        let prefix = kind.key_prefix();
        let default_sound = kind.default_sound();
        let mut changed = false;
        let card_id = format!("snd_event_{}", prefix);

        shadcn.card(ui, &card_id, title, |ui| {
            let section = match kind {
                SectionKind::Veteran => &mut current.realm_events.veteran,
                SectionKind::Adept => &mut current.realm_events.adept,
                SectionKind::Seasonal => &mut current.realm_events.seasonal,
            };

            // Mode selector: None / All / Selected.
            ui.horizontal(|ui| {
                ui.label("Notifications:");
                for (m, label, tip) in [
                    (EventMode::None, "None", "No sounds for this category."),
                    (
                        EventMode::All,
                        "All",
                        "Every encounter in this category plays the default sound. Add bosses \
                         below to give specific ones a custom sound or volume.",
                    ),
                    (
                        EventMode::Selected,
                        "Selected",
                        "Only the encounters you add below play. Each uses the default sound \
                         unless you upload a custom one.",
                    ),
                ] {
                    if ui
                        .selectable_value(&mut section.mode, m, label)
                        .on_hover_text(tip)
                        .changed()
                    {
                        changed = true;
                    }
                }
            });

            if section.mode == EventMode::None {
                return;
            }

            ui.add_space(6.0);

            // Section default volume + test.
            shadcn.field_row(ui, |ui| {
                ui.set_min_height(event_row::ROW_H);
                fixed_cell(ui, label_w, event_row::ROW_H, |ui| {
                    ui.label("Default volume:").on_hover_text(
                        "Starting volume for events you add next. Does not change events \
                             already in the list -- adjust those with their own sliders.",
                    );
                });
                if shadcn
                    .slider_value(
                        ui,
                        &format!("{}_def_vol", prefix),
                        &mut section.default_volume,
                        0.0,
                        1.0,
                    )
                    .on_hover_cursor(egui::CursorIcon::Default)
                    .changed()
                {
                    changed = true;
                }
                fixed_cell(ui, event_row::PCT_W, event_row::ROW_H, |ui| {
                    ui.label(format!("{:.0}%", section.default_volume * 100.0));
                });
                fixed_cell(ui, event_row::PICKER_W, event_row::ROW_H, |_ui| {});
                if ui
                    .small_button("🔊 Test")
                    .hover_tip("Test default sound")
                    .clicked()
                {
                    self.audio_tx.send(AudioCommand::PlayEvent {
                        sound: default_sound,
                        custom_key: format!("event:{}:default", prefix),
                        volume: section.default_volume,
                    });
                }
            });

            ui.add_space(8.0);

            // Context hint + boss picker. In Selected mode the list is the
            // whitelist; in All mode it customizes individual bosses.
            let hint = match section.mode {
                EventMode::Selected => {
                    "Choose the encounters you want to hear notification sounds for:"
                }
                _ => "Customize a specific encounter's sound or volume:",
            };
            ui.label(RichText::new(hint).weak().small());

            // Boss picker: type-ahead search with a floating suggestion list
            // (reuses the Treasury dungeon-category picker pattern).
            changed |= self.render_event_boss_picker(
                ui,
                shadcn,
                kind,
                prefix,
                section.default_volume,
                &mut section.overrides,
            );

            // Override rows.
            if !section.overrides.is_empty() {
                ui.add_space(6.0);
                let mut remove: Option<usize> = None;
                for (idx, ov) in section.overrides.iter_mut().enumerate() {
                    let key = override_key(kind, ov.boss_id);
                    let has_custom = current.custom_sounds.contains_key(&key);
                    // Main row: icon + name + slider + % + picker + Test + Delete,
                    // always a single line so a custom sound never shifts it.
                    shadcn.field_row(ui, |ui| {
                        ui.set_min_height(event_row::ROW_H);
                        fixed_cell(ui, label_w, event_row::ROW_H, |ui| {
                            let (r, _) = ui.allocate_exact_size(
                                egui::vec2(event_row::ICON, event_row::ICON),
                                egui::Sense::hover(),
                            );
                            self.sprite_renderer.draw_sprite_in_rect(ui, ov.boss_id, r);
                            ui.add_space(6.0);
                            ui.label(RichText::new(&ov.name).strong());
                        });
                        if shadcn
                            .slider_value(
                                ui,
                                &format!("{}_ov_{}", prefix, ov.boss_id),
                                &mut ov.volume,
                                0.0,
                                1.0,
                            )
                            .on_hover_cursor(egui::CursorIcon::Default)
                            .changed()
                        {
                            changed = true;
                        }
                        fixed_cell(ui, event_row::PCT_W, event_row::ROW_H, |ui| {
                            ui.label(format!("{:.0}%", ov.volume * 100.0));
                        });
                        fixed_cell(ui, event_row::PICKER_W, event_row::ROW_H, |ui| {
                            if !has_custom {
                                changed |= self.render_custom_sound_picker(
                                    ui,
                                    &key,
                                    &mut current.custom_sounds,
                                    &mut current.custom_sound_names,
                                );
                            }
                        });
                        if ui.small_button("🔊 Test").hover_tip("Test").clicked() {
                            self.audio_tx.send(AudioCommand::PlayEvent {
                                sound: default_sound,
                                custom_key: key.clone(),
                                volume: ov.volume,
                            });
                        }
                        if ui.small_button("🗑").hover_tip("Remove").clicked() {
                            remove = Some(idx);
                        }
                    });
                    // Second row (only with a custom sound): reset button +
                    // filename, tucked directly under the boss name.
                    if has_custom {
                        ui.horizontal(|ui| {
                            ui.set_min_height(22.0);
                            ui.add_space(event_row::ICON + 6.0);
                            changed |= self.render_custom_sound_reset(
                                ui,
                                &key,
                                &mut current.custom_sounds,
                                &mut current.custom_sound_names,
                            );
                        });
                    }
                }
                if let Some(idx) = remove {
                    let ov = section.overrides.remove(idx);
                    // Drop any custom sound file bound to this override.
                    let key = override_key(kind, ov.boss_id);
                    if let Some(filename) = current.custom_sounds.remove(&key) {
                        if let Some(dir) = custom_sounds_dir() {
                            let _ = fs::remove_file(dir.join(filename));
                        }
                    }
                    current.custom_sound_names.remove(&key);
                    changed = true;
                }
            }
        });

        changed
    }

    /// The Alien Invasion realm-event card: three fixed rows (wave / UFO /
    /// Calbrik), no boss picker.
    fn render_alien_section_card(
        &mut self,
        ui: &mut egui::Ui,
        shadcn: &crate::shadcn_ui::Shadcn,
        current: &mut realmhound_core::settings::SoundSettings,
        label_w: f32,
    ) -> bool {
        use crate::realm_event_sound::{ALIEN_CALBRIK_KEY, ALIEN_UFO_KEY, ALIEN_WAVE_KEY};
        use crate::sound::EventSound;

        let mut changed = false;
        shadcn.card(ui, "snd_event_alien", "Alien Invasion", |ui| {
            changed |= shadcn
                .switch(
                    ui,
                    &mut current.realm_events.alien.enabled,
                    "Enable notifications",
                )
                .hover_tip("Plays sounds for the Alien Invasion event.")
                .changed();
            ui.add_space(6.0);

            let (ufo_icon, calbrik_icon) = crate::realm_event_sound::alien_icon_ids();
            let rows: [(&str, Option<i32>, EventSound, &str, &mut f32); 3] = [
                (
                    "Alien wave start",
                    None,
                    EventSound::AlienWave,
                    ALIEN_WAVE_KEY,
                    &mut current.realm_events.alien.wave.volume,
                ),
                (
                    "Alien UFO",
                    ufo_icon,
                    EventSound::Ufo,
                    ALIEN_UFO_KEY,
                    &mut current.realm_events.alien.ufo.volume,
                ),
                (
                    "Commander Calbrik",
                    calbrik_icon,
                    EventSound::Calbrik,
                    ALIEN_CALBRIK_KEY,
                    &mut current.realm_events.alien.calbrik.volume,
                ),
            ];

            for (label, icon_id, sound, key, volume) in rows {
                let has_custom = current.custom_sounds.contains_key(key);
                shadcn.field_row(ui, |ui| {
                    ui.set_min_height(event_row::ROW_H);
                    fixed_cell(ui, label_w, event_row::ROW_H, |ui| {
                        let (r, _) = ui.allocate_exact_size(
                            egui::vec2(event_row::ICON, event_row::ICON),
                            egui::Sense::hover(),
                        );
                        if let Some(id) = icon_id {
                            self.sprite_renderer.draw_sprite_in_rect(ui, id, r);
                        }
                        ui.add_space(6.0);
                        ui.label(RichText::new(label).strong());
                    });
                    if shadcn
                        .slider_value(ui, &format!("alien_vol_{}", key), volume, 0.0, 1.0)
                        .on_hover_cursor(egui::CursorIcon::Default)
                        .changed()
                    {
                        changed = true;
                    }
                    fixed_cell(ui, event_row::PCT_W, event_row::ROW_H, |ui| {
                        ui.label(format!("{:.0}%", *volume * 100.0));
                    });
                    fixed_cell(ui, event_row::PICKER_W, event_row::ROW_H, |ui| {
                        if !has_custom {
                            changed |= self.render_custom_sound_picker(
                                ui,
                                key,
                                &mut current.custom_sounds,
                                &mut current.custom_sound_names,
                            );
                        }
                    });
                    if ui.small_button("🔊 Test").hover_tip("Test").clicked() {
                        self.audio_tx.send(AudioCommand::PlayEvent {
                            sound,
                            custom_key: key.to_string(),
                            volume: *volume,
                        });
                    }
                });
                if has_custom {
                    ui.horizontal(|ui| {
                        ui.set_min_height(22.0);
                        ui.add_space(event_row::ICON + 6.0);
                        changed |= self.render_custom_sound_reset(
                            ui,
                            key,
                            &mut current.custom_sounds,
                            &mut current.custom_sound_names,
                        );
                    });
                }
            }
        });

        changed
    }

    /// Play a realm-event notification sound for a Live Feed boss call, if the
    /// matching section is enabled (see [`crate::realm_event_sound`]).
    fn maybe_play_realm_event(&self, boss_id: i32, text: &str, should_sound: bool) {
        let (events, custom_sounds, log_events) = match self.settings.read() {
            Ok(s) => (
                s.sound.realm_events.clone(),
                s.sound.custom_sounds.clone(),
                s.live_feed.log_event_notifications,
            ),
            Err(_) => return,
        };
        let sets = crate::realm_event_sound::EventSets::from_assets();

        // Play first so a slow log write never delays the notification. Repeat
        // callouts (e.g. the Crystal Prisoner re-announcing itself, or a boss
        // deduped within the realm) pass `should_sound == false` and are logged
        // silently.
        use realmhound_core::settings::EventMode;
        let any_enabled = events.veteran.mode != EventMode::None
            || events.adept.mode != EventMode::None
            || events.seasonal.mode != EventMode::None
            || events.alien.enabled;
        if should_sound && any_enabled {
            if let Some(t) = crate::realm_event_sound::resolve(&events, boss_id, text, &sets) {
                self.audio_tx.send(AudioCommand::PlayEvent {
                    sound: t.sound,
                    custom_key: t.custom_key,
                    volume: t.volume,
                });
            }
        }

        // Diagnostic log: record every boss call (including silent ones) so
        // missing / mis-mapped notification sounds can be traced later. Written
        // only when the global opt-in toggle is enabled, to the per-account log.
        let display_name = realmhound_core::assets::get_asset_manager()
            .get_object(boss_id)
            .map(|o| o.name().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| text.to_string());
        let history_path = (log_events && self.view_state.account_verified)
            .then(|| self.persistence.event_notifications());
        crate::event_log::log_boss_call(
            history_path,
            boss_id,
            &display_name,
            text,
            should_sound,
            &events,
            &custom_sounds,
            &sets,
        );
    }

    /// Render a custom sound file picker button for a given sound type.
    /// Returns `true` if settings were changed.
    fn render_custom_sound_button(
        &self,
        ui: &mut egui::Ui,
        sound: SoundType,
        settings: &mut realmhound_core::settings::SoundSettings,
    ) -> bool {
        self.render_custom_sound_button_key(
            ui,
            sound.settings_key(),
            &mut settings.custom_sounds,
            &mut settings.custom_sound_names,
        )
    }

    /// Render the shared per-sound row controls for the Loot Sounds / Dungeon
    /// Alerts grids: volume slider, percent label, custom-sound picker (with
    /// delete), and a labelled Test button. The caller renders the leading
    /// enable switch and the trailing `end_row`. Returns `true` if any setting
    /// changed.
    fn render_sound_row_controls(
        &self,
        ui: &mut egui::Ui,
        shadcn: &crate::shadcn_ui::Shadcn,
        sound: SoundType,
        settings: &mut realmhound_core::settings::SoundSettings,
    ) -> bool {
        let mut changed = false;
        let key = sound.settings_key();
        let mut vol = settings.sound_volume(key);
        if shadcn
            .slider_value(ui, &format!("vol_{}", key), &mut vol, 0.0, 1.0)
            .on_hover_cursor(egui::CursorIcon::Default)
            .changed()
        {
            settings.set_sound_volume(key, vol);
            changed = true;
        }
        ui.label(format!("{:.0}%", vol * 100.0));
        changed |= self.render_custom_sound_button(ui, sound, settings);
        if ui.small_button("🔊 Test").hover_tip("Test").clicked() {
            self.audio_tx.send(AudioCommand::Play(sound));
        }
        changed
    }

    /// Custom sound file picker keyed by an arbitrary `custom_sounds` key (used
    /// by realm-event overrides, which key by boss id rather than [`SoundType`]).
    fn render_custom_sound_button_key(
        &self,
        ui: &mut egui::Ui,
        key: &str,
        custom_sounds: &mut std::collections::HashMap<String, String>,
        custom_sound_names: &mut std::collections::HashMap<String, String>,
    ) -> bool {
        if custom_sounds.contains_key(key) {
            let mut changed = false;
            ui.horizontal(|ui| {
                changed =
                    self.render_custom_sound_reset(ui, key, custom_sounds, custom_sound_names);
            });
            changed
        } else {
            self.render_custom_sound_picker(ui, key, custom_sounds, custom_sound_names)
        }
    }

    /// Original (human-readable) display name for a custom sound key, truncated
    /// to 20 characters with a trailing ellipsis. Falls back to the on-disk
    /// filename for entries saved before names were tracked. `None` when the key
    /// has no custom sound.
    fn custom_sound_display_name(
        key: &str,
        custom_sounds: &std::collections::HashMap<String, String>,
        custom_sound_names: &std::collections::HashMap<String, String>,
    ) -> Option<String> {
        if !custom_sounds.contains_key(key) {
            return None;
        }
        let raw = custom_sound_names
            .get(key)
            .or_else(|| custom_sounds.get(key))
            .cloned()
            .unwrap_or_default();
        Some(truncate_ellipsis(&raw, 20))
    }

    /// Render the `♫ <name>` label with a trailing `✕` reset button for a custom
    /// sound. Removes the mapping and deletes the on-disk file on reset. Returns
    /// `true` when reset. Caller is responsible for the surrounding layout.
    fn render_custom_sound_reset(
        &self,
        ui: &mut egui::Ui,
        key: &str,
        custom_sounds: &mut std::collections::HashMap<String, String>,
        custom_sound_names: &mut std::collections::HashMap<String, String>,
    ) -> bool {
        let Some(display) = Self::custom_sound_display_name(key, custom_sounds, custom_sound_names)
        else {
            return false;
        };
        let mut changed = false;
        if ui
            .small_button("✕")
            .hover_tip("Reset to default sound")
            .clicked()
        {
            if let Some(filename) = custom_sounds.remove(key) {
                if let Some(dir) = custom_sounds_dir() {
                    let _ = fs::remove_file(dir.join(&filename));
                }
            }
            custom_sound_names.remove(key);
            changed = true;
        }
        ui.label(RichText::new(format!("♫ {}", display)).small().weak())
            .on_hover_text(
                custom_sound_names
                    .get(key)
                    .or_else(|| custom_sounds.get(key))
                    .cloned()
                    .unwrap_or_default(),
            );
        changed
    }

    /// Render the `📂` button that opens a file dialog to choose a custom sound,
    /// copies it into the custom-sounds directory, and records both the on-disk
    /// filename and the original display name. Returns `true` when a file was
    /// chosen.
    fn render_custom_sound_picker(
        &self,
        ui: &mut egui::Ui,
        key: &str,
        custom_sounds: &mut std::collections::HashMap<String, String>,
        custom_sound_names: &mut std::collections::HashMap<String, String>,
    ) -> bool {
        let mut changed = false;
        if ui
            .small_button("📂")
            .hover_tip("Choose a custom sound file")
            .clicked()
        {
            if let Some(picked) = rfd::FileDialog::new()
                .add_filter("Audio", &["wav", "mp3", "ogg", "flac"])
                .pick_file()
            {
                // Reject files larger than 2 MB
                let too_large = fs::metadata(&picked)
                    .map(|m| m.len() > crate::sound::MAX_CUSTOM_SOUND_BYTES)
                    .unwrap_or(true);
                if too_large {
                    tracing::warn!(
                        "[SOUND] Rejected custom sound (too large): {}",
                        picked.display()
                    );
                } else if let Some(dir) = custom_sounds_dir() {
                    if let Err(e) = fs::create_dir_all(&dir) {
                        tracing::warn!("[SOUND] Failed to create custom sounds dir: {}", e);
                    } else {
                        // Use sound key + original extension as filename. Sanitize
                        // the key so it is always a safe single-path-segment name.
                        let ext = picked.extension().and_then(|e| e.to_str()).unwrap_or("wav");
                        let safe_key: String = key
                            .chars()
                            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                            .collect();
                        let dest_name = format!("{}.{}", safe_key, ext);
                        let dest_path = dir.join(&dest_name);
                        match fs::copy(&picked, &dest_path) {
                            Ok(_) => {
                                let original = picked
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or(&dest_name)
                                    .to_string();
                                custom_sounds.insert(key.to_owned(), dest_name);
                                custom_sound_names.insert(key.to_owned(), original);
                                changed = true;
                                tracing::info!(
                                    "[SOUND] Custom sound set for {}: {}",
                                    key,
                                    dest_path.display()
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "[SOUND] Failed to copy custom sound to {}: {}",
                                    dest_path.display(),
                                    e
                                );
                            }
                        }
                    }
                }
            }
        }
        changed
    }

    /// Render the account settings panel.
    fn render_account_settings(&mut self, ui: &mut egui::Ui, shadcn: &crate::shadcn_ui::Shadcn) {
        use realmhound_core::account::shorten_duplicate_keys;

        ui.add_space(10.0);
        ui.heading("Account");
        ui.add_space(10.0);

        if !self.registry_cache_loaded {
            self.registry_cache_loaded = true;
            let store = AccountRegistryStore::new(self.account.storage_root().clone());
            match store.reconcile() {
                Ok(reconciled) => {
                    self.cached_registry_entries = reconciled.registry.accounts().to_vec();
                }
                Err(e) => {
                    tracing::warn!("[ACCOUNT] failed to load registry: {e}");
                }
            }
        }

        let current_key = self.account.account_key();
        let entries = self.cached_registry_entries.clone();
        let shortened = shorten_duplicate_keys(&entries);
        let now = chrono::Utc::now();

        // Show inline error from a failed switch attempt.
        if let Some(err) = &self.switch_error {
            ui.label(RichText::new(err.as_str()).color(Color32::from_rgb(255, 100, 100)));
            ui.add_space(6.0);
        }

        // Show inline error from a failed discovery attempt.
        if let Some(err) = &self.discovery_error {
            ui.label(RichText::new(err.as_str()).color(Color32::from_rgb(255, 100, 100)));
            ui.add_space(6.0);
        }

        // Show inline error from a failed delete attempt.
        if let Some(err) = &self.delete_error {
            ui.label(RichText::new(err.as_str()).color(Color32::from_rgb(255, 100, 100)));
            ui.add_space(6.0);
        }

        // Discovery confirmation replaces the profile list while active.
        if self.discovery_confirm_active {
            shadcn.card(ui, "discovery_confirm", "Add Account", |ui| {
                ui.label(
                    "RealmHound will save the current profile and restart into \
                     discovery mode. Connect to a game server with the account \
                     you want to add.",
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if shadcn.btn(ui, "Cancel").clicked() {
                        self.discovery_confirm_active = false;
                        self.discovery_error = None;
                    }
                    if shadcn.btn(ui, "Restart & Discover").clicked() {
                        self.discovery_confirm_active = false;
                        self.discovery_error = None;
                        let store = AccountRegistryStore::new(self.account.storage_root().clone());
                        match store.begin_discovery(chrono::Utc::now()) {
                            Ok(_) => {
                                self.pending_relaunch = Some(RelaunchReason::AccountDiscovery);
                            }
                            Err(e) => {
                                tracing::error!("[DISCOVERY] begin failed: {e}");
                                self.discovery_error = Some(format!("Cannot start discovery: {e}"));
                            }
                        }
                    }
                });
            });
            return;
        }

        // Switch confirmation replaces the profile list while active.
        if let Some(target_key) = self.switch_confirm_target {
            let target_name = entries
                .iter()
                .find(|e| e.key() == target_key)
                .and_then(|e| e.display_name())
                .unwrap_or("the selected account");

            shadcn.card(ui, "switch_confirm", "Switch Account", |ui| {
                ui.label(format!(
                    "RealmHound will save the current profile and restart \
                     using {target_name}. Other connected accounts will remain ignored."
                ));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if shadcn.btn(ui, "Cancel").clicked() {
                        self.switch_confirm_target = None;
                    }
                    if shadcn.btn(ui, "Switch & Restart").clicked() {
                        let key = target_key;
                        self.switch_confirm_target = None;
                        self.switch_error = None;
                        // Full validation: begin_select validates the profile
                        // manifest, version, and identity binding — the same
                        // checks select() will run inside relaunch().
                        let store = AccountRegistryStore::new(self.account.storage_root().clone());
                        match store.begin_select(key, chrono::Utc::now()) {
                            Ok(_) => {
                                self.pending_relaunch = Some(RelaunchReason::AccountSwitch(key));
                            }
                            Err(e) => {
                                tracing::error!("[SWITCH] validation failed: {e}");
                                self.switch_error = Some(format!("Cannot switch: {e}"));
                            }
                        }
                    }
                });
            });
            return;
        }

        // Delete confirmation replaces the profile list while active.
        if let Some(target_key) = self.delete_confirm_target {
            let is_active = target_key == current_key;
            let target_name = entries
                .iter()
                .find(|e| e.key() == target_key)
                .and_then(|e| e.display_name())
                .unwrap_or("this account");

            shadcn.card(ui, "delete_confirm", "Delete Account", |ui| {
                ui.label(format!(
                    "All data for {target_name} will be permanently deleted. \
                     This includes loot history, combat history, chat logs, \
                     quest progress, and character data. This cannot be undone."
                ));
                if is_active {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "This is the active account. RealmHound will restart after deletion.",
                        )
                        .small()
                        .weak(),
                    );
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if shadcn.btn(ui, "Cancel").clicked() {
                        self.delete_confirm_target = None;
                        self.delete_error = None;
                    }
                    if shadcn
                        .button_destructive(ui, "Delete permanently", true)
                        .clicked()
                    {
                        self.delete_confirm_target = None;
                        self.delete_error = None;
                        let store = AccountRegistryStore::new(self.account.storage_root().clone());
                        if is_active {
                            self.pending_relaunch = Some(RelaunchReason::AccountDeletion);
                        } else {
                            use realmhound_core::account::ProfileLock;
                            let lock =
                                ProfileLock::acquire(self.account.storage_root(), target_key);
                            match lock {
                                Err(e) => {
                                    tracing::error!("[DELETE] lock failed: {e}");
                                    self.delete_error = Some(format!(
                                        "Cannot delete: profile is in use by another process"
                                    ));
                                }
                                Ok(_lock) => {
                                    match store.delete_account(
                                        target_key,
                                        self.account.credentials().as_ref(),
                                    ) {
                                        Ok(_) => {
                                            self.cached_registry_entries
                                                .retain(|e| e.key() != target_key);
                                        }
                                        Err(e) => {
                                            tracing::error!("[DELETE] failed: {e}");
                                            self.delete_error =
                                                Some(format!("Cannot delete account: {e}"));
                                        }
                                    }
                                }
                            }
                        }
                    }
                });
            });
            return;
        }

        let mut delete_target = None;
        let mut open_folder_key = None;

        shadcn.card(ui, "known_accounts", "Known Accounts", |ui| {
            if entries.is_empty() {
                ui.label(RichText::new("No accounts registered.").weak().italics());
                return;
            }

            for entry in &entries {
                let is_current = entry.key() == current_key;

                ui.horizontal(|ui| {
                    // Status dot
                    let dot_color = if is_current {
                        match self.view_state.main_account_status {
                            MainAccountStatus::Connected => Color32::from_rgb(100, 255, 100),
                            MainAccountStatus::Disconnected => Color32::from_rgb(255, 100, 100),
                            MainAccountStatus::WaitingForConnection => {
                                Color32::from_rgb(150, 150, 150)
                            }
                        }
                    } else {
                        Color32::from_rgb(100, 100, 100)
                    };
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 4.0, dot_color);

                    // Name + optional short key suffix for duplicates
                    let name = entry.display_name().unwrap_or("Unnamed");
                    let label = match shortened.get(&entry.key()) {
                        Some(suffix) => format!("{name} ({suffix})"),
                        None => name.to_string(),
                    };
                    ui.label(RichText::new(label).strong());

                    // Right-aligned: "Current" badge or Switch button, plus actions
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let entry_key = entry.key();
                        if ui
                            .small_button("🗑")
                            .on_hover_text("Delete account")
                            .clicked()
                        {
                            delete_target = Some(entry_key);
                        }
                        if ui
                            .small_button("📂")
                            .on_hover_text("Open profile folder")
                            .clicked()
                        {
                            open_folder_key = Some(entry_key);
                        }
                        if is_current {
                            ui.label(RichText::new("Current").weak().italics());
                        } else if shadcn.btn_small(ui, "Switch").clicked() {
                            self.switch_confirm_target = Some(entry_key);
                        }
                    });
                });

                // Second row: status/last-used
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    if is_current {
                        let status = match self.view_state.main_account_status {
                            MainAccountStatus::Connected => "Connected",
                            MainAccountStatus::Disconnected => "Disconnected",
                            MainAccountStatus::WaitingForConnection => "Waiting for connection",
                        };
                        ui.label(RichText::new(status).small().weak());
                    } else if let Some(last) = entry.last_opened_at() {
                        let ago = format_relative_time(now, last);
                        ui.label(RichText::new(format!("Last used: {ago}")).small().weak());
                    }
                });

                ui.add_space(4.0);
            }
        });

        ui.add_space(8.0);
        if shadcn.btn(ui, "➕ Add Account").clicked() {
            self.discovery_confirm_active = true;
        }

        if let Some(key) = delete_target {
            self.delete_confirm_target = Some(key);
        }
        if let Some(key) = open_folder_key {
            let paths = realmhound_core::account::AccountPaths::new(
                self.account.storage_root().clone(),
                key,
            );
            if let Ok(folder) = paths.root() {
                open_in_file_explorer(&folder);
            }
        }
    }

    /// Render the Treasury settings panel (section reordering).
    fn render_treasury_settings(&mut self, ui: &mut egui::Ui, shadcn: &crate::shadcn_ui::Shadcn) {
        ui.add_space(10.0);
        ui.heading("Treasury");
        ui.add_space(5.0);
        ui.label(
            RichText::new("Configure the display order of sections in the Treasury tab.").weak(),
        );
        ui.add_space(15.0);

        let order = self.treasury_panel.section_order().to_vec();
        let mut swap: Option<(usize, usize)> = None;

        shadcn.card(ui, "treasury_section_order", "Section order", |ui| {
            egui::ScrollArea::vertical()
                .max_height(200.0)
                .show(ui, |ui| {
                    for (idx, section) in order.iter().enumerate() {
                        ui.horizontal(|ui| {
                            let up_enabled = idx > 0;
                            if ui
                                .add_enabled(up_enabled, egui::Button::new("▲").small())
                                .clicked()
                            {
                                swap = Some((idx, idx - 1));
                            }
                            let down_enabled = idx < order.len() - 1;
                            if ui
                                .add_enabled(down_enabled, egui::Button::new("▼").small())
                                .clicked()
                            {
                                swap = Some((idx, idx + 1));
                            }

                            ui.label(section.display_name());
                        });
                    }
                });
        });

        if let Some((a, b)) = swap {
            self.treasury_panel.swap_sections(a, b);
            self.save_treasury_section_order();
        }

        ui.add_space(15.0);

        if shadcn.btn(ui, "🔄 Reset to defaults").clicked() {
            self.treasury_panel.reset_section_order();
            self.save_treasury_section_order();
        }
    }

    /// Persist the current treasury section order to settings.
    fn save_treasury_section_order(&self) {
        if let Ok(mut settings) = self.settings.write() {
            settings.treasury.section_order = self
                .treasury_panel
                .section_order()
                .iter()
                .map(|s| s.to_key().to_string())
                .collect();
            settings.treasury.sort_mode =
                Some(self.treasury_panel.sort_mode().to_key().to_string());
            settings.treasury.custom_category_order = self
                .treasury_panel
                .custom_category_order()
                .iter()
                .map(|c| c.to_key().to_string())
                .collect();
            settings.save();
        }
    }

    /// Render the exit confirmation dialog (modal).
    fn render_exit_confirm(&mut self, ctx: &egui::Context) {
        let shadcn = self.shadcn.clone();
        let mut open = self.modals.show_exit_confirm;
        let cancel = std::cell::Cell::new(false);
        let confirm = std::cell::Cell::new(false);

        egui::Area::new(egui::Id::new("exit_confirm_host"))
            .fixed_pos(egui::pos2(0.0, 0.0))
            .show(ctx, |ui| {
                shadcn.dialog(
                    ui,
                    "exit_confirm_modal",
                    &mut open,
                    "Exit",
                    280.0,
                    120.0,
                    |ui| {
                        ui.add_space(4.0);
                        ui.label("Are you sure you want to exit?");
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            let exit_btn = shadcn.button_destructive(ui, "  Exit  ", true);
                            if exit_btn.clicked()
                                || (exit_btn.has_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                            {
                                confirm.set(true);
                            }
                            let cancel_btn = shadcn.button(ui, "Cancel");
                            if cancel_btn.clicked()
                                || (cancel_btn.has_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                            {
                                cancel.set(true);
                            }
                            // Auto-focus Cancel when neither button has focus yet
                            if !exit_btn.has_focus() && !cancel_btn.has_focus() {
                                cancel_btn.request_focus();
                            }
                        });
                    },
                );
            });

        if confirm.get() {
            self.modals.exit_confirmed = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if cancel.get() {
            open = false;
        }
        self.modals.show_exit_confirm = open;
    }

    /// Render a tab button with icon and label.
    fn render_tab_button(&mut self, ui: &mut egui::Ui, tab: ActiveTab, label: &str, tooltip: &str) {
        let is_selected = self.active_tab == tab;
        let icon = Some(get_tab_icon(tab));
        let claimable = tab == ActiveTab::Missions && self.has_claimable_missions();
        // Quests tab: a Daily Quest Chest whose marks are fully collected shows
        // its chest reward sprite as a claimable cue (mirrors the Missions gift).
        let quest_chest: Option<i32> = if tab == ActiveTab::Quests {
            self.quest_panel
                .claimable_chest_icon(&self.account_data, self.account_data_revision)
        } else {
            None
        };
        let needs_refresh =
            tab == ActiveTab::Missions && !claimable && !self.mission_view.defs_loaded;
        // Quests tab: daily quests that expired at the reset and could not have
        // been re-fetched by the running game (restarted after the reset, but the
        // Tinkerer Cave has not been re-entered) are stale -- flag them like the
        // Missions "data not loaded" state so a re-visit is noticeable. Stale
        // takes precedence over the claimable-chest gold cue: an expired cached
        // chest is no longer actually claimable in game.
        let quests_stale = tab == ActiveTab::Quests
            && self.quest_panel.daily_quests_stale(
                (self.client_launch_unix > 0).then_some(self.client_launch_unix),
                chrono::Utc::now(),
            );
        let show_quest_chest = quest_chest.filter(|_| !quests_stale);
        // Warning states (stale quests, unloaded missions) render a two-line rich
        // tooltip: a normal header line plus a red italic warning line. Other
        // states use the plain string tip.
        let warn: Option<(&str, &str)> = if quests_stale {
            Some((
                "Daily and event quests from the Tinkerer.",
                crate::panels::quest::STALE_QUESTS_TAB_TOOLTIP,
            ))
        } else if needs_refresh {
            Some((
                "Seasonal missions progress and rewards.",
                "Mission data isn't loaded yet - press Refresh (top-right) while logged in.",
            ))
        } else {
            None
        };
        let tip: String = if claimable {
            "Seasonal missions progress and rewards.\nYou have unclaimed mission rewards - open and claim them in the game.".to_string()
        } else if show_quest_chest.is_some() {
            "Daily and event quests from the Tinkerer.\nA Daily Quest Chest is ready to claim - you have enough marks collected.".to_string()
        } else {
            tooltip.to_string()
        };
        let response = self.sprite_renderer.render_icon_button(
            ui,
            icon,
            label,
            is_selected,
            None,
            warn.is_none().then_some(tip.as_str()),
        );
        let response = if let Some((head, warn_line)) = warn {
            response.hover_tip_ui(|ui| {
                ui.label(head);
                ui.label(
                    egui::RichText::new(warn_line)
                        .italics()
                        .color(egui::Color32::from_rgb(210, 55, 55)),
                );
            })
        } else {
            response
        };
        // Claimable-missions cue: tint the Missions tab gold and stamp the same
        // gift badge used on claimable mission cards, so a ready reward is
        // noticeable from any tab.
        if claimable {
            let rect = response.rect;
            let gold = egui::Color32::from_rgb(235, 205, 70);
            let p = ui.painter();
            p.rect_filled(
                rect,
                4.0,
                egui::Color32::from_rgba_unmultiplied(gold.r(), gold.g(), gold.b(), 22),
            );
            p.rect_stroke(
                rect,
                4.0,
                egui::Stroke::new(1.5_f32, gold),
                egui::StrokeKind::Inside,
            );
            let g = 13.0;
            let gift_rect = egui::Rect::from_min_size(
                egui::pos2(rect.right() - g + 3.0, rect.top() - 3.0),
                egui::vec2(g, g),
            );
            self.sprite_renderer
                .draw_embedded_icon(ui, EmbeddedIcon::Gift, gift_rect);
        }
        // Claimable-chest cue for the Quests tab: same gold tint + border as the
        // Missions claimable state, but stamp the claimable chest's reward sprite
        // in the top-right corner instead of the gift badge.
        if let Some(chest_id) = show_quest_chest {
            let rect = response.rect;
            let gold = egui::Color32::from_rgb(235, 205, 70);
            let p = ui.painter();
            p.rect_filled(
                rect,
                4.0,
                egui::Color32::from_rgba_unmultiplied(gold.r(), gold.g(), gold.b(), 22),
            );
            p.rect_stroke(
                rect,
                4.0,
                egui::Stroke::new(1.5_f32, gold),
                egui::StrokeKind::Inside,
            );
            let g = 16.0;
            let chest_rect = egui::Rect::from_min_size(
                egui::pos2(rect.right() - g + 4.0, rect.top() - 4.0),
                egui::vec2(g, g),
            );
            self.sprite_renderer
                .draw_sprite_in_rect(ui, chest_id, chest_rect);
        }
        // Not-loaded cue: outline the Missions tab in warning-red and stamp the
        // in-game [Warning] emote in the top-right (same slot/size as the gift) so
        // a needed Refresh is noticeable from any tab. Red (not gold) keeps it
        // distinct from the claimable-reward state.
        if needs_refresh || quests_stale {
            let rect = response.rect;
            let red = egui::Color32::from_rgb(210, 55, 55);
            ui.painter().rect_stroke(
                rect,
                4.0,
                egui::Stroke::new(1.5_f32, red),
                egui::StrokeKind::Inside,
            );
            let object_id =
                crate::rendering::emotes::resolve_emote_id(get_asset_manager(), "Warning");
            if let Some(id) = object_id {
                let g = 16.0;
                let badge_rect = egui::Rect::from_min_size(
                    egui::pos2(rect.right() - g + 4.0, rect.top() - 4.0),
                    egui::vec2(g, g),
                );
                self.sprite_renderer.draw_sprite_in_rect(ui, id, badge_rect);
            }
        }
        if response.clicked() {
            self.active_tab = tab;
        }
    }

    /// True when any mission currently has a reward ready to claim (mirrors the
    /// panel's claimable test).
    fn has_claimable_missions(&self) -> bool {
        use realmhound_core::api::MissionState;
        self.mission_view.entries.iter().any(|e| {
            matches!(e.state, MissionState::Claimable)
                || (e.complete && !matches!(e.state, MissionState::Claimed | MissionState::Locked))
        })
    }

    /// Compact forge-dust readout shown below the tab header on every tab.
    /// Lays out the seasonal and regular accounts per the user's display mode,
    /// always ordering the current character's account first.
    /// Total active play time (minutes) across all living and dead characters,
    /// decoded from each character's cached PCStats.
    fn total_playtime_minutes(&self) -> i32 {
        let cache = &self.account_data.characters;
        cache
            .characters
            .iter()
            .chain(cache.dead_characters.iter())
            .filter(|c| !c.pc_stats_raw.is_empty())
            .filter_map(|c| realmhound_core::api::decode_pcstats(&c.pc_stats_raw))
            .map(|s| s.minutes_active)
            .sum()
    }

    /// Format a whole number of minutes as a compact play time, e.g. "1d 5h",
    /// "5h 12m" or "12m".
    fn format_playtime(minutes: i32) -> String {
        let m = minutes.max(0);
        let days = m / (24 * 60);
        let hours = (m % (24 * 60)) / 60;
        let mins = m % 60;
        if days > 0 {
            format!("{}d {}h", days, hours)
        } else if hours > 0 {
            format!("{}h {}m", hours, mins)
        } else {
            format!("{}m", mins)
        }
    }

    /// Format a countdown value for a timer widget: the remaining time, "ended"
    /// when the configured reset is in the past, or "not set" when unconfigured.
    fn format_timer(
        target: Option<chrono::DateTime<chrono::Utc>>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> String {
        match target {
            Some(t) => realmhound_core::season::remaining(now, t)
                .map(realmhound_core::season::format_remaining)
                .unwrap_or_else(|| "ended".to_string()),
            None => "not set".to_string(),
        }
    }

    /// Clear any season/battlepass end date whose instant has passed, so the
    /// widget returns to its empty "not set" state and prompts the user to enter
    /// the next cycle's date. Persists and re-applies the change when it fires.
    fn expire_due_season_resets(&mut self) {
        use realmhound_core::season::reset_to_datetime;

        let now = chrono::Utc::now();
        let mut season = {
            let Ok(s) = self.settings.read() else { return };
            s.season.clone()
        };

        let mut changed = false;
        for reset in [&mut season.reset, &mut season.battlepass_reset] {
            if reset.is_set() {
                if let Some(target) = reset_to_datetime(reset) {
                    if now >= target {
                        *reset = Default::default();
                        changed = true;
                    }
                }
            }
        }

        if changed {
            if let Ok(mut s) = self.settings.write() {
                s.season = season.clone();
                s.save();
            }
            self.live_feed_panel.apply_season_config(&season);
        }
    }

    /// Uniform inner content height for every Widget Bar chip. Forcing an
    /// identical height on all chips (large enough for the tallest content: the
    /// 18px sprites and the text galley) keeps the top-aligned row perfectly
    /// level - no chip can grow the row and push its neighbours off the line.
    const WIDGET_CHIP_H: f32 = 22.0;

    /// Fixed width reserved for the leading warning-glyph slot in a stacked
    /// account chip, so the `S:` / `R:` labels line up whether or not a warning
    /// glyph is shown.
    const WIDGET_GLYPH_W: f32 = 15.0;

    /// Icon size for a dust/material amount cell.
    const WIDGET_AMOUNT_ICON: f32 = 18.0;

    /// Fixed pixel width reserved for an amount's `current/max` text, sized to
    /// the widest realistic four-digit value. Fixing this width makes every
    /// amount cell the same size so the icons (and the whole chip) line up
    /// vertically across the two stacked account rows.
    fn widget_amount_number_w(ui: &egui::Ui) -> f32 {
        let big = ui
            .painter()
            .layout_no_wrap(
                "8888".to_string(),
                egui::FontId::proportional(14.0),
                Color32::WHITE,
            )
            .size()
            .x;
        let slash = ui
            .painter()
            .layout_no_wrap(
                "/".to_string(),
                egui::FontId::proportional(14.0),
                Color32::WHITE,
            )
            .size()
            .x;
        let small = ui
            .painter()
            .layout_no_wrap(
                "8888".to_string(),
                egui::FontId::proportional(10.0),
                Color32::WHITE,
            )
            .size()
            .x;
        big + slash + small
    }

    /// Fixed width for the `S:` / `R:` label slot (the wider of the two), so the
    /// first amount icon starts at the same x on both stacked rows.
    fn widget_label_w(ui: &egui::Ui) -> f32 {
        ["S:", "R:"]
            .iter()
            .map(|s| {
                ui.painter()
                    .layout_no_wrap(
                        (*s).to_string(),
                        egui::FontId::proportional(12.0),
                        Color32::WHITE,
                    )
                    .size()
                    .x
            })
            .fold(0.0_f32, f32::max)
    }

    /// Render the fixed-width leading prefix of a stacked account chip: a
    /// warning-glyph slot (⚠ when a type is full/near full, otherwise a subtle
    /// "ok" check) followed by the single-letter account label (`S:` / `R:`).
    fn render_account_prefix(
        ui: &mut egui::Ui,
        letter: &str,
        label_color: Color32,
        label_w: f32,
        warning: Option<(Color32, String)>,
    ) {
        let (grect, gresp) = ui.allocate_exact_size(
            egui::vec2(Self::WIDGET_GLYPH_W, Self::WIDGET_CHIP_H),
            egui::Sense::hover(),
        );
        match warning {
            Some((color, tooltip)) => {
                ui.painter().text(
                    grect.center(),
                    egui::Align2::CENTER_CENTER,
                    "⚠",
                    egui::FontId::proportional(14.0),
                    color,
                );
                gresp.hover_tip(tooltip);
            }
            None => {
                ui.painter().text(
                    grect.center(),
                    egui::Align2::CENTER_CENTER,
                    "✓",
                    egui::FontId::proportional(10.0),
                    Color32::from_rgb(95, 120, 95),
                );
            }
        }

        let (lrect, _) = ui.allocate_exact_size(
            egui::vec2(label_w, Self::WIDGET_CHIP_H),
            egui::Sense::hover(),
        );
        let galley = ui.painter().layout_no_wrap(
            format!("{}:", letter),
            egui::FontId::proportional(12.0),
            label_color,
        );
        let text_pos = egui::pos2(lrect.left(), lrect.center().y - galley.size().y / 2.0);
        ui.painter().galley(text_pos, galley, label_color);
    }

    /// Render one fixed-width amount cell: an icon drawn by `draw_icon` followed
    /// by its `current/max` text left-aligned in a slot of width `num_w`. The
    /// icon always starts at the cell's left edge, so cells line up across rows.
    fn render_amount_cell(
        ui: &mut egui::Ui,
        current: i32,
        max: i32,
        num_w: f32,
        draw_icon: impl FnOnce(&mut egui::Ui, egui::Rect),
    ) -> egui::Response {
        let icon = Self::WIDGET_AMOUNT_ICON;
        let cell_w = icon + num_w;
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(cell_w, Self::WIDGET_CHIP_H),
            egui::Sense::hover(),
        );
        let icon_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.center().y - icon / 2.0),
            egui::vec2(icon, icon),
        );
        draw_icon(ui, icon_rect);

        let mut job = egui::text::LayoutJob::default();
        job.append(
            &current.to_string(),
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(14.0),
                color: Color32::from_rgb(210, 210, 210),
                ..Default::default()
            },
        );
        job.append(
            "/",
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(14.0),
                color: Color32::from_rgb(150, 150, 150),
                ..Default::default()
            },
        );
        job.append(
            &max.to_string(),
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(10.0),
                color: Color32::from_rgb(150, 150, 150),
                ..Default::default()
            },
        );
        let galley = ui.painter().layout_job(job);
        let text_pos = egui::pos2(icon_rect.right(), rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(text_pos, galley, Color32::WHITE);
        resp
    }

    /// Render one Widget Bar chip: an outlined frame with a coloured label and a
    /// value. Returns the frame response so callers can attach a hover tooltip.
    fn widget_chip(
        ui: &mut egui::Ui,
        bg_fill: Color32,
        border: Color32,
        label: &str,
        label_color: Color32,
        value: &str,
        value_color: Color32,
    ) -> egui::Response {
        egui::Frame::NONE
            .fill(bg_fill)
            .stroke(egui::Stroke::new(1.0_f32, border))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(6, 2))
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(0.0, Self::WIDGET_CHIP_H),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_min_height(Self::WIDGET_CHIP_H);
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.label(RichText::new(label).size(12.0).strong().color(label_color));
                        ui.label(RichText::new(value).size(13.0).color(value_color));
                    },
                );
            })
            .response
    }

    /// Like [`Self::widget_chip`] but renders a leading sprite icon before the
    /// text label and value.
    fn widget_chip_labeled_icon(
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        bg_fill: Color32,
        border: Color32,
        label: &str,
        label_color: Color32,
        value: &str,
        value_color: Color32,
        sprite_id: i32,
    ) -> egui::Response {
        egui::Frame::NONE
            .fill(bg_fill)
            .stroke(egui::Stroke::new(1.0_f32, border))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(6, 2))
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(0.0, Self::WIDGET_CHIP_H),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_min_height(Self::WIDGET_CHIP_H);
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                        sprite_renderer.draw_sprite_in_rect(ui, sprite_id, rect);
                        ui.label(RichText::new(label).size(12.0).strong().color(label_color));
                        ui.label(RichText::new(value).size(13.0).color(value_color));
                    },
                );
            })
            .response
    }

    /// Like [`Self::widget_chip`] but renders the value followed by a sprite
    /// icon (no text label), matching the in-game HUD look for fame and gold.
    fn widget_chip_icon(
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        bg_fill: Color32,
        border: Color32,
        value: &str,
        value_color: Color32,
        sprite_id: i32,
    ) -> egui::Response {
        egui::Frame::NONE
            .fill(bg_fill)
            .stroke(egui::Stroke::new(1.0_f32, border))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(6, 2))
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(0.0, Self::WIDGET_CHIP_H),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_min_height(Self::WIDGET_CHIP_H);
                        ui.spacing_mut().item_spacing.x = 3.0;
                        ui.label(RichText::new(value).size(13.0).color(value_color));
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                        sprite_renderer.draw_sprite_in_rect(ui, sprite_id, rect);
                    },
                );
            })
            .response
    }

    /// Render the configurable Widget Bar below the tab header.
    /// Render the configurable Widget Bar below the tab header.
    ///
    /// The bar is up to two rows. Row 1 is `widget_bar.widgets`, row 2 is
    /// `widget_bar.widgets_row2` (both persisted). When the window is too narrow
    /// for row 1, its tail spills onto row 2 for that frame (returning when
    /// widened). When dust is stacked, the dust widget is the pinned left column
    /// of both rows and overflow lands to its right on row 2.
    /// Render the Taskbar: a full-width row of tracked mission/quest progress
    /// pills, shown app-wide directly under the Widget Bar on every tab. Builds
    /// the smart-combined task list from the current mission view and tracked
    /// Daily Quest Chests each frame.
    fn render_taskbar(&mut self, ui: &mut egui::Ui, shadcn: &crate::shadcn_ui::Shadcn) {
        let taskbar = {
            let Ok(s) = self.settings.read() else { return };
            s.taskbar.clone()
        };
        if !taskbar.enabled {
            return;
        }

        // Rebuild the projected task list only when an input changes -- mission
        // data, account/mark counts, the quest list/display state, the live
        // character, or Taskbar settings. Otherwise reuse the cache so the row
        // and its tooltips are not reprojected on every (hover-driven) repaint.
        let sig = TaskbarCacheSig {
            mission_gen: self.owned_mission_gen,
            account_revision: self.account_data_revision,
            quests_revision: self.quest_panel.quests_revision(),
            live_char_id: self.live_char_id,
            hidden_missions: self.missions_panel.hidden_mission_ids().clone(),
            taskbar: taskbar.clone(),
        };
        if self.taskbar_cache_sig.as_ref() != Some(&sig) {
            let quest_tasks = self.quest_panel.taskbar_quest_tasks(&self.account_data);
            let current_char =
                crate::panels::missions::current_char_from(&self.account_data, self.live_char_id);
            self.taskbar_items_cache = crate::panels::taskbar::build_taskbar_items(
                &self.mission_view,
                |id| taskbar.mission_tracked(id) && !sig.hidden_missions.contains(&id),
                quest_tasks,
                current_char,
                taskbar.show_all_choice_options,
            );
            self.taskbar_cache_sig = Some(sig);
        }
        if self.taskbar_items_cache.is_empty() {
            return;
        }
        // Stale daily quests (expired at reset, not re-fetched by the running
        // game) are dimmed + red-outlined in the taskbar so tracked quest pills
        // are not shown as if still available.
        let quests_stale = self.quest_panel.daily_quests_stale(
            (self.client_launch_unix > 0).then_some(self.client_launch_unix),
            chrono::Utc::now(),
        );
        crate::panels::live_feed::render_taskbar_row(
            ui,
            &mut self.sprite_renderer,
            shadcn,
            &self.taskbar_items_cache,
            quests_stale,
        );
    }

    fn render_widget_bar(&mut self, ui: &mut egui::Ui, shadcn: &crate::shadcn_ui::Shadcn) {
        use realmhound_core::settings::{DustDisplayMode, MaterialsDisplayMode, WidgetKind};

        // Snapshot config, releasing the settings lock before rendering.
        let (row1_cfg, row2_cfg, season_cfg) = match self.settings.read() {
            Ok(s) => (
                s.widget_bar.widgets.clone(),
                s.widget_bar.widgets_row2.clone(),
                s.season.clone(),
            ),
            Err(_) => (vec![WidgetKind::Dust], Vec::new(), Default::default()),
        };
        if row1_cfg.is_empty() && row2_cfg.is_empty() {
            return;
        }

        let account_name = self
            .view_state
            .detected_account_name
            .clone()
            .or_else(|| self.account.display_name().map(str::to_string));

        let ctx = WidgetChipCtx {
            bg: shadcn.colors().secondary,
            border: shadcn.colors().border,
            value_color: Color32::from_rgb(210, 210, 210),
            label_color: Color32::from_rgb(170, 170, 170),
            is_seasonal: self.live_feed_panel.is_seasonal(),
            dust_mode: self.live_feed_panel.dust_display_mode(),
            materials_mode: self.live_feed_panel.materials_display_mode(),
            stats: self.account_data.account_stats.clone(),
            season_cfg,
            account_name,
            max_num_chars: self.account_data.max_num_chars,
            skins: self.account_data.owned_skins_count,
            alive_count: self.account_data.characters.characters.len() as i32,
            playtime: self.total_playtime_minutes(),
            now: chrono::Utc::now(),
        };

        let has_timer = row1_cfg
            .iter()
            .chain(row2_cfg.iter())
            .any(|k| matches!(k, WidgetKind::SeasonTimer | WidgetKind::BattlepassTimer));

        // Stacked dust becomes the pinned two-cell left column of both rows.
        let dust_state = self.live_feed_panel.dust_state().clone();
        let both_accounts = dust_state.regular.is_some() && dust_state.seasonal.is_some();
        let dust_enabled =
            row1_cfg.contains(&WidgetKind::Dust) || row2_cfg.contains(&WidgetKind::Dust);
        let is_stacked =
            ctx.dust_mode == DustDisplayMode::TwoLines && both_accounts && dust_enabled;

        // Ordered dust anchor cells (current-character account first), owned so
        // rendering can borrow `self` freely afterwards.
        let dust_anchor: Vec<(String, Color32, realmhound_core::dust::DustAmounts)> = if is_stacked
        {
            let regular = dust_state.regular.as_ref().map(|a| {
                (
                    "Regular".to_string(),
                    Color32::from_rgb(230, 200, 60),
                    a.clone(),
                )
            });
            let seasonal = dust_state.seasonal.as_ref().map(|a| {
                (
                    "Seasonal".to_string(),
                    Color32::from_rgb(80, 220, 200),
                    a.clone(),
                )
            });
            let ordered = if ctx.is_seasonal {
                [seasonal, regular]
            } else {
                [regular, seasonal]
            };
            ordered.into_iter().flatten().collect()
        } else {
            Vec::new()
        };

        // Stacked materials becomes a second pinned column, placed immediately
        // after the dust anchor on each row.
        let stats = ctx.stats.clone();
        let both_materials =
            stats.forge_materials_regular.is_some() && stats.forge_materials_seasonal.is_some();
        let materials_enabled =
            row1_cfg.contains(&WidgetKind::Materials) || row2_cfg.contains(&WidgetKind::Materials);
        let materials_is_stacked = ctx.materials_mode == MaterialsDisplayMode::TwoLines
            && both_materials
            && materials_enabled;

        // Ordered materials anchor cells (current-character account first),
        // owned so rendering can borrow `self` freely afterwards.
        let materials_anchor: Vec<(bool, MaterialAmounts)> = if materials_is_stacked {
            let regular = stats
                .forge_materials_regular
                .as_ref()
                .map(|a| (false, a.clone()));
            let seasonal = stats
                .forge_materials_seasonal
                .as_ref()
                .map(|a| (true, a.clone()));
            let ordered = if ctx.is_seasonal {
                [seasonal, regular]
            } else {
                [regular, seasonal]
            };
            ordered.into_iter().flatten().collect()
        } else {
            Vec::new()
        };

        // Flowing widgets (pinned columns excluded from the flow).
        let flow = |v: &[WidgetKind]| -> Vec<WidgetKind> {
            v.iter()
                .copied()
                .filter(|k| {
                    !(is_stacked && *k == WidgetKind::Dust)
                        && !(materials_is_stacked && *k == WidgetKind::Materials)
                })
                .collect()
        };
        let row1_flow = flow(&row1_cfg);
        let row2_flow = flow(&row2_cfg);

        // Responsive spill: pack row 1 against the available width using widths
        // measured last frame; overflow tail renders on row 2.
        let avail = ui.available_width();
        let spacing = 6.0_f32;
        let dust_col_w = if is_stacked {
            self.widget_widths
                .get(&WidgetKind::Dust)
                .copied()
                .unwrap_or(0.0)
        } else {
            0.0
        };
        let materials_col_w = if materials_is_stacked {
            let w = self
                .widget_widths
                .get(&WidgetKind::Materials)
                .copied()
                .unwrap_or(0.0);
            if w > 0.0 && dust_col_w > 0.0 {
                spacing + w
            } else {
                w
            }
        } else {
            0.0
        };
        let mut used = dust_col_w + materials_col_w;
        // The top row carries the leading Widget Bar marker; reserve its cell
        // (plus one gap) so the responsive spill packs row 1 against the real
        // remaining width and the last chip doesn't overflow behind the icon.
        used += WIDGET_BAR_ICON_CELL + spacing;
        let mut cut = row1_flow.len();
        for (i, k) in row1_flow.iter().enumerate() {
            let w = self.widget_widths.get(k).copied().unwrap_or(0.0);
            let need = if used > 0.0 { spacing + w } else { w };
            if used + need > avail && i > 0 {
                cut = i;
                break;
            }
            used += need;
        }
        let row1_keep: Vec<WidgetKind> = row1_flow[..cut].to_vec();
        let mut row2_final: Vec<WidgetKind> = row2_flow.clone();
        row2_final.extend_from_slice(&row1_flow[cut..]);

        let row_h = 26.0;
        let show_row2 =
            !row2_final.is_empty() || dust_anchor.len() > 1 || materials_anchor.len() > 1;
        let mut widget_rects: Vec<(WidgetKind, egui::Rect, usize)> = Vec::new();

        // Chips are drag handles, not text: disable label selection so dragging
        // doesn't highlight text or flip the pointer to the I-beam cursor.
        ui.style_mut().interaction.selectable_labels = false;

        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            self.render_widget_row(
                ui,
                0,
                dust_anchor.first(),
                materials_anchor.first(),
                &row1_keep,
                &ctx,
                row_h,
                &mut widget_rects,
            );
            if show_row2 {
                self.render_widget_row(
                    ui,
                    1,
                    dust_anchor.get(1),
                    materials_anchor.get(1),
                    &row2_final,
                    &ctx,
                    row_h,
                    &mut widget_rects,
                );
            }
        });

        // Cache this frame's intrinsic chip widths for next frame's spill math.
        self.widget_widths.clear();
        for (k, r, _row) in &widget_rects {
            self.widget_widths.entry(*k).or_insert(r.width());
        }

        // Dim the widget being dragged.
        if let Some(dk) = self.widget_drag {
            for (k, r, _) in &widget_rects {
                if *k == dk {
                    ui.painter()
                        .rect_filled(*r, 4.0, Color32::from_black_alpha(110));
                }
            }
        }

        // Show a move cursor on hover and begin a drag on press+move. Pinned
        // columns (stacked dust / materials) are not draggable.
        for (k, r, _) in &widget_rects {
            let draggable = !(is_stacked && *k == WidgetKind::Dust)
                && !(materials_is_stacked && *k == WidgetKind::Materials);
            if draggable && ui.rect_contains_pointer(*r) {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Move);
                if self.widget_drag.is_none() && ui.input(|i| i.pointer.primary_down()) {
                    let moved = ui.input(|i| i.pointer.delta()).length_sq() > 0.0;
                    if moved {
                        self.widget_drag = Some(*k);
                    }
                }
            }
        }

        // Drag-reorder: draw a drop indicator and commit on release.
        if let Some(drag) = self.widget_drag {
            ui.ctx().request_repaint();
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Grabbing);
            let ptr = ui.input(|i| i.pointer.interact_pos());
            if let Some(p) = ptr {
                let target = Self::widget_drop_target_at(
                    &widget_rects,
                    p,
                    drag,
                    is_stacked,
                    materials_is_stacked,
                );
                self.widget_drop = Some(target);
                if let Some((a, b)) =
                    Self::widget_drop_indicator(&widget_rects, target, drag, is_stacked)
                {
                    ui.painter().line_segment(
                        [a, b],
                        egui::Stroke::new(3.0_f32, Color32::from_rgb(255, 200, 50)),
                    );
                }
            } else {
                self.widget_drop = None;
            }

            if ui.input(|i| i.pointer.any_released()) {
                if let Some(target) = self.widget_drop {
                    self.commit_widget_move(drag, target);
                }
                self.widget_drag = None;
                self.widget_drop = None;
            }
        }

        // Keep countdown widgets ticking without spinning the CPU every frame.
        if has_timer {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs(30));
        }
    }

    /// Render one Widget Bar row: an optional pinned dust anchor cell (leftmost,
    /// when stacked), an optional pinned materials anchor cell (right after the
    /// dust anchor, when stacked), followed by each flowing widget, each in a
    /// fixed-height, top-aligned cell. Appends every rendered cell's rect to
    /// `rects`.
    #[allow(clippy::too_many_arguments)]
    fn render_widget_row(
        &mut self,
        ui: &mut egui::Ui,
        row: usize,
        anchor: Option<&(String, Color32, realmhound_core::dust::DustAmounts)>,
        materials_anchor: Option<&(bool, MaterialAmounts)>,
        kinds: &[realmhound_core::settings::WidgetKind],
        ctx: &WidgetChipCtx,
        row_h: f32,
        rects: &mut Vec<(realmhound_core::settings::WidgetKind, egui::Rect, usize)>,
    ) {
        use realmhound_core::settings::WidgetKind;
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
            ui.set_min_height(row_h);
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);

            // Leading marker on the top row identifies the Widget Bar, mirroring
            // the compass that heads the Live Feed Taskbar. Sized to match that
            // compass (20px art), vertically centred in the row.
            if row == 0 {
                let (rect, resp) = ui.allocate_exact_size(
                    egui::vec2(WIDGET_BAR_ICON_CELL, row_h),
                    egui::Sense::hover(),
                );
                let art = egui::Rect::from_center_size(rect.center(), egui::vec2(20.0, 20.0));
                self.sprite_renderer
                    .draw_outlined_sprite_in_rect_fit_tinted(
                        ui,
                        WIDGET_BAR_ICON_ID,
                        art,
                        Color32::WHITE,
                    );
                resp.hover_tip("Widget Bar - customisable in-game information");
            }

            if let Some((label, color, amounts)) = anchor {
                let resp = ui
                    .push_id(("widget_bar_anchor", row), |ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(0.0, row_h),
                            egui::Layout::left_to_right(egui::Align::Min),
                            |ui| {
                                ui.set_min_height(row_h);
                                Self::render_dust_account(
                                    ui,
                                    &mut self.sprite_renderer,
                                    label,
                                    *color,
                                    amounts,
                                    ctx.bg,
                                    ctx.border,
                                );
                            },
                        )
                        .response
                    })
                    .inner;
                rects.push((WidgetKind::Dust, resp.rect, row));
            }

            if let Some((acct_seasonal, amounts)) = materials_anchor {
                let resp = ui
                    .push_id(("widget_bar_materials_anchor", row), |ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(0.0, row_h),
                            egui::Layout::left_to_right(egui::Align::Min),
                            |ui| {
                                ui.set_min_height(row_h);
                                Self::render_materials_account(
                                    ui,
                                    &mut self.sprite_renderer,
                                    amounts,
                                    *acct_seasonal,
                                    ctx.bg,
                                    ctx.border,
                                );
                            },
                        )
                        .response
                    })
                    .inner;
                rects.push((WidgetKind::Materials, resp.rect, row));
            }

            for (i, kind) in kinds.iter().enumerate() {
                let kind = *kind;
                let resp = ui
                    .push_id(("widget_bar_item", row, i), |ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(0.0, row_h),
                            egui::Layout::left_to_right(egui::Align::Min),
                            |ui| {
                                ui.set_min_height(row_h);
                                ui.spacing_mut().item_spacing.x = 4.0;
                                self.render_widget_chip(ui, kind, ctx);
                            },
                        )
                        .response
                    })
                    .inner;
                rects.push((kind, resp.rect, row));
            }
        });
    }

    /// Render a single Widget Bar chip of the given kind into `ui`.
    fn render_widget_chip(
        &mut self,
        ui: &mut egui::Ui,
        kind: realmhound_core::settings::WidgetKind,
        ctx: &WidgetChipCtx,
    ) {
        use realmhound_core::season;
        use realmhound_core::settings::WidgetKind;

        let bg = ctx.bg;
        let border = ctx.border;
        let value_color = ctx.value_color;
        let label_color = ctx.label_color;
        let is_seasonal = ctx.is_seasonal;
        let dust_mode = ctx.dust_mode;
        let stats = &ctx.stats;
        let season_cfg = &ctx.season_cfg;
        let now = ctx.now;
        let unknown = "—";

        match kind {
            WidgetKind::Dust => {
                Self::render_dust_compact(
                    ui,
                    &mut self.sprite_renderer,
                    self.live_feed_panel.dust_state(),
                    is_seasonal,
                    dust_mode,
                    bg,
                    border,
                );
            }
            WidgetKind::ForgeFire => {
                let val = stats
                    .forge_fire(is_seasonal)
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| unknown.to_string());
                egui::Frame::NONE
                    .fill(bg)
                    .stroke(egui::Stroke::new(1.0_f32, border))
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::symmetric(6, 2))
                    .show(ui, |ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(0.0, Self::WIDGET_CHIP_H),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.set_min_height(Self::WIDGET_CHIP_H);
                                ui.spacing_mut().item_spacing.x = 3.0;
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(9.0, 18.0),
                                    egui::Sense::hover(),
                                );
                                self.sprite_renderer.draw_embedded_icon(
                                    ui,
                                    EmbeddedIcon::ForgeFire,
                                    rect,
                                );
                                ui.label(RichText::new(&val).size(13.0).color(value_color));
                            },
                        );
                    })
                    .response
                    .hover_tip("Forge Fire (shared between Seasonal and Regular)");
            }
            WidgetKind::Materials => {
                Self::render_materials_compact(
                    ui,
                    &mut self.sprite_renderer,
                    stats,
                    is_seasonal,
                    ctx.materials_mode,
                    bg,
                    border,
                    label_color,
                    value_color,
                );
            }
            WidgetKind::Fame => {
                let val = stats
                    .account_fame
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| unknown.to_string());
                Self::widget_chip_icon(
                    ui,
                    &mut self.sprite_renderer,
                    bg,
                    border,
                    &val,
                    Color32::from_rgb(235, 200, 90),
                    crate::panels::character_card::FAME_SPRITE_ID,
                )
                .hover_tip("Your account fame");
            }
            WidgetKind::Gold => {
                let val = stats
                    .gold
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| unknown.to_string());
                Self::widget_chip_icon(
                    ui,
                    &mut self.sprite_renderer,
                    bg,
                    border,
                    &val,
                    Color32::from_rgb(235, 190, 70),
                    crate::panels::character_card::GOLD_SPRITE_ID,
                );
            }
            WidgetKind::Rank => {
                let (val, color) = match stats.stars {
                    Some(s) => {
                        let c = realmhound_core::account_stats::star_color(s);
                        (format!("★ {}", s), Color32::from_rgb(c[0], c[1], c[2]))
                    }
                    None => (format!("★ {}", unknown), value_color),
                };
                let name = ctx
                    .account_name
                    .clone()
                    .unwrap_or_else(|| "Rank".to_string());
                Self::widget_chip(ui, bg, border, &name, label_color, &val, color);
            }
            WidgetKind::CharSlots => {
                let val = if ctx.max_num_chars > 0 {
                    format!("{}/{}", ctx.alive_count, ctx.max_num_chars)
                } else {
                    unknown.to_string()
                };
                Self::widget_chip_labeled_icon(
                    ui,
                    &mut self.sprite_renderer,
                    bg,
                    border,
                    "Slots",
                    label_color,
                    &val,
                    value_color,
                    810,
                )
                .hover_tip(
                    "Character slots occupied and total\nPress Refresh to load data for this widget",
                );
            }
            WidgetKind::Skins => {
                let val = if ctx.skins > 0 {
                    ctx.skins.to_string()
                } else {
                    unknown.to_string()
                };
                Self::widget_chip_labeled_icon(
                    ui,
                    &mut self.sprite_renderer,
                    bg,
                    border,
                    "Skins",
                    label_color,
                    &val,
                    value_color,
                    9138,
                )
                .hover_tip(
                    "Total skins owned on account\nPress Refresh to load data for this widget",
                );
            }
            WidgetKind::Playtime => {
                let val = if ctx.playtime > 0 {
                    Self::format_playtime(ctx.playtime)
                } else {
                    unknown.to_string()
                };
                Self::widget_chip(ui, bg, border, "Playtime", label_color, &val, value_color)
                    .hover_tip(
                        "Sum of Active playtime from all characters (including Seasonal, Regular and Graveyard categories)",
                    );
            }
            WidgetKind::SeasonTimer => {
                let val = Self::format_timer(season::reset_to_datetime(&season_cfg.reset), now);
                Self::widget_chip(
                    ui,
                    bg,
                    border,
                    "Season end:",
                    Color32::from_rgb(120, 200, 235),
                    &val,
                    value_color,
                )
                .hover_tip(
                    "Season end date from live game data.\nPress Refresh to load or update it.",
                );
            }
            WidgetKind::BattlepassTimer => {
                let val = Self::format_timer(season::battlepass_target(season_cfg, now), now);
                Self::widget_chip(
                    ui,
                    bg,
                    border,
                    "Battlepass end:",
                    Color32::from_rgb(200, 160, 235),
                    &val,
                    value_color,
                )
                .hover_tip(
                    "Estimated battlepass end (season midpoint, snapped to Tuesday).\n\
                     Press Refresh to load or update it.",
                );
            }
            WidgetKind::CurrentCharacter => {
                self.render_current_char_chip(ui, bg, border, label_color, value_color);
            }
        }
    }

    /// Render the Current-Character widget chip: the live character's (dyed)
    /// sprite plus its label, with the derived Attributes readout on hover.
    fn render_current_char_chip(
        &mut self,
        ui: &mut egui::Ui,
        bg: Color32,
        border: Color32,
        label_color: Color32,
        value_color: Color32,
    ) {
        // Prefer the live character; otherwise fall back to the last-seen one so
        // the widget stays populated after logout/restart (shown dimmed). The
        // last-seen id is tracked + persisted in refresh_snapshot. "Live" also
        // requires an active connection -- live_char_id lingers after a
        // disconnect, so gate on the connection status too.
        let connected = matches!(
            self.view_state.main_account_status,
            MainAccountStatus::Connected
        );
        let (id, is_live) = match (
            self.live_char_id.filter(|_| connected),
            self.last_live_char_id,
        ) {
            (Some(id), _) => (id, true),
            (None, Some(id)) => (id, false),
            (None, None) => {
                Self::widget_chip(ui, bg, border, "Character", label_color, "—", value_color)
                    .hover_tip("No live character detected yet.");
                return;
            }
        };
        let Some(data) = self.characters_panel.current_char_widget(id, is_live) else {
            Self::widget_chip(ui, bg, border, "Character", label_color, "—", value_color)
                .hover_tip("Character data not loaded. Press Refresh.");
            return;
        };

        let seasonal = data.bonus.seasonal;
        let crucible = data.bonus.crucible_active;
        let resp = egui::Frame::NONE
            .fill(bg)
            .stroke(egui::Stroke::new(1.0_f32, border))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(6, 2))
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(0.0, Self::WIDGET_CHIP_H),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_min_height(Self::WIDGET_CHIP_H);
                        ui.spacing_mut().item_spacing.x = 4.0;
                        // Dim the whole chip (sprite + label) when not live.
                        if !is_live {
                            ui.multiply_opacity(0.55);
                        }
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                        if !self.sprite_renderer.draw_dyed_outlined_character_sprite(
                            ui,
                            data.sprite_id,
                            rect,
                            6,
                            data.tex1,
                            data.tex2,
                        ) {
                            self.sprite_renderer
                                .draw_sprite_in_rect(ui, data.sprite_id, rect);
                        }
                        // Bright white label when live, dimmed default when stale.
                        let lbl_color = if is_live { Color32::WHITE } else { label_color };
                        ui.label(
                            RichText::new(&data.label)
                                .size(12.0)
                                .strong()
                                .color(lbl_color),
                        );
                        // Seasonal (green) / Crucible (red) markers.
                        if seasonal {
                            let (r, _) =
                                ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                            ui.painter().circle_filled(
                                r.center() + egui::vec2(0.0, 1.0),
                                4.0,
                                Color32::from_rgb(0x15, 0xdc, 0xa6),
                            );
                        }
                        if crucible {
                            let (r, _) =
                                ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                            ui.painter().circle_filled(
                                r.center() + egui::vec2(0.0, 1.0),
                                4.0,
                                Color32::from_rgb(0xdc, 0x21, 0x15),
                            );
                        }
                    },
                );
            })
            .response;

        let derived = data.derived;
        let bonus = data.bonus;
        let maxed = data.maxed;
        let identity = data.identity;
        let show_breakdown = self.characters_panel.stats_show_breakdown();
        let (td, td_text) = self.characters_panel.target_def_fields_mut();
        let sr = &mut self.sprite_renderer;
        resp.on_hover_ui(|ui| {
            ui.set_max_width(320.0);
            crate::panels::derived_stats::render_readout(
                ui,
                sr,
                &derived,
                td,
                td_text,
                &maxed,
                &bonus,
                false,
                false,
                show_breakdown,
                Some(&identity),
            );
        });
    }

    /// Kick off a full account refresh: characters plus seasonal mission
    /// definitions and progress. The mission fetch also auto-populates the
    /// season end date from the active season's live `endDate`.
    /// No-op when no access token has been captured.
    fn start_account_refresh(&mut self) {
        let Some(token) = self.captured_access_token.clone() else {
            return;
        };
        // Safety gate: never let RealmHound be the account's first server contact
        // of the UTC day. The daily forge-fire grant only lands on a fresh client
        // relaunch after 00:00 UTC, so require the game client to have been
        // relaunched (a new process, not just an area change) today first.
        if !self.client_relaunched_today() {
            return;
        }
        self.characters_panel
            .fetch_characters(&token, self.token_captured_at);
        // Fetch definitions + player progress off-thread and post the parsed
        // result straight to the processor. Raw bodies are also written to the
        // RealmHound debug folder by the API client.
        let mission_tx = self.worker.control_sender();
        let settings = self.settings.clone();
        let mission_scope = AccountOperationScope {
            account_key: self.account.account_key(),
            expected_account_id: self.account.account_id().clone(),
            generation: self.account_op_generations.next_mission(),
        };
        std::thread::spawn(move || {
            let client = realmhound_core::api::RotmgApiClient::new(token);
            let defs = match client.get_client_seasons() {
                Ok(body) => realmhound_core::api::parse_client_seasons(&body).ok(),
                Err(e) => {
                    tracing::warn!("[MISSIONS] getClientSeasons failed: {e}");
                    None
                }
            };
            let progress = match client.get_player_missions() {
                Ok(body) => Some(realmhound_core::api::parse_player_missions(&body)),
                Err(e) => {
                    tracing::warn!("[MISSIONS] getPlayerMissions failed: {e}");
                    None
                }
            };
            // Auto-populate the season end date from the active season, and the
            // season-start anchor (pool timestamp) for the battlepass estimate.
            if let Some(cs) = defs.as_ref() {
                if let Some(season) = cs
                    .seasons
                    .iter()
                    .find(|s| s.current == 1)
                    .or_else(|| cs.seasons.first())
                {
                    if season.end_date > 0 {
                        let start = progress.as_ref().and_then(|p| p.pool_ts);
                        if let Ok(mut s) = settings.write() {
                            if s.season
                                .apply_live_season_end(season.id, season.end_date, start)
                            {
                                s.save();
                                tracing::info!(
                                    "[MISSIONS] season {} end date auto-set from live data",
                                    season.id
                                );
                            }
                        }
                    }
                }
            }
            if defs.is_some() || progress.is_some() {
                let _ = mission_tx.send(crate::processing::ControlMsg::ApplyMissionData {
                    defs,
                    progress,
                    scope: mission_scope,
                });
            }
        });
    }

    /// Move a dragged widget into the target row at the position implied by the
    /// drop's `before` anchor, persisting the change.
    fn commit_widget_move(
        &mut self,
        drag: realmhound_core::settings::WidgetKind,
        target: WidgetDropTarget,
    ) {
        if let Ok(mut s) = self.settings.write() {
            let wb = &mut s.widget_bar;
            wb.widgets.retain(|k| *k != drag);
            wb.widgets_row2.retain(|k| *k != drag);
            let dest = if target.row == 0 {
                &mut wb.widgets
            } else {
                &mut wb.widgets_row2
            };
            let pos = match target.before {
                Some(bk) => dest.iter().position(|k| *k == bk).unwrap_or(dest.len()),
                None => dest.len(),
            };
            dest.insert(pos, drag);
            s.save();
        }
    }

    /// Drop target (row + `before` anchor) for the widget drag, based on the
    /// pointer position among the rendered widget rects. Picks the nearest row
    /// by y, then the slot by x within that row (ignoring the pinned dust anchor
    /// and the dragged widget itself).
    fn widget_drop_target_at(
        rects: &[(realmhound_core::settings::WidgetKind, egui::Rect, usize)],
        p: egui::Pos2,
        drag: realmhound_core::settings::WidgetKind,
        is_stacked: bool,
        materials_is_stacked: bool,
    ) -> WidgetDropTarget {
        use realmhound_core::settings::WidgetKind;
        let mut rows: Vec<usize> = rects.iter().map(|e| e.2).collect();
        rows.sort_unstable();
        rows.dedup();
        let row = rows
            .iter()
            .copied()
            .min_by(|&a, &b| {
                let ca = Self::row_center_y(rects, a);
                let cb = Self::row_center_y(rects, b);
                (p.y - ca).abs().total_cmp(&(p.y - cb).abs())
            })
            .unwrap_or(0);

        let mut before = None;
        for (k, r, rr) in rects.iter() {
            if *rr != row || *k == drag {
                continue;
            }
            if is_stacked && *k == WidgetKind::Dust {
                continue;
            }
            if materials_is_stacked && *k == WidgetKind::Materials {
                continue;
            }
            if p.x < r.center().x {
                before = Some(*k);
                break;
            }
        }
        WidgetDropTarget { row, before }
    }

    /// Vertical centre of a given row from its rendered rects.
    fn row_center_y(
        rects: &[(realmhound_core::settings::WidgetKind, egui::Rect, usize)],
        row: usize,
    ) -> f32 {
        let ys: Vec<f32> = rects
            .iter()
            .filter(|e| e.2 == row)
            .map(|e| e.1.center().y)
            .collect();
        if ys.is_empty() {
            0.0
        } else {
            ys.iter().sum::<f32>() / ys.len() as f32
        }
    }

    /// Endpoints (top, bottom) of the vertical drop-indicator line for a drop
    /// target: the left edge of the `before` widget, or the right edge of the
    /// row's rightmost cell when appending. Skips the dragged widget.
    fn widget_drop_indicator(
        rects: &[(realmhound_core::settings::WidgetKind, egui::Rect, usize)],
        target: WidgetDropTarget,
        drag: realmhound_core::settings::WidgetKind,
        _is_stacked: bool,
    ) -> Option<(egui::Pos2, egui::Pos2)> {
        let gap = 3.0;
        let (x, top, bottom) = match target.before {
            Some(bk) => {
                let r = rects.iter().find(|e| e.0 == bk && e.2 == target.row)?.1;
                (r.left() - gap, r.top(), r.bottom())
            }
            None => {
                let r = rects
                    .iter()
                    .filter(|e| e.2 == target.row && e.0 != drag)
                    .max_by(|a, b| a.1.right().total_cmp(&b.1.right()))?
                    .1;
                (r.right() + gap, r.top(), r.bottom())
            }
        };
        Some((egui::pos2(x, top), egui::pos2(x, bottom)))
    }

    fn render_dust_compact(
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        dust: &DustState,
        is_seasonal: bool,
        mode: realmhound_core::settings::DustDisplayMode,
        bg_fill: Color32,
        border: Color32,
    ) {
        use realmhound_core::settings::DustDisplayMode;

        let regular = dust
            .regular
            .as_ref()
            .map(|a| ("Regular", Color32::from_rgb(230, 200, 60), a));
        let seasonal = dust
            .seasonal
            .as_ref()
            .map(|a| ("Seasonal", Color32::from_rgb(80, 220, 200), a));

        // Order so the current character's account comes first (leftmost / top).
        let ordered = if is_seasonal {
            [seasonal, regular]
        } else {
            [regular, seasonal]
        };

        let want_label = if is_seasonal { "Seasonal" } else { "Regular" };
        let groups: Vec<_> = ordered
            .into_iter()
            .flatten()
            .filter(|(label, _, _)| mode != DustDisplayMode::CurrentOnly || *label == want_label)
            .collect();

        if groups.is_empty() {
            return;
        }

        match mode {
            DustDisplayMode::TwoLines => {
                ui.vertical(|ui| {
                    for (label, color, amounts) in &groups {
                        ui.horizontal(|ui| {
                            Self::render_dust_account(
                                ui,
                                sprite_renderer,
                                label,
                                *color,
                                amounts,
                                bg_fill,
                                border,
                            );
                        });
                    }
                });
            }
            _ => {
                // Render each account frame inline. With uniform chip heights and
                // a top-aligned parent row, frames line up on one top/bottom line.
                for (label, color, amounts) in &groups {
                    Self::render_dust_account(
                        ui,
                        sprite_renderer,
                        label,
                        *color,
                        amounts,
                        bg_fill,
                        border,
                    );
                }
            }
        }
    }

    /// Render one account's dust inside a lighter outlined frame, prefixed by a
    /// per-account warning glyph when any of its dust types is full or near full.
    fn render_dust_account(
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        label: &str,
        label_color: Color32,
        amounts: &DustAmounts,
        bg_fill: Color32,
        border: Color32,
    ) {
        egui::Frame::NONE
            .fill(bg_fill)
            .stroke(egui::Stroke::new(1.0_f32, border))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(6, 2))
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(0.0, Self::WIDGET_CHIP_H),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_min_height(Self::WIDGET_CHIP_H);
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let letter = &label[..1];
                        let label_w = Self::widget_label_w(ui);
                        Self::render_account_prefix(
                            ui,
                            letter,
                            label_color,
                            label_w,
                            Self::dust_group_warning(label, amounts),
                        );
                        let num_w = Self::widget_amount_number_w(ui);
                        for dust_type in [DustType::Green, DustType::Red, DustType::Purple] {
                            let (cur, max) = amounts.get(dust_type);
                            let object_id = dust_type.object_id();
                            let resp = Self::render_amount_cell(ui, cur, max, num_w, |ui, rect| {
                                sprite_renderer.draw_sprite_in_rect(ui, object_id, rect);
                            });
                            resp.hover_tip(format!("{} {} dust", label, dust_type.name()));
                        }
                    },
                );
            });
    }

    /// Warning glyph color + tooltip for an account's dust, or `None` when no
    /// type is full or near full. Red when any type is full, yellow otherwise.
    fn dust_group_warning(account: &str, amounts: &DustAmounts) -> Option<(Color32, String)> {
        let mut lines: Vec<String> = Vec::new();
        let mut any_full = false;
        for dt in [DustType::Green, DustType::Red, DustType::Purple] {
            let (cur, max) = amounts.get(dt);
            if max <= 0 {
                continue;
            }
            if cur >= max {
                lines.push(format!("{} {} dust is full", account, dt.name()));
                any_full = true;
            } else if cur >= max * 9 / 10 {
                lines.push(format!("{} {} dust is almost full", account, dt.name()));
            }
        }
        if lines.is_empty() {
            return None;
        }
        let color = if any_full {
            Color32::from_rgb(220, 60, 60)
        } else {
            Color32::from_rgb(220, 180, 60)
        };
        Some((color, lines.join("\n")))
    }

    /// Render the forge-materials widget for both accounts, mirroring
    /// [`Self::render_dust_compact`]. Renders the seasonal and regular accounts
    /// (each in its own chip frame) with the account matching the current
    /// character ordered first, honouring the OneLine / TwoLines / CurrentOnly
    /// layouts. Falls back to a single "Forge" unknown chip when no account has
    /// materials data.
    #[allow(clippy::too_many_arguments)]
    fn render_materials_compact(
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        stats: &realmhound_core::account_stats::AccountStats,
        is_seasonal: bool,
        mode: realmhound_core::settings::MaterialsDisplayMode,
        bg_fill: Color32,
        border: Color32,
        label_color: Color32,
        value_color: Color32,
    ) {
        use realmhound_core::settings::MaterialsDisplayMode;

        let regular = stats
            .forge_materials_regular
            .as_ref()
            .map(|a| ("Regular", false, a));
        let seasonal = stats
            .forge_materials_seasonal
            .as_ref()
            .map(|a| ("Seasonal", true, a));

        // Order so the current character's account comes first (leftmost / top).
        let ordered = if is_seasonal {
            [seasonal, regular]
        } else {
            [regular, seasonal]
        };

        let want_label = if is_seasonal { "Seasonal" } else { "Regular" };
        let groups: Vec<_> = ordered
            .into_iter()
            .flatten()
            .filter(|(label, _, _)| {
                mode != MaterialsDisplayMode::CurrentOnly || *label == want_label
            })
            .collect();

        if groups.is_empty() {
            Self::widget_chip(ui, bg_fill, border, "Forge", label_color, "—", value_color);
            return;
        }

        match mode {
            MaterialsDisplayMode::TwoLines => {
                ui.vertical(|ui| {
                    for (_, acct_seasonal, amounts) in &groups {
                        ui.horizontal(|ui| {
                            Self::render_materials_account(
                                ui,
                                sprite_renderer,
                                amounts,
                                *acct_seasonal,
                                bg_fill,
                                border,
                            );
                        });
                    }
                });
            }
            _ => {
                for (_, acct_seasonal, amounts) in &groups {
                    Self::render_materials_account(
                        ui,
                        sprite_renderer,
                        amounts,
                        *acct_seasonal,
                        bg_fill,
                        border,
                    );
                }
            }
        }
    }

    /// Render one account's forge materials: a fixed-width chip with the account
    /// label, a warning-glyph slot, and the 4 tier icons + counts (Mythical,
    /// Legendary, Rare, Common), each with a colored hover tooltip.
    fn render_materials_account(
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        amounts: &MaterialAmounts,
        is_seasonal: bool,
        bg_fill: Color32,
        border: Color32,
    ) {
        egui::Frame::NONE
            .fill(bg_fill)
            .stroke(egui::Stroke::new(1.0_f32, border))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(6, 2))
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(0.0, Self::WIDGET_CHIP_H),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_min_height(Self::WIDGET_CHIP_H);
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let (name, letter, label_color) = if is_seasonal {
                            ("Seasonal", "S", Color32::from_rgb(80, 220, 200))
                        } else {
                            ("Regular", "R", Color32::from_rgb(230, 200, 60))
                        };
                        let label_w = Self::widget_label_w(ui);
                        Self::render_account_prefix(
                            ui,
                            letter,
                            label_color,
                            label_w,
                            Self::materials_group_warning(name, amounts),
                        );
                        let num_w = Self::widget_amount_number_w(ui);
                        for tier in [
                            ForgeMaterialTier::Mythical,
                            ForgeMaterialTier::Legendary,
                            ForgeMaterialTier::Rare,
                            ForgeMaterialTier::Common,
                        ] {
                            let (cur, max) = amounts.get(tier);
                            let icon = match tier {
                                ForgeMaterialTier::Common => EmbeddedIcon::ForgeCommon,
                                ForgeMaterialTier::Rare => EmbeddedIcon::ForgeRare,
                                ForgeMaterialTier::Legendary => EmbeddedIcon::ForgeLegendary,
                                ForgeMaterialTier::Mythical => EmbeddedIcon::ForgeMythical,
                            };
                            let resp = Self::render_amount_cell(ui, cur, max, num_w, |ui, rect| {
                                sprite_renderer.draw_embedded_icon(ui, icon, rect);
                            });
                            let tip = format!("{} {} material", name, tier.name());
                            resp.hover_tip(tip);
                        }
                    },
                );
            });
    }

    /// Warning glyph color + tooltip for an account's forge materials, or `None`
    /// when no tier is full or near full. Red when any tier is full, yellow
    /// otherwise.
    fn materials_group_warning(
        account: &str,
        amounts: &MaterialAmounts,
    ) -> Option<(Color32, String)> {
        let mut lines: Vec<String> = Vec::new();
        let mut any_full = false;
        for tier in ForgeMaterialTier::ALL {
            let (cur, max) = amounts.get(tier);
            if max <= 0 {
                continue;
            }
            if cur >= max {
                lines.push(format!(
                    "{} {} material storage is full",
                    account,
                    tier.name()
                ));
                any_full = true;
            } else if cur >= max * 9 / 10 {
                lines.push(format!(
                    "{} {} material storage is almost full",
                    account,
                    tier.name()
                ));
            }
        }
        if lines.is_empty() {
            return None;
        }
        let color = if any_full {
            Color32::from_rgb(220, 60, 60)
        } else {
            Color32::from_rgb(220, 180, 60)
        };
        Some((color, lines.join("\n")))
    }

    /// Render the main content area.
    fn render_content(&mut self, ui: &mut egui::Ui) {
        let shadcn = self.shadcn.clone();

        // Slightly darker band behind the top bar (page tabs + dust line),
        // derived from the current theme background. A negative outer margin
        // lets the fill bleed past the CentralPanel padding to the window edges
        // on the left, right and top.
        let top_bar_fill = shadcn.top_bar_fill();
        let band = egui::Frame::NONE
            .fill(top_bar_fill)
            .outer_margin(egui::Margin { left: -8, right: -8, top: -8, bottom: 0 })
            .inner_margin(egui::Margin { left: 8, right: 8, top: 4, bottom: 4 })
            .show(ui, |ui| {
        // Tab bar with Settings/About on the right
        ui.horizontal(|ui| {
            // Left side: tabs with icons
            self.render_tab_button(ui, ActiveTab::LiveFeed, "Live Feed", "Real-time game events, notifications and clipboard tools.");
            self.render_tab_button(ui, ActiveTab::Quests, "Quests", "Daily and event quests from the Tinkerer.");
            self.render_tab_button(ui, ActiveTab::Missions, "Missions", "Seasonal mission progress and rewards.");
            self.render_tab_button(ui, ActiveTab::LootHistory, "Loot History", "A searchable record of loot dropped by monsters.");
            self.render_tab_button(ui, ActiveTab::CombatHistory, "Combat History", "DPS breakdown and history of boss fights.");
            self.render_tab_button(ui, ActiveTab::TrophyHall, "Trophy Hall", "Dungeon collections, completion progress and loot statistics.");
            self.render_tab_button(ui, ActiveTab::Treasury, "Treasury", "Browse and search every item on your account.");
            self.render_tab_button(ui, ActiveTab::Vault, "Vault", "View your Vault and other storage containers.");
            self.render_tab_button(ui, ActiveTab::Characters, "Characters", "Characters, equipment, statistics and progression.");
            self.render_tab_button(ui, ActiveTab::Exaltations, "Exaltations", "Account-wide exaltation progress broken down by class.");
            self.render_tab_button(ui, ActiveTab::Party, "Party", "Party management, player information and utilities.");
            self.render_tab_button(ui, ActiveTab::Chat, "Chat", "Searchable chat history and message logs.");
            // Pet Yard is a diagnostic-only tab, hidden from normal release
            // builds. Set the REALMHOUND_PET_YARD env var to reveal it.
            if std::env::var_os("REALMHOUND_PET_YARD").is_some() {
                self.render_tab_button(ui, ActiveTab::PetYard, "Pet Yard", "Diagnostics: what RealmHound has detected for each of your pets.");
            }

            // Right side: Settings, Update indicator, Account, About.
            // Collapse these to glyph-only buttons (labels move to tooltips)
            // when the tabs leave too little room, so they never overlap on
            // narrow windows (e.g. 1366x768).
            let full_labels = ["🔄 Refresh", "⚙ Settings", "ℹ About"];
            let btn_padding = 12.0;
            let spacing = ui.spacing().item_spacing.x;
            let full_cluster_w: f32 = full_labels
                .iter()
                .map(|s| {
                    egui::WidgetText::from(*s)
                        .into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Button)
                        .size()
                        .x
                        + btn_padding
                })
                .sum::<f32>()
                + spacing * (full_labels.len() as f32 - 1.0);
            let narrow = ui.available_width() < full_cluster_w + 24.0;
            // The Appearance "Tab Headers" mode also drives these corner buttons:
            // Icons-only shows just the glyph, Text-only drops the glyph, and
            // Both keeps the responsive glyph+text (collapsing when narrow).
            let tab_mode = self.settings.read().map(|s| s.appearance.tab_labels).unwrap_or_default();
            let tab_tooltips = self.settings.read().map(|s| s.appearance.tab_tooltips).unwrap_or(true);
            let (icon_only, text_only) = match tab_mode {
                realmhound_core::settings::TabLabelMode::IconsOnly => (true, false),
                realmhound_core::settings::TabLabelMode::TextOnly => (false, true),
                realmhound_core::settings::TabLabelMode::IconsAndText => (narrow, false),
            };
            let corner_label = |icon: &str, name: &str, full: &str| -> String {
                if icon_only {
                    icon.to_string()
                } else if text_only {
                    name.to_string()
                } else {
                    full.to_string()
                }
            };

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let about_resp = shadcn.btn(ui, corner_label("ℹ", "About", "ℹ About"));
                if icon_only && tab_tooltips {
                    about_resp.clone().hover_tip("About");
                }
                if about_resp.clicked() {
                    self.modals.show_about = true;
                }

                let settings_resp = shadcn.btn(ui, corner_label("⚙", "Settings", "⚙ Settings"));
                if icon_only && tab_tooltips {
                    settings_resp.clone().hover_tip("Settings");
                }
                if settings_resp.clicked() {
                    self.modals.show_settings = true;
                }

                // Update notification indicator
                self.update_panel.render_indicator(ui, &self.update_checker, &shadcn);

                // Refresh button - fetches character/vault data from Realm API
                // Check for API results (needed since panel may not be visible)
                if let Some(data) = self.characters_panel.check_api_result() {
                    // Persist the token only once a fetch positively confirms it
                    // belongs to the selected account. This keeps a mule's token
                    // from silently becoming the saved default. The selected
                    // identity is the verified account context, never the legacy
                    // `settings.account`.
                    let saved_main = self.account.account_id().as_str();
                    let token_belongs_to_main = match &data.account_id {
                        // Positive match with the selected account: safe to persist.
                        Some(incoming) => incoming == saved_main,
                        // The fetch's identity is unknown: do NOT persist (mule).
                        None => false,
                    };
                    if token_belongs_to_main {
                        // Persist the token this fetch used, not the current one:
                        // a mid-flight capture could otherwise swap in a mule token.
                        if let Some((token, captured_at)) =
                            self.characters_panel.last_fetch_token()
                        {
                            self.save_account_credential(token, captured_at);
                        } else if let Some(token) = self.captured_access_token.clone() {
                            self.save_account_credential(&token, self.token_captured_at);
                        }
                    } else {
                        tracing::warn!(
                            "[TOKEN] Not saving token: fetched account differs from the selected account."
                        );
                    }
                    let scope = AccountOperationScope {
                        account_key: self.account.account_key(),
                        expected_account_id: self.account.account_id().clone(),
                        generation: self.account_op_generations.next_character(),
                    };
                    self.worker.send_control(ControlMsg::ApplyApiAccountData(data, scope));
                }
                let has_token = self.captured_access_token.is_some();
                let is_loading = self.characters_panel.is_loading();
                // Withhold refresh until the game client has been relaunched
                // (a fresh process, not just an area change) on the current UTC
                // day: the daily forge-fire grant only lands on such a relaunch,
                // and RealmHound must never claim it first. Presented to the
                // user as an expired token.
                let client_relaunched_today = self.client_relaunched_today();
                let needs_client_login = has_token && !client_relaunched_today;
                let can_refresh = has_token && !is_loading && client_relaunched_today;
                let token_looks_expired = has_token
                    && realmhound_core::api::is_token_expired(self.token_captured_at);

                let refresh_btn = ui
                    .add_enabled_ui(can_refresh, |ui| shadcn.btn(ui, corner_label("🔄", "Refresh", "🔄 Refresh")))
                    .inner;

                if can_refresh {
                    if token_looks_expired {
                        let age_hint = realmhound_core::api::token_age(self.token_captured_at)
                            .map(|age| format!(" (captured {}h ago)", age.num_hours()))
                            .unwrap_or_default();
                        refresh_btn.clone().hover_tip(format!(
                            "Click here to refresh your entire account data at once.\n\n\u{26a0} Your access token looks expired{age_hint}. If the refresh fails, launch the game to capture a fresh one."
                        ));
                    } else {
                        refresh_btn.clone().hover_tip(
                            "Click here to refresh your entire account data at once."
                        );
                    }
                }

                if refresh_btn.clicked() {
                    self.start_account_refresh();
                }

                // Surface refresh failures from any tab (the button is always
                // visible, but panel errors only render on their own tab). Poll
                // the vault globally since its own poll runs only when visible.
                self.vault_panel.check_api_result();
                let refresh_error = self
                    .characters_panel
                    .fetch_error()
                    .or_else(|| self.vault_panel.fetch_error());
                if let Some(err) = refresh_error {
                    ui.label(RichText::new("\u{26a0}").color(Color32::from_rgb(255, 100, 100)))
                        .hover_tip(err.to_string());
                }

                // Show tooltip explaining why button is disabled
                if !can_refresh {
                    if !has_token {
                        refresh_btn.disabled_hover_tip(
                            "No access token - start capture and log into RotMG to capture token"
                        );
                    } else if is_loading {
                        refresh_btn.disabled_hover_tip("Loading...");
                    } else if needs_client_login {
                        refresh_btn.disabled_hover_tip(
                            "Your access token has expired. Restart the game to refresh it."
                        );
                    }
                }

                // Loading indicator
                if is_loading {
                    ui.spinner();
                }
            });
        });

        // Configurable Widget Bar: a left-aligned account-info
        // bar below the tab header, visible on every tab. Bracketed by full-width
        // separators so it reads as a distinct band. When every widget is toggled
        // off the bar is empty, so skip the separator too -- otherwise a stray
        // divider hangs under the tabs.
        let widget_bar_visible = self
            .settings
            .read()
            .map(|s| !(s.widget_bar.widgets.is_empty() && s.widget_bar.widgets_row2.is_empty()))
            .unwrap_or(true);
        if widget_bar_visible {
            shadcn.full_width_separator(ui);
            self.render_widget_bar(ui, &shadcn);
        }

        // Taskbar: a full-width row of tracked mission/quest progress pills,
        // rendered inside the same darker band as the Widget Bar (matching its
        // background, margins and pill outlines) and set off by its own leading
        // separator. Empty/disabled -> nothing is drawn.
        self.render_taskbar(ui, &shadcn);
        });

        // Draw the divider exactly on the bottom edge of the darker band so the
        // separator sits precisely where the two background shades meet. The
        // line is widened by 8px each side to match the band's edge-to-edge fill.
        let band_rect = band.response.rect;
        ui.painter().hline(
            egui::Rangef::new(band_rect.left() - 8.0, band_rect.right() + 8.0),
            band_rect.bottom(),
            egui::Stroke::new(1.0_f32, shadcn.colors().border),
        );

        // Resync stateful panels from the App-owned snapshots when their
        // generation counters change (avoids cloning every frame).
        let char_view_gen = self.view_state.char_view_gen;
        let vault_view_gen = self.view_state.vault_view_gen;
        let account_generation = self.view_state.account_generation;

        // Characters mirror: refresh from the canonical cache on change. Not
        // gated on the active tab -- the Current-Character widget lives in the
        // Widget Bar (visible on every tab) and needs live gear/stat updates
        // regardless of which tab is open. The generation-counter guard already
        // prevents cloning on unchanged frames.
        if char_view_gen != self.last_char_view_gen {
            self.last_char_view_gen = char_view_gen;
            let live_char_id = self.live_char_id;
            self.characters_panel
                .sync_from(&self.account_data.characters, live_char_id);
        }
        // Loot-boost seconds ride outside the character-cache generation (they
        // change every tick). Observe the live value each frame, attributing it
        // to the currently playing character. The panel keeps each card's
        // last-known value (also seeded from an API refresh).
        self.characters_panel
            .observe_live_loot_boost(self.view_state.loot_boost_secs, self.live_char_id);

        // Vault + treasury live-vault data: resync on change.
        if vault_view_gen != self.last_vault_view_gen {
            self.last_vault_view_gen = vault_view_gen;
            let live_vault = self.live_vault_data.clone();
            self.vault_panel.set_live_vault_data(live_vault.clone());
            self.treasury_panel.set_live_vault_data(live_vault);
        }

        // Sync character cache to treasury panel when tab is active and cache changed.
        // Note: keyed on the account_data generation (matches prior timing).
        if self.active_tab == ActiveTab::Treasury {
            if account_generation != self.treasury_cache_generation {
                self.treasury_cache_generation = account_generation;
                self.treasury_panel
                    .set_character_cache(self.account_data.characters.clone());
            }
        }

        // Build shared context for panel rendering
        let labels = self.account_data.characters.custom_labels.clone();

        self.sprite_renderer
            .update_owned_rarities(&self.account_data);

        let api_refresh_allowed = self.client_relaunched_today();
        let mut ctx = PanelContext {
            sprite_renderer: &mut self.sprite_renderer,
            access_token: &self.captured_access_token,
            token_captured_at: self.token_captured_at,
            labels: &labels,
            loot_database: self.loot_reader.as_ref(),
            combat_database: self.combat_reader.as_ref(),
            account_data: &self.account_data,
            account_epoch: self.account_data_revision,
            shadcn: &shadcn,
            client_launch_unix: self.client_launch_unix,
            account_verified: self.view_state.account_verified,
            mission_view: &self.mission_view,
            live_char_id: self.live_char_id,
            api_refresh_allowed,
        };

        // Detect tab activation so panels can reset transient view state (e.g.
        // scroll position) on entry instead of inheriting a stale position.
        if self.active_tab != self.last_active_tab {
            match self.active_tab {
                ActiveTab::CombatHistory => self.combat_panel.reset_scroll(),
                ActiveTab::LootHistory => self.loot_panel.reset_scroll(),
                ActiveTab::Chat => self.chat_panel.reset_scroll(),
                _ => {}
            }
            self.last_active_tab = self.active_tab;
        }

        let actions = {
            realmhound_core::prof_scope!("panel_show");
            match self.active_tab {
                ActiveTab::Characters => self.characters_panel.show(ui, &mut ctx),
                ActiveTab::Exaltations => self.exaltations_panel.show(ui, &mut ctx),
                ActiveTab::LiveFeed => self.live_feed_panel.show(ui, &mut ctx),
                ActiveTab::LootHistory => self.loot_panel.show(ui, &mut ctx),
                ActiveTab::CombatHistory => self.combat_panel.show(ui, &mut ctx),
                ActiveTab::TrophyHall => self.trophy_hall_panel.show(ui, &mut ctx),
                ActiveTab::Vault => self.vault_panel.show(ui, &mut ctx),
                ActiveTab::Quests => self.quest_panel.show(ui, &mut ctx),
                ActiveTab::Treasury => self.treasury_panel.show(ui, &mut ctx),
                ActiveTab::Party => self.party_panel.show(ui, &mut ctx),
                ActiveTab::Chat => self.chat_panel.show(ui, &mut ctx),
                ActiveTab::PetYard => self.pet_yard_panel.show(ui, &mut ctx),
                ActiveTab::Missions => self.missions_panel.show(ui, &mut ctx),
            }
        };

        // Persist treasury sort settings when they change
        if self.treasury_panel.take_sort_settings_dirty() {
            self.save_treasury_section_order();
        }

        self.process_actions(actions);
    }

    /// Process cross-panel actions returned by panels.
    fn process_actions(&mut self, actions: Vec<AppAction>) {
        for action in actions {
            match action {
                AppAction::SwitchTab(tab) => {
                    self.active_tab = tab;
                }
                AppAction::ViewCharacterLoot {
                    char_id,
                    char_name,
                    icon_id,
                } => {
                    self.loot_panel
                        .set_char_filter_exclusive(char_id, char_name, icon_id);
                }
                AppAction::ViewCharacterFights { char_id, char_name } => {
                    self.combat_panel.view_character_fights(char_id, char_name);
                    self.active_tab = ActiveTab::CombatHistory;
                }
                AppAction::OpenFight { selection } => {
                    self.combat_panel.open_fight(selection);
                    self.active_tab = ActiveTab::CombatHistory;
                }
                AppAction::SaveChatSettings => {
                    self.save_chat_settings(self.chat_panel.filters());
                }
                AppAction::SaveLootSettings => {
                    self.save_loot_settings(self.loot_panel.filters());
                }
                AppAction::SaveLiveFeedHeaderSettings {
                    call_for_party,
                    include_server_name,
                    include_realm_name,
                } => {
                    if let Ok(mut s) = self.settings.write() {
                        s.live_feed.call_for_party = call_for_party;
                        s.live_feed.include_server_name = include_server_name;
                        s.live_feed.include_realm_name = include_realm_name;
                        s.save();
                    }
                }
                AppAction::SaveLiveFeedFilterSettings(filters) => {
                    if let Ok(mut s) = self.settings.write() {
                        s.live_feed.filters = filters;
                        s.save();
                    }
                }
                AppAction::SavePartyListLayout(list_layout) => {
                    if let Ok(mut s) = self.settings.write() {
                        s.party.list_layout = list_layout;
                        s.save();
                    }
                }
                AppAction::SaveCharactersSettings(characters) => {
                    if let Ok(mut s) = self.settings.write() {
                        // Preserve the app-owned last-seen live character id.
                        let keep = s.characters.last_live_char_id;
                        s.characters = characters;
                        s.characters.last_live_char_id = keep;
                        s.save();
                    }
                }
                AppAction::SaveMissionsSettings(missions) => {
                    if let Ok(mut s) = self.settings.write() {
                        s.missions = missions;
                        s.save();
                    }
                }
                AppAction::SaveQuestsSettings(quests) => {
                    if let Ok(mut s) = self.settings.write() {
                        s.quests = quests;
                        s.save();
                    }
                }
                AppAction::SaveTaskbarSettings(taskbar) => {
                    if let Ok(mut s) = self.settings.write() {
                        s.taskbar = taskbar.clone();
                        s.save();
                    }
                    self.missions_panel.apply_taskbar_settings(&taskbar);
                    self.quest_panel.apply_taskbar_settings(&taskbar);
                }
                AppAction::SaveQuickClipButtons(buttons) => {
                    if let Ok(mut s) = self.settings.write() {
                        s.live_feed.custom_clip_buttons = buttons;
                        s.save();
                    }
                }
                AppAction::PushKickAlert(name) => {
                    self.live_feed_panel.push_kick_alert(name);
                }
                AppAction::PushJoinRequestAlert(name) => {
                    self.live_feed_panel.push_join_request_alert(name);
                }
                AppAction::PushBossCall { boss_id, text } => {
                    let should_sound = self.live_feed_panel.push_boss_call(boss_id, text.clone());
                    self.maybe_play_realm_event(boss_id, &text, should_sound);
                }
                AppAction::CharacterCacheChanged => {
                    // The processor owns the canonical character cache now; this
                    // action is retained for compatibility but is a no-op.
                }
                AppAction::SetCharacterLabel { char_id, label } => {
                    self.worker
                        .send_control(ControlMsg::SetCharacterLabel { char_id, label });
                }
                AppAction::RemoveCharacter(char_id) => {
                    self.worker
                        .send_control(ControlMsg::RemoveCharacter(char_id));
                }
                AppAction::MoveCharacter {
                    char_id,
                    to_index,
                    seasonal,
                } => {
                    self.worker.send_control(ControlMsg::MoveCharacter {
                        char_id,
                        to_index,
                        seasonal,
                    });
                }
                AppAction::ViewChatWith { sender } => {
                    self.chat_panel.view_chat_with(sender);
                    self.active_tab = ActiveTab::Chat;
                }
            }
        }
    }

    /// Render the status bar.
    fn render_status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            // The party leader crown and party status indicator are part of the
            // moderation tools, so they only appear when those are enabled.
            if self.party_panel.mod_tools() {
                // Crown icon when local player is party leader
                if self.party_panel.is_local_leader() {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                    if !self.sprite_renderer.draw_embedded_icon(
                        ui,
                        crate::rendering::EmbeddedIcon::PartyLeader,
                        rect,
                    ) {
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "👑",
                            egui::FontId::proportional(12.0),
                            egui::Color32::from_rgb(253, 199, 0),
                        );
                    }
                }

                // Party watchlist status indicator
                let banned = self.party_panel.banned_count();
                let watched = self.party_panel.watched_count();
                let member_count = self.party_panel.member_count();

                let (color, status_text) = if banned > 0 {
                    (
                        egui::Color32::from_rgb(255, 100, 100),
                        format!("[!] Party: {} banned", banned),
                    )
                } else if watched > 0 {
                    (
                        egui::Color32::from_rgb(255, 220, 100),
                        format!("[*] Party: {} watched", watched),
                    )
                } else if member_count > 0 {
                    (
                        egui::Color32::from_rgb(100, 255, 100),
                        format!("[OK] Party: Clear | {}", member_count),
                    )
                } else {
                    (
                        egui::Color32::from_rgb(150, 150, 150),
                        "No party".to_string(),
                    )
                };

                ui.label(egui::RichText::new(status_text).color(color));

                ui.separator();
            }

            // Unified connection health indicator (merges account status + capture health)
            use realmhound_core::stream::CaptureHealth;
            let (dot_color, dot_tooltip) = match self.view_state.main_account_status {
                MainAccountStatus::WaitingForConnection => (
                    egui::Color32::from_rgb(150, 150, 150),
                    "Waiting for connection",
                ),
                MainAccountStatus::Disconnected => {
                    (egui::Color32::from_rgb(255, 100, 100), "Disconnected")
                }
                MainAccountStatus::Connected => match self.view_state.capture_health {
                    CaptureHealth::Resyncing => {
                        (egui::Color32::from_rgb(255, 100, 100), "Re-syncing cipher")
                    }
                    CaptureHealth::Degraded => (
                        egui::Color32::from_rgb(255, 220, 100),
                        "Packet loss detected",
                    ),
                    CaptureHealth::Healthy => (egui::Color32::from_rgb(100, 255, 100), "Connected"),
                },
            };
            let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), 4.0, dot_color);
            let dot_label = ui.label(egui::RichText::new(dot_tooltip).color(dot_color));
            dot_label.hover_tip_ui(|ui| {
                ui.label(egui::RichText::new("Connection & Capture").strong());
                ui.separator();
                let vs = &self.view_state;
                ui.label(format!("Account: {:?}", vs.main_account_status));
                ui.label(format!("Capture: {:?}", vs.capture_health));
                if vs.capture_gap_skips > 0
                    || vs.capture_resyncs > 0
                    || vs.capture_packets_dropped > 0
                    || vs.capture_queue_drops > 0
                {
                    ui.separator();
                    ui.label(format!(
                        "Gap-skips: {} ({} bytes lost)",
                        vs.capture_gap_skips, vs.capture_bytes_lost
                    ));
                    ui.label(format!("Resyncs: {}", vs.capture_resyncs));
                    ui.label(format!("Packets dropped: {}", vs.capture_packets_dropped));
                    ui.label(format!("Queue drops: {}", vs.capture_queue_drops));
                    ui.label(format!("Time unsynced: {}ms", vs.capture_unsync_ms));
                }
            });

            // Surface UI-update-channel overflow as an anomaly
            let dropped = self.worker.ui_rx.dropped();
            if dropped > 0 {
                ui.separator();
                ui.label(
                    egui::RichText::new(format!("[!] Dropped {} updates", dropped))
                        .color(egui::Color32::from_rgb(255, 100, 100)),
                );
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(format!("v{}", env!("CARGO_PKG_VERSION")));
            });
        });
    }
}

impl eframe::App for RealmHoundApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(windows)]
        crate::apply_window_chrome(&*_frame);

        #[cfg(all(feature = "latency-diagnostics", windows))]
        {
            use eframe::glow::HasContext as _;

            static GRAPHICS_DIAGNOSTICS: std::sync::Once = std::sync::Once::new();
            GRAPHICS_DIAGNOSTICS.call_once(|| {
                if let Some(gl) = _frame.gl() {
                    // The eframe update callback runs with its OpenGL context current.
                    let (renderer, vendor, version) = unsafe {
                        (
                            gl.get_parameter_string(eframe::glow::RENDERER),
                            gl.get_parameter_string(eframe::glow::VENDOR),
                            gl.get_parameter_string(eframe::glow::VERSION),
                        )
                    };
                    tracing::info!(
                        "[LATENCY][SYSTEM] graphics_renderer=\"{}\" graphics_vendor=\"{}\" graphics_version=\"{}\"",
                        crate::sanitize_system_label(&renderer),
                        crate::sanitize_system_label(&vendor),
                        crate::sanitize_system_label(&version)
                    );
                } else {
                    tracing::info!(
                        "[LATENCY][SYSTEM] graphics_renderer=\"unknown\" graphics_vendor=\"unknown\" graphics_version=\"unknown\""
                    );
                }
            });
        }

        crate::profiling::frame(ctx, self.active_tab.name());
        realmhound_core::prof_scope!("app_update");

        // A requested relaunch takes over: shut down, then spawn or abort.
        if let Some(reason) = self.pending_relaunch.take() {
            self.relaunch(reason);
        }

        // Poll the capture manager for the auto-detected interface name.
        self.capture.poll_detected_interface();

        // Drain worker UI updates and refresh the App-owned model snapshots.
        if self.drain_ui_updates(ctx) {
            ctx.request_repaint();
        }
        self.refresh_snapshot();

        // Account data is never auto-refreshed on launch: any unsolicited API
        // call could be the account's first server contact of the day, which
        // the game client should always make first.
        // The user populates missions/characters via the manual Refresh button
        // (after logging into the game first).

        // Poll sprite atlas loading
        self.sprite_renderer.poll_atlas_loading(ctx);

        // Load embedded overlay sprites (shiny, enchants) - runs once
        self.sprite_renderer.load_overlay_sprites(ctx);

        // Update window state in settings (will be saved on exit)
        self.update_window_state(ctx);

        // Update checker (runs on startup and every 2 hours)
        self.update_checker.update();

        // Clear season/battlepass end dates once their reset instant passes.
        self.expire_due_season_resets();

        // Check if any modal is open
        let modal_open = self.modals.any_blocking_open() || self.update_panel.is_details_open();

        // Bottom status bar
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.add_space(2.0);
            if modal_open {
                ui.disable();
            }
            self.render_status_bar(ui);
            ui.add_space(2.0);
        });

        // Main content - disable interaction when modal is open
        egui::CentralPanel::default().show(ctx, |ui| {
            if modal_open {
                ui.disable();
            }
            self.render_content(ui);
        });

        // About window (modal)
        if self.modals.show_about {
            self.render_about_window(ctx);
        }

        // Settings modal
        if self.modals.show_settings {
            self.render_settings_modal(ctx);
        }

        // Update details window (modal)
        self.update_panel.render_details_window(
            ctx,
            &self.update_checker,
            &self.self_updater,
            &self.shadcn,
        );
        if self.update_panel.take_relaunch_request() {
            self.pending_relaunch = Some(RelaunchReason::Updated);
        }

        // Changelog window (modal)
        if self.modals.show_changelog {
            self.render_changelog_window(ctx);
        }

        // Exit confirmation dialog (modal)
        if self.modals.show_exit_confirm {
            self.render_exit_confirm(ctx);
        }

        // Handle close request
        if ctx.input(|i| i.viewport().close_requested()) {
            let confirm_on_close = self
                .settings
                .read()
                .map(|s| s.appearance.confirm_on_close)
                .unwrap_or(true);
            if self.modals.exit_confirmed || !confirm_on_close {
                // Allow close
            } else {
                // Cancel close and show confirmation
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.modals.show_exit_confirm = true;
            }
        }

        // Repaints are demand-driven: the worker calls request_repaint() when
        // data changes, and animated panels (live-feed countdowns, chat flash)
        // schedule their own repaints while active. So an idle app sleeps instead
        // of burning CPU re-rendering unchanged frames.

        // Publish the tab that was actually rendered this frame so the worker
        // gates repaints on the current view (relevance-gated wakes). Done at
        // frame end so any in-frame tab switch (SwitchTab actions) is reflected.
        self.active_tab_signal
            .store(self.active_tab as u8, Ordering::Relaxed);

        #[cfg(feature = "profiling")]
        {
            let (glow, dye, char_dye, bytes) = self.sprite_renderer.cache_stats();
            crate::profiling::update_cache_stats(glow, dye, char_dye, bytes);
        }
        crate::profiling::end_frame(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Stop the worker (which flushes account/vault data on shutdown), with
        // a bounded join timeout so a stuck worker can't hang exit.
        self.worker.shutdown(std::time::Duration::from_secs(5));

        // Save settings on exit. Also capture the last-seen live character id
        // from app state so it persists even if the mid-session change-write in
        // refresh_snapshot never fired (e.g. very short session).
        if let Ok(mut settings) = self.settings.write() {
            if self.last_live_char_id.is_some() {
                settings.characters.last_live_char_id = self.last_live_char_id;
            }
            settings.save();
            tracing::info!(
                last_live_char_id = ?settings.characters.last_live_char_id,
                "[SETTINGS] Saved settings on exit"
            );
        }
    }
}

#[cfg(test)]
mod reader_tests {
    use super::{AppStartError, RealmHoundApp};
    use realmhound_core::combat::CombatDatabase;
    use realmhound_core::loot::LootDatabase;

    #[test]
    fn selected_readers_open_after_strict_writers() {
        let temp = tempfile::tempdir().unwrap();
        let loot = temp.path().join("databases").join("loot_history.db");
        let combat = temp.path().join("databases").join("combat_history.db");
        // Writers create both files/schemas first (combat before loot).
        drop(CombatDatabase::open_writer(&combat).unwrap());
        drop(LootDatabase::open_writer(&loot, Some(&combat)).unwrap());

        let (loot_reader, combat_reader) =
            RealmHoundApp::open_selected_readers(&loot, &combat).unwrap();
        assert!(loot_reader.is_some());
        assert!(combat_reader.is_some());
    }

    #[test]
    fn missing_combat_database_propagates_reader_error() {
        let temp = tempfile::tempdir().unwrap();
        let loot = temp.path().join("databases").join("loot_history.db");
        let combat = temp.path().join("databases").join("combat_history.db");
        drop(CombatDatabase::open_writer(&combat).unwrap());
        drop(LootDatabase::open_writer(&loot, Some(&combat)).unwrap());
        // A damaged/inaccessible combat DB (removed after the writer) must not be
        // silently swallowed; the reader failure propagates.
        std::fs::remove_file(&combat).unwrap();

        let error = match RealmHoundApp::open_selected_readers(&loot, &combat) {
            Ok(_) => panic!("a missing combat database must not be swallowed"),
            Err(error) => error,
        };
        match error {
            AppStartError::Reader(detail) => assert!(detail.starts_with("combat")),
            other => panic!("expected a reader error, got {other}"),
        }
    }

    #[test]
    fn missing_loot_database_propagates_reader_error() {
        let temp = tempfile::tempdir().unwrap();
        let loot = temp.path().join("databases").join("loot_history.db");
        let combat = temp.path().join("databases").join("combat_history.db");
        drop(CombatDatabase::open_writer(&combat).unwrap());
        // The loot database was never created: its reader failure propagates.
        let error = match RealmHoundApp::open_selected_readers(&loot, &combat) {
            Ok(_) => panic!("a missing loot database must not be swallowed"),
            Err(error) => error,
        };
        match error {
            AppStartError::Reader(detail) => assert!(detail.starts_with("loot")),
            other => panic!("expected a reader error, got {other}"),
        }
    }
}

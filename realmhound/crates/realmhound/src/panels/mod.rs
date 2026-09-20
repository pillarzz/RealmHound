//! Panel trait and shared types for the tab-based UI.
//!
//! Provides a common interface for all tab panels, enabling standardized
//! rendering, event handling, lifecycle hooks, and cross-panel communication
//! via `AppAction`.

pub mod character_card;
pub mod characters;
pub mod chat;
pub mod combat_history;
pub mod derived_stats;
pub mod dungeon_callout;
pub mod event_call;
pub mod exalt_view;
pub mod exaltations;
pub mod live_feed;
pub mod loot;
pub mod loot_filters;
pub mod missions;
pub mod party;
pub mod pet_card;
pub mod pet_yard;
pub mod quest;
pub mod scroll_nav;
pub mod taskbar;
pub mod treasury;
pub mod trophy_hall;
pub mod update;
pub mod vault;

use eframe::egui;
use realmhound_core::{
    combat::CombatDatabase, loot::LootDatabase, vault::AccountData, GameEvent, GameSession,
};
use std::collections::HashMap;

use crate::rendering::SpriteRenderer;
use crate::shadcn_ui::Shadcn;

// ---------------------------------------------------------------------------
// Empty-state helper
// ---------------------------------------------------------------------------

/// Render a standardized empty-state message, centered near the top of the
/// panel: a gray 18px title with an optional plain subtitle beneath it. Keeps
/// every "nothing here yet" message consistent in style, size and position.
pub fn empty_state(ui: &mut egui::Ui, title: &str, subtitle: Option<&str>) {
    ui.vertical_centered(|ui| {
        ui.add_space(50.0);
        ui.label(
            egui::RichText::new(title)
                .size(18.0)
                .color(egui::Color32::GRAY),
        );
        if let Some(subtitle) = subtitle {
            ui.add_space(10.0);
            ui.label(subtitle);
        }
    });
}

/// Resolve a living character's current sprite id and cloth dyes from the
/// (live-updated) character cache. Returns `(icon_id, tex1, tex2)`, or `None`
/// when the character is not in the living cache (dead/unknown) so callers can
/// fall back to a per-row recorded snapshot. Keeps the local player's skin/dyes
/// consistent across Loot History, Live Feed and Combat History.
pub fn character_appearance(account_data: &AccountData, char_id: i32) -> Option<(i32, u32, u32)> {
    if char_id == 0 {
        return None;
    }
    let c = account_data.find_character(char_id)?;
    let icon_id = if c.skin > 0 {
        c.skin
    } else {
        c.class_id as i32
    };
    Some((icon_id, c.tex1, c.tex2))
}

/// Like [`empty_state`] but styled as a warning: a ⚠ glyph and amber text,
/// used when a tab can't show real data until the user takes an action (e.g.
/// pressing Refresh to load mission definitions).
pub fn empty_state_warning(ui: &mut egui::Ui, title: &str, subtitle: Option<&str>) {
    // Matches the warning-red outline stamped on the Missions tab button.
    let red = egui::Color32::from_rgb(210, 55, 55);
    ui.vertical_centered(|ui| {
        ui.add_space(50.0);
        ui.label(
            egui::RichText::new(format!("⚠ {title}"))
                .size(18.0)
                .color(red),
        );
        if let Some(subtitle) = subtitle {
            ui.add_space(10.0);
            ui.label(egui::RichText::new(subtitle).color(red));
        }
    });
}

/// Like [`empty_state`] but renders one or more subtitle lines beneath the
/// title (each on its own row).
pub fn empty_state_lines(ui: &mut egui::Ui, title: &str, subtitles: &[&str]) {
    ui.vertical_centered(|ui| {
        ui.add_space(50.0);
        ui.label(
            egui::RichText::new(title)
                .size(18.0)
                .color(egui::Color32::GRAY),
        );
        for line in subtitles {
            ui.add_space(10.0);
            ui.label(*line);
        }
    });
}

// ---------------------------------------------------------------------------
// ActiveTab
// ---------------------------------------------------------------------------

/// Active tab in the main view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ActiveTab {
    LiveFeed,
    Quests,
    LootHistory,
    TrophyHall,
    Treasury,
    Vault,
    Characters,
    Exaltations,
    Party,
    Chat,
    CombatHistory,
    PetYard,
    Missions,
}

impl ActiveTab {
    /// Recover a tab from its `#[repr(u8)]` discriminant (the shared worker
    /// signal); unknown values fall back to `LiveFeed`.
    pub fn from_u8(value: u8) -> ActiveTab {
        match value {
            0 => ActiveTab::LiveFeed,
            1 => ActiveTab::Quests,
            2 => ActiveTab::LootHistory,
            3 => ActiveTab::TrophyHall,
            4 => ActiveTab::Treasury,
            5 => ActiveTab::Vault,
            6 => ActiveTab::Characters,
            7 => ActiveTab::Exaltations,
            8 => ActiveTab::Party,
            9 => ActiveTab::Chat,
            10 => ActiveTab::CombatHistory,
            11 => ActiveTab::PetYard,
            12 => ActiveTab::Missions,
            _ => ActiveTab::LiveFeed,
        }
    }

    /// Short, stable label used in logs/profiling output.
    pub fn name(self) -> &'static str {
        match self {
            ActiveTab::LiveFeed => "LiveFeed",
            ActiveTab::Quests => "Quests",
            ActiveTab::LootHistory => "LootHistory",
            ActiveTab::TrophyHall => "TrophyHall",
            ActiveTab::Treasury => "Treasury",
            ActiveTab::Vault => "Vault",
            ActiveTab::Characters => "Characters",
            ActiveTab::Exaltations => "Exaltations",
            ActiveTab::Party => "Party",
            ActiveTab::Chat => "Chat",
            ActiveTab::CombatHistory => "CombatHistory",
            ActiveTab::PetYard => "PetYard",
            ActiveTab::Missions => "Missions",
        }
    }
}

// ---------------------------------------------------------------------------
// AppAction
// ---------------------------------------------------------------------------

/// Actions that panels can request from the app.
///
/// Panels return these from `show()` and `handle_event()` instead of directly
/// mutating app state. The app processes them in a single loop, keeping
/// cross-panel wiring in one place.
#[derive(Debug)]
pub enum AppAction {
    /// Switch to a different tab.
    SwitchTab(ActiveTab),
    /// Set exclusive loot filter for a character and switch to Loot History.
    ViewCharacterLoot {
        char_id: i32,
        char_name: String,
        icon_id: i32,
    },
    /// Filter Combat History to a specific character and switch to that tab.
    ViewCharacterFights { char_id: i32, char_name: String },
    /// Open a specific fight/encounter in Combat History.
    OpenFight {
        selection: realmhound_core::combat::FightSelection,
    },
    /// Chat filter settings changed -- app should persist them.
    SaveChatSettings,
    /// Loot filter settings changed -- app should persist them.
    SaveLootSettings,
    /// Live Feed header toggles changed -- app should persist them.
    SaveLiveFeedHeaderSettings {
        /// Prefix callouts with `/p` to post to the party.
        call_for_party: bool,
        /// Prefix callouts with the short server name.
        include_server_name: bool,
        /// Include the realm name in callouts.
        include_realm_name: bool,
    },
    /// Live Feed loot-filter bar changed -- app should persist the selection.
    SaveLiveFeedFilterSettings(realmhound_core::settings::LiveFeedFilterSettings),
    /// Custom quick-clipboard buttons changed -- app should persist them.
    SaveQuickClipButtons(Vec<realmhound_core::settings::QuickClipButton>),
    /// Push a kick alert to the live feed.
    PushKickAlert(String),
    /// Push a join request alert to the live feed (banned player trying to join).
    PushJoinRequestAlert(String),
    /// Push a boss call to the live feed.
    PushBossCall { boss_id: i32, text: String },
    /// Character cache was modified -- app should sync and mark AccountData dirty.
    CharacterCacheChanged,
    /// UI-origin: set a custom label for a character (routed to the processor).
    SetCharacterLabel { char_id: i32, label: String },
    /// UI-origin: remove a (dead) character (routed to the processor).
    RemoveCharacter(i32),
    /// UI-origin: reorder a character within its tab (routed to the processor).
    MoveCharacter {
        char_id: i32,
        to_index: usize,
        seasonal: bool,
    },
    /// Navigate to the Chat tab's history browser filtered to a specific sender.
    /// Used by the Live Feed DM entry right-click.
    ViewChatWith { sender: String },
    /// Party member list layout toggled from the panel's header controls --
    /// app should persist it.
    SavePartyListLayout(realmhound_core::settings::PartyListLayout),
    /// Characters toolbar changed (sort/stat display/section toggles) -- app should persist them.
    SaveCharactersSettings(realmhound_core::settings::CharactersSettings),
    /// Missions panel state changed (hidden/order/collapsed/hide-claimed) -- app should persist it.
    SaveMissionsSettings(realmhound_core::settings::MissionsSettings),
    /// Quests panel state changed (collapsed/disabled sections) -- app should persist it.
    SaveQuestsSettings(realmhound_core::settings::QuestsSettings),
    /// Live Feed Taskbar tracking changed (per-card compass toggle) -- app
    /// should persist it and re-apply to the mission/quest panels.
    SaveTaskbarSettings(realmhound_core::settings::TaskbarSettings),
}

// ---------------------------------------------------------------------------
// PanelContext
// ---------------------------------------------------------------------------

/// Shared context provided to panels during rendering.
///
/// Bundles references to state that multiple panels need, avoiding per-panel
/// parameter sprawl. Built once per frame in `render_content()`.
pub struct PanelContext<'a> {
    /// Sprite renderer for item/character icons.
    pub sprite_renderer: &'a mut SpriteRenderer,
    /// Captured access token for API calls (from Hello packet).
    pub access_token: &'a Option<String>,
    /// Time the current access token was captured (if known).
    pub token_captured_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Character labels (char_id -> custom label). Snapshot taken once per frame.
    pub labels: &'a HashMap<i32, String>,
    /// Loot history database, if available.
    pub loot_database: Option<&'a LootDatabase>,
    /// Combat history database (read-only), if available.
    pub combat_database: Option<&'a CombatDatabase>,
    /// Unified account data (characters + vault). Single source of truth.
    pub account_data: &'a AccountData,
    /// Memo key that changes whenever `account_data` is re-cloned. Prefer over
    /// `AccountData::generation()`, which does not move on live inventory updates.
    pub account_epoch: u64,
    /// Themed shadcn widget helpers.
    pub shadcn: &'a Shadcn,
    /// The running game client's launch time (Unix seconds), or 0 if no launch
    /// has been confirmed this session. Used to distinguish daily quests that
    /// expired while the game kept running (restart to refresh) from quests that
    /// are stale because the game was restarted after the reset but the Tinkerer
    /// Cave has not been re-entered yet.
    pub client_launch_unix: i64,
    /// Whether the captured main connection is the verified main account (not a
    /// mule tentatively accepted before the main's Hello arrived). `false` until
    /// the main loads a map and its account id is verified this session.
    pub account_verified: bool,
    /// Seasonal mission tracker view, cloned from the worker snapshot.
    pub mission_view: &'a crate::processing::MissionView,
    /// Currently-playing character id, if any. `None` when not connected or the
    /// active character is unknown/dead.
    pub live_char_id: Option<i32>,
    /// Whether account API refreshes are currently permitted: true only once the
    /// verified main client has contacted the server on the current UTC day.
    /// Panels must not trigger RotMG API calls when this is false (ensures the
    /// game client makes the day's first server contact). Surfaced to users as
    /// an expired token.
    pub api_refresh_allowed: bool,
}

// ---------------------------------------------------------------------------
// Panel trait
// ---------------------------------------------------------------------------

/// Common interface for all tab panels.
///
/// Each tab panel implements this trait so the app can:
/// - Render panels through a uniform `show()` signature
/// - Forward game events via `handle_event()`
/// - Process cross-panel actions returned by panels
pub trait Panel {
    /// Render the panel UI. Returns actions for the app to process.
    fn show(&mut self, ui: &mut egui::Ui, ctx: &mut PanelContext) -> Vec<AppAction>;

    /// Handle a game event. Returns actions for the app to process.
    ///
    /// Default implementation ignores all events. Override for events
    /// the panel cares about.
    fn handle_event(&mut self, _event: &GameEvent, _session: &GameSession) -> Vec<AppAction> {
        Vec::new()
    }
}

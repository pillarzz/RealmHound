//! User settings persistence.
//!
//! This module handles loading and saving user preferences to disk.
//! Settings are stored in JSON format at `%LOCALAPPDATA%\RealmHound\settings.json`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Current settings file version for migration support.
const SETTINGS_VERSION: u32 = 7;

/// Rename an unparseable data file to a timestamped `.corrupt-<ts>.bak` sibling
/// so a single bad field can't silently wipe the user's data on the next save.
/// Best-effort: failures are logged and otherwise ignored.
pub(crate) fn back_up_corrupt_file(path: &std::path::Path) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let ext = match path.extension().and_then(|e| e.to_str()) {
        Some(e) => format!("{e}.corrupt-{ts}.bak"),
        None => format!("corrupt-{ts}.bak"),
    };
    let backup = path.with_extension(ext);
    match std::fs::rename(path, &backup) {
        Ok(()) => tracing::warn!("[SETTINGS] Backed up corrupt file to {:?}", backup),
        Err(e) => tracing::error!(
            "[SETTINGS] Failed to back up corrupt file {:?}: {}",
            path,
            e
        ),
    }
}

/// A non-destructive settings-load failure from [`Settings::load_explicit`].
///
/// Neither variant relocates or replaces the source document.
#[derive(Debug)]
pub enum SettingsLoadError {
    /// The settings file could not be read.
    Read(std::io::Error),
    /// The settings file is present but not valid settings JSON.
    Parse(serde_json::Error),
}

impl std::fmt::Display for SettingsLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SettingsLoadError::Read(source) => write!(f, "could not read settings: {source}"),
            SettingsLoadError::Parse(source) => write!(f, "settings are not valid: {source}"),
        }
    }
}

impl std::error::Error for SettingsLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SettingsLoadError::Read(source) => Some(source),
            SettingsLoadError::Parse(source) => Some(source),
        }
    }
}

/// User settings that persist between sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Settings file version for future migrations.
    #[serde(default = "default_version")]
    pub version: u32,

    /// Window position and size settings.
    #[serde(default)]
    pub window: WindowSettings,

    /// Loot panel filter settings (display filters).
    #[serde(default)]
    pub loot: LootSettings,

    /// Loot tracking/capture settings (what gets stored in DB).
    #[serde(default)]
    pub loot_tracking: LootTrackingSettings,

    /// Chat panel filter settings.
    #[serde(default)]
    pub chat: ChatSettings,

    /// Stamp of the resources.assets file at last extraction.
    /// Used to detect game updates and trigger automatic re-extraction.
    /// Format: file modified time as seconds since Unix epoch.
    #[serde(default)]
    pub assets_stamp: Option<u64>,

    /// Main account settings for multi-client isolation.
    #[serde(default)]
    pub account: AccountSettings,

    /// Treasury tab settings (section display order).
    #[serde(default, rename = "totals")]
    pub treasury: TreasurySettings,

    /// Characters tab toolbar settings (sort, stat display, section toggles).
    #[serde(default)]
    pub characters: CharactersSettings,

    /// Missions tab settings (hidden missions, drag order, collapsed sections).
    #[serde(default)]
    pub missions: MissionsSettings,

    /// Quests tab settings (collapsed sections, disabled sections).
    #[serde(default)]
    pub quests: QuestsSettings,

    /// Sound notification settings.
    #[serde(default)]
    pub sound: SoundSettings,

    /// Live Feed panel settings.
    #[serde(default)]
    pub live_feed: LiveFeedSettings,

    /// Appearance/theme settings for the shadcn-styled UI.
    #[serde(default)]
    pub appearance: AppearanceSettings,

    #[serde(default)]
    pub party: PartySettings,

    /// Combat History tracking/filter settings (which boss groups to record).
    #[serde(default)]
    pub combat_history: CombatHistorySettings,

    /// Shared season / battlepass schedule. Entered once by
    /// the user; drives both the Widget Bar timers and the pinned warnings.
    #[serde(default)]
    pub season: SeasonConfig,

    /// Widget Bar (the account-info bar below the tabs) configuration.
    #[serde(default)]
    pub widget_bar: WidgetBarSettings,

    /// Taskbar (mission/quest progress pills shown below the Widget Bar on every
    /// tab) configuration. Promoted from `live_feed.taskbar` in settings v6.
    #[serde(default)]
    pub taskbar: TaskbarSettings,

    /// Trophy Hall panel settings (tracked-loot data source).
    #[serde(default)]
    pub trophy_hall: TrophyHallSettings,
}

/// How tab and sub-tab headers display their icon and text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TabLabelMode {
    /// Show both the icon and the text label (default).
    IconsAndText,
    /// Show only the icon; the name moves to the hover tooltip.
    IconsOnly,
    /// Show only the text label; no icon.
    TextOnly,
}

impl Default for TabLabelMode {
    fn default() -> Self {
        Self::IconsAndText
    }
}

/// Appearance settings: which theme preset (or custom primary color + mode) the
/// interface uses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppearanceSettings {
    /// Theme selector key: a preset key (e.g. `default-dark`, `nord`,
    /// `catppuccin-mocha`) or `custom` to use the fields below.
    #[serde(default = "default_preset")]
    pub preset: String,

    /// Custom brand color as `#rrggbb` (drives toggles, focus rings, active tab).
    #[serde(default = "default_custom_primary")]
    pub custom_primary: String,

    /// Custom surface mode: `dark` or `light`.
    #[serde(default = "default_mode")]
    pub mode: String,

    /// Use compact (smaller) controls in panel toolbars and filter bars.
    #[serde(default = "default_compact_ui")]
    pub compact_ui: bool,

    /// How tab and sub-tab headers display their icon and text.
    #[serde(default)]
    pub tab_labels: TabLabelMode,

    /// Show hover tooltips on tab and sub-tab headers.
    #[serde(default = "default_true")]
    pub tab_tooltips: bool,

    /// Ask for confirmation before closing the application window.
    #[serde(default = "default_true")]
    pub confirm_on_close: bool,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            preset: default_preset(),
            custom_primary: default_custom_primary(),
            mode: default_mode(),
            compact_ui: default_compact_ui(),
            tab_labels: TabLabelMode::default(),
            tab_tooltips: true,
            confirm_on_close: true,
        }
    }
}

fn default_preset() -> String {
    "default-dark".to_string()
}

fn default_compact_ui() -> bool {
    true
}

fn default_custom_primary() -> String {
    "#7c83ff".to_string()
}

fn default_mode() -> String {
    "dark".to_string()
}

/// Live Feed panel settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveFeedSettings {
    /// Disable the Dimitus dungeon clipboard callout. When `false` (default),
    /// entering a Dimitus dungeon produces a clipboard callout with a trailing
    /// `dimitus` tag (e.g. `ddocks dimitus j`). When `true`, Dimitus dungeons
    /// produce no clipboard callout.
    #[serde(default)]
    pub disable_dimitus_callout: bool,

    /// Prefix clipboard callouts with `/p` so they post to the party.
    /// Toggled from the Live Feed tab header. Defaults to `true`.
    #[serde(default = "default_true")]
    pub call_for_party: bool,

    /// Prefix clipboard callouts with the short server name (e.g. `USMW2`).
    /// Toggled from the Live Feed tab header. Defaults to `false`.
    #[serde(default)]
    pub include_server_name: bool,

    /// Include the realm name (e.g. `Medusa`) in clipboard callouts.
    /// Toggled from the Live Feed tab header. Defaults to `false`.
    #[serde(default)]
    pub include_realm_name: bool,

    /// How event/boss calls are rendered in the clipboard. The
    /// Live Feed itself always shows full names; this only affects the copied
    /// text. Defaults to [`EventCallMode::ShortName`].
    #[serde(default)]
    pub event_call_mode: EventCallMode,

    /// Where the join marker `j` is placed in a clipboard callout. `j` is always
    /// positioned immediately after the `/p` prefix when at the beginning.
    /// Defaults to [`JoinPosition::End`].
    #[serde(default)]
    pub join_position: JoinPosition,

    /// Whether dungeon clipboard callouts use the curated short nickname or the
    /// full dungeon name. Defaults to [`DungeonNameStyle::Short`].
    #[serde(default)]
    pub dungeon_name_style: DungeonNameStyle,

    /// Whether reward bonuses in clipboard callouts use the `lb`/`db`
    /// abbreviations or the words `loot`/`dust`. Defaults to
    /// [`RewardLabelStyle::Abbreviated`].
    #[serde(default)]
    pub reward_label_style: RewardLabelStyle,

    /// Whether to include the `%` sign in clipboard callout reward/modifier
    /// values. When `true` (default), callouts read e.g.
    /// `15% lb`; when `false`, they read `15 lb`.
    ///
    /// Deprecated: superseded by [`callout_percent`]. Kept for backward
    /// compatibility and migration of pre-v2 settings files.
    #[serde(default = "default_true")]
    pub include_percent_sign: bool,

    /// Where the join marker `j` is placed in **dungeon** clipboard callouts.
    /// Defaults to [`JoinPosition::End`].
    #[serde(default)]
    pub dungeon_join_position: JoinPosition,

    /// Where the join marker `j` is placed in **event** clipboard callouts.
    /// Defaults to [`JoinPosition::None`].
    #[serde(default = "default_event_join")]
    pub event_join_position: JoinPosition,

    /// How the loot-boost tag is rendered in callouts (`lb`, `loot`, or hidden).
    /// Defaults to [`LootLabel::Lb`].
    #[serde(default)]
    pub loot_label: LootLabel,

    /// How the dust-boost tag is rendered in callouts (`db`, `dust`, or hidden).
    /// Defaults to [`DustLabel::Db`].
    #[serde(default)]
    pub dust_label: DustLabel,

    /// How the XP-boost tag is rendered in callouts (`xp` or hidden).
    /// Defaults to [`XpLabel::None`].
    #[serde(default)]
    pub xp_label: XpLabel,

    /// Whether loot/dust/xp callout values include the `%` sign. Defaults to
    /// `false` (e.g. `15 lb`).
    #[serde(default)]
    pub callout_percent: bool,

    /// Whether event clipboard callouts use the curated short nickname or the
    /// full event name. Defaults to [`DungeonNameStyle::Short`].
    #[serde(default)]
    pub event_name_style: DungeonNameStyle,

    /// Whether event callouts append the upcoming-dungeon hint. Defaults to
    /// `true`.
    #[serde(default = "default_true")]
    pub event_add_upcoming: bool,

    /// Editable reward-modifier callout tags. Each entry pairs a canonical
    /// modifier id with the short call to emit; disabled or blank-call entries
    /// are never added to callouts. Seeded with [`default_reward_mods`].
    #[serde(default = "default_reward_mods")]
    pub reward_mods: Vec<RewardModEntry>,

    /// User overrides for dungeon short names, keyed by the dungeon's full
    /// display name. An empty value falls back to the built-in nickname.
    #[serde(default)]
    pub dungeon_name_overrides: std::collections::BTreeMap<String, String>,

    /// User overrides for event short names, keyed by the event's full display
    /// name. An empty value falls back to the built-in nickname.
    #[serde(default)]
    pub event_name_overrides: std::collections::BTreeMap<String, String>,

    /// Callout prefix for Alien Invasion Adept waves. The wave number is
    /// appended (e.g. `adept wave 4`).
    #[serde(default = "default_alien_adept_prefix")]
    pub alien_adept_prefix: String,

    /// Callout prefix for Alien Invasion Veteran waves. The wave number is
    /// appended (e.g. `veteran wave 4`).
    #[serde(default = "default_alien_veteran_prefix")]
    pub alien_veteran_prefix: String,

    /// Append the upcoming boss hint to the wave-4 alien callout ("UFO soon"
    /// for Adept, "calbrik soon" for Veteran). Defaults to `true`.
    #[serde(default = "default_true")]
    pub alien_wave4_boss: bool,

    /// Live Feed loot filters. Independent from the Loot History filters; hides
    /// matching bags from the Live Feed display while loot tracking continues.
    #[serde(default)]
    pub filters: LiveFeedFilterSettings,

    /// How the forge-dust readout lays out the seasonal and regular accounts.
    #[serde(default)]
    pub dust_display_mode: DustDisplayMode,

    /// How the forge-materials readout lays out the seasonal and regular
    /// accounts.
    #[serde(default)]
    pub materials_display_mode: MaterialsDisplayMode,

    /// Show incoming direct messages (whispers) as entries in the Live Feed.
    /// Defaults to `true`.
    #[serde(default = "default_true")]
    pub show_dm_in_live_feed: bool,

    /// Show public dungeon key-pop notifications ("Opened by <player>") as
    /// entries in the Live Feed. Defaults to `true`.
    #[serde(default = "default_true")]
    pub show_key_pops: bool,

    /// Which dungeon difficulty tiers produce a Live Feed key-pop entry. This is
    /// independent of the sound-side [`SoundSettings::keypop_tiers`] filter so
    /// muting a tier's sound never hides its Live Feed entry (and vice versa).
    #[serde(default)]
    pub key_pop_tiers: KeyPopTierFilter,

    /// Show special "area unlock" pops (Wine Cellar Incantation, Lost Halls
    /// Runes, Vial of Pure Darkness) as Live Feed entries. Defaults to `true`.
    #[serde(default = "default_true")]
    pub show_area_unlocks: bool,

    /// Show dungeon-entry callouts (created when the user enters a dungeon) in
    /// the Live Feed. Defaults to `true`.
    #[serde(default = "default_true")]
    pub show_dungeon_entries: bool,
    /// Show realm event / boss announcement callouts in the Live Feed.
    /// Defaults to `true`.
    #[serde(default = "default_true")]
    pub show_boss_calls: bool,

    /// Show loot-drop entries (white/red/etc. bags) in the Live Feed. Defaults
    /// to `true`. Disabling only hides them from the feed -- every bag is still
    /// tracked and recorded in Loot History.
    #[serde(default = "default_true")]
    pub show_loot_drops: bool,

    /// Show realm-close and Oryx-lag warnings in the Live Feed. Defaults to
    /// `true`.
    #[serde(default = "default_true")]
    pub show_realm_warnings: bool,

    /// Show forge-dust cap-reached warnings in the Live Feed. Defaults to
    /// `true`.
    #[serde(default = "default_true")]
    pub show_dust_full: bool,

    /// Capture the server's `/who` reply and show the sorted player roster as a
    /// click-to-copy entry in the Live Feed. Defaults to `true`.
    #[serde(default = "default_true")]
    pub show_who_roster: bool,

    /// User-configurable quick-clipboard buttons shown in the Live Feed header.
    #[serde(default = "default_clip_buttons")]
    pub custom_clip_buttons: Vec<QuickClipButton>,

    /// End-of-cycle pinned warning toggles.
    #[serde(default)]
    pub season_warnings: SeasonWarningSettings,

    /// Legacy Live Feed Taskbar settings. Promoted to the top-level
    /// [`Settings::taskbar`] in settings v6; retained only so pre-v6 files still
    /// deserialize here for the migration. Never written back out.
    #[serde(default, skip_serializing)]
    pub taskbar: TaskbarSettings,

    /// Opt-in: append every Live Feed boss call to the per-account event
    /// notification diagnostic log. Global toggle, `false` by default; when off,
    /// no event-notification history is written.
    #[serde(default)]
    pub log_event_notifications: bool,
}

/// Live Feed Taskbar settings: the full-width bar of mission/quest progress
/// pills shown between the Filters and Live Feed headers. The mission/quest
/// tracking toggles are master switches (on shows every pill, off hides them
/// all) and the per-card compass buttons record explicit overrides on top.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskbarSettings {
    /// Show the Taskbar on the Live Feed page. Defaults to `true`.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Master switch for tracking in-progress missions in the Taskbar. When
    /// toggled, every mission's compass is flipped to match (clearing per-card
    /// overrides); individual cards can still be adjusted afterward via their
    /// compass button. Defaults to `true`.
    #[serde(default = "default_true", alias = "track_missions_by_default")]
    pub track_missions: bool,

    /// Master switch for tracking Daily Quest Chest tasks in the Taskbar. When
    /// toggled, every quest's compass is flipped to match (clearing per-card
    /// overrides); individual cards can still be adjusted afterward. Defaults to
    /// `true`.
    #[serde(default = "default_true", alias = "track_quests_by_default")]
    pub track_quests: bool,

    /// Show remaining seasonal marks (rather than regular) for quest pills.
    /// Defaults to `true`.
    #[serde(default = "default_true")]
    pub prioritize_seasonal_mark_progress: bool,

    /// Show every available choice for multiple-choice (OR) tasks. When `true`
    /// the pill and tooltip always list all matching options; when `false` they
    /// collapse to the single most-progressed option. Defaults to `true`.
    #[serde(default = "default_true")]
    pub show_all_choice_options: bool,

    /// Per-mission tracking overrides keyed by the composite mission uid
    /// (`(tree_id << 32) | mission_id`). Absent means inherit [`track_missions`];
    /// present is an explicit on/off choice.
    #[serde(default)]
    pub mission_overrides: std::collections::BTreeMap<i64, bool>,

    /// Per-quest tracking overrides keyed by quest id. Absent means inherit
    /// [`track_quests`]; present is an explicit on/off choice.
    #[serde(default)]
    pub quest_overrides: std::collections::BTreeMap<String, bool>,
}

impl Default for TaskbarSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            track_missions: true,
            track_quests: true,
            prioritize_seasonal_mark_progress: true,
            show_all_choice_options: true,
            mission_overrides: std::collections::BTreeMap::new(),
            quest_overrides: std::collections::BTreeMap::new(),
        }
    }
}

impl TaskbarSettings {
    /// Whether a mission is currently tracked in the Taskbar: its explicit
    /// override if set, otherwise the mission default.
    pub fn mission_tracked(&self, mission_uid: i64) -> bool {
        self.mission_overrides
            .get(&mission_uid)
            .copied()
            .unwrap_or(self.track_missions)
    }

    /// Whether a quest is currently tracked in the Taskbar: its explicit
    /// override if set, otherwise the quest default.
    pub fn quest_tracked(&self, quest_id: &str) -> bool {
        self.quest_overrides
            .get(quest_id)
            .copied()
            .unwrap_or(self.track_quests)
    }

    /// Record an explicit per-mission tracking choice.
    pub fn set_mission_tracked(&mut self, mission_uid: i64, tracked: bool) {
        self.mission_overrides.insert(mission_uid, tracked);
    }

    /// Record an explicit per-quest tracking choice.
    pub fn set_quest_tracked(&mut self, quest_id: String, tracked: bool) {
        self.quest_overrides.insert(quest_id, tracked);
    }
}

/// Default set of custom clip buttons: a single empty button.
fn default_clip_buttons() -> Vec<QuickClipButton> {
    vec![QuickClipButton::default()]
}

/// A user-configurable quick-clipboard button (label + message).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuickClipButton {
    /// Display label (max 10 characters).
    #[serde(default)]
    pub label: String,
    /// Message copied to clipboard on click (max 128 characters).
    #[serde(default)]
    pub message: String,
}

impl Default for QuickClipButton {
    fn default() -> Self {
        Self {
            label: String::new(),
            message: String::new(),
        }
    }
}

/// Live Feed loot-filter state: bag-type visibility plus item-property
/// (UT/ST/Shiny) and rarity toggles. Mirrors the Loot History filter bar but is
/// persisted separately so each tab remembers its own selection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveFeedFilterSettings {
    #[serde(default = "default_true")]
    pub show_white: bool,
    #[serde(default = "default_true")]
    pub show_orange: bool,
    #[serde(default = "default_true")]
    pub show_red: bool,
    #[serde(default = "default_true")]
    pub show_gold: bool,
    #[serde(default = "default_true")]
    pub show_blue: bool,
    #[serde(default = "default_true")]
    pub show_egg: bool,
    #[serde(default = "default_true")]
    pub show_teal: bool,
    #[serde(default = "default_true")]
    pub show_purple: bool,
    #[serde(default = "default_true")]
    pub show_pink: bool,
    #[serde(default = "default_true")]
    pub show_brown: bool,
    #[serde(default)]
    pub ut: bool,
    #[serde(default)]
    pub st: bool,
    #[serde(default)]
    pub shiny: bool,
    #[serde(default = "default_true")]
    pub r_none: bool,
    #[serde(default = "default_true")]
    pub r_uncommon: bool,
    #[serde(default = "default_true")]
    pub r_rare: bool,
    #[serde(default = "default_true")]
    pub r_legendary: bool,
    #[serde(default = "default_true")]
    pub r_divine: bool,
    /// Show direct-message entries in the feed.
    #[serde(default = "default_true")]
    pub show_dms: bool,
}

impl Default for LiveFeedFilterSettings {
    fn default() -> Self {
        Self {
            show_white: true,
            show_orange: true,
            show_red: true,
            show_gold: true,
            show_blue: true,
            show_egg: true,
            show_teal: true,
            show_purple: true,
            show_pink: true,
            show_brown: true,
            ut: false,
            st: false,
            shiny: false,
            r_none: true,
            r_uncommon: true,
            r_rare: true,
            r_legendary: true,
            r_divine: true,
            show_dms: true,
        }
    }
}

/// How a dungeon's name is rendered in the clipboard callout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DungeonNameStyle {
    /// Use the curated short nickname (e.g. `halls`), falling back to the
    /// lowercased full name when no nickname exists.
    #[default]
    Short,
    /// Use the full dungeon name, lowercased (e.g. `lost halls`).
    Full,
}

/// How reward bonuses are labeled in the clipboard callout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RewardLabelStyle {
    /// Use the `lb`/`db` abbreviations (e.g. `12% lb 18% db`).
    #[default]
    Abbreviated,
    /// Use the words `loot`/`dust` (e.g. `12% loot 18% dust`).
    Words,
}

/// How event/boss calls are abbreviated in the clipboard callout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum EventCallMode {
    /// Use the event's short name (e.g. `rav rot`).
    #[default]
    ShortName,
    /// Use the event's short name plus the upcoming dungeon hint
    /// (e.g. `rav rot, halls soon`).
    ShortNamePlusUpcoming,
}

/// Placement of the join marker `j` in a clipboard callout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum JoinPosition {
    /// Append ` j` at the end of the callout (e.g. `/p USMW2 rav rot j`).
    #[default]
    End,
    /// Insert `j` right after the `/p` prefix (e.g. `/p j USMW2 rav rot`).
    Beginning,
    /// Do not add a join marker (e.g. `/p USMW2 rav rot`).
    None,
}

/// How the loot-boost tag is rendered in a clipboard callout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LootLabel {
    /// Abbreviated `lb` (e.g. `12 lb`).
    #[default]
    Lb,
    /// Full word `loot` (e.g. `12 loot`).
    Loot,
    /// Do not emit a loot tag.
    None,
}

/// How the dust-boost tag is rendered in a clipboard callout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DustLabel {
    /// Abbreviated `db` (e.g. `20 db`).
    #[default]
    Db,
    /// Full word `dust` (e.g. `20 dust`).
    Dust,
    /// Do not emit a dust tag.
    None,
}

/// How the XP-boost tag is rendered in a clipboard callout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum XpLabel {
    /// Emit an `xp` tag (e.g. `15 xp`).
    Xp,
    /// Do not emit an XP tag.
    #[default]
    None,
}

/// A single editable reward-modifier callout tag. Pairs a canonical modifier id
/// with the short call to emit when a dungeon carries that modifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewardModEntry {
    /// Canonical modifier id (uppercase alphanumerics only), e.g. `GENEROUS`.
    /// Matched against dungeon modifier tokens (including legacy tier suffixes).
    pub id: String,
    /// Human-readable display name shown in the settings editor.
    pub name: String,
    /// Short call emitted in the callout. Blank means never emitted.
    pub short: String,
    /// Whether this tag is added to callouts.
    pub enabled: bool,
}

/// Default event join marker placement ([`JoinPosition::None`]).
fn default_event_join() -> JoinPosition {
    JoinPosition::None
}

/// The default reward-modifier callout tags, in emission order. Named tags are
/// enabled; the rarely-called mods ship disabled with a blank call so users can
/// enable and name them. Dimitus is included as an editable preset.
pub fn default_reward_mods() -> Vec<RewardModEntry> {
    const PRESET: &[(&str, &str, &str, bool)] = &[
        // Popular unique mods that players call by name. Syndicate uses the
        // synthetic id `SYNDICATE` (the engine maps it to the tiered `MERCA`..
        // `MERCD` wire ids); the rest use their canonical `mods.xml` ids. Kept
        // ahead of the boost mods so `synd` precedes `generous` as it did when
        // Syndicate was hardcoded first.
        ("NOBLEBOSS", "Noble Boss", "court", true),
        ("PRISMIMIC", "Prismimic", "prismimic", true),
        ("SPIRITSNAKESWARM", "Spirit Snake Swarm", "snakes", true),
        ("SYNDICATE", "Syndicate Takeover", "synd", true),
        ("NILDROPS", "Nildrops", "nildrop", true),
        ("ALIENWORMHOLES", "Alien Wormholes", "alien portal", true),
        ("LANTERNLIGHT", "Lanternlight", "troom activated", true),
        ("CRABRAVE", "Crab Rave", "crab rave", true),
        ("SOULBOOST", "Soul Boost", "soul boost", true),
        ("SQUARED", "Squared", "squared", true),
        (
            "STEAMWORKSMAINTENANCE",
            "Steamworks Maintenance",
            "turrets off",
            true,
        ),
        ("SHATTERSACCEL", "Lingering Magi", "lingering magi", true),
        ("GENEROUS", "Generous", "generous", true),
        ("FOOUNDTREASURE", "Found Treasure!", "troom", true),
        ("SOUVENIR", "Souvenir", "souv", true),
        ("EXALTEDBANNER", "Exalted Banner", "banner", true),
        ("KEYFAIRY", "Key Fairy", "keyf", true),
        ("SKINHUNTER", "Skin Hunter", "skinhunt", true),
        ("MYSTERYSKIN", "Mystery Skin", "skin token", true),
        ("PETCOLLECTOR", "Pet Collector", "pet egg", true),
        ("AGENTOFORYX", "Agent of Oryx", "AoO shards", true),
        ("WANDERERBOSS", "Wanderer", "wanderer", true),
        ("CHEF", "Chef", "chef", true),
        ("BIS", "BIS", "bis", true),
        // Looting/Rewarding are pure loot/dust/xp boosts, already shown by the
        // numeric `lb`/`db`/`xp` tags; off by default to avoid redundancy, but
        // named so users can opt into an explicit tag.
        ("LOOTING", "Looting", "looting", false),
        ("REWARDING", "Rewarding", "rewarding", false),
        ("DUSTSTORM", "Dust Storm", "", false),
        ("EXPERIENCED", "Experienced", "", false),
        ("MYSTERYSTATPOTION", "Mystery Stat Potion", "", false),
        ("COLORFUL", "Colorful", "", false),
        ("ORYXMANIAPORTAL", "Oryxmania Rumble", "", false),
        ("EASTERLOOTBUNNY", "Loot Bunny", "", false),
        // Dimitus is special: when disabled it suppresses the whole Dimitus
        // dungeon callout (matching the legacy `disable_dimitus_callout`). Off by
        // default; migration re-enables it for users who kept Dimitus callouts.
        ("DIMITUS", "Dimitus", "dimitus", false),
    ];
    PRESET
        .iter()
        .map(|(id, name, short, enabled)| RewardModEntry {
            id: (*id).to_string(),
            name: (*name).to_string(),
            short: (*short).to_string(),
            enabled: *enabled,
        })
        .collect()
}

/// How the forge-dust readout arranges the seasonal and regular accounts.
/// The account matching the current character is always ordered first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DustDisplayMode {
    /// Both accounts on one line; current character's account leftmost.
    OneLine,
    /// Both accounts stacked on two lines; current character's account on top.
    TwoLines,
    /// Only the account matching the current character, on a single line.
    #[default]
    CurrentOnly,
}

/// How the forge-materials readout arranges the seasonal and regular accounts.
/// The account matching the current character is always ordered first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MaterialsDisplayMode {
    /// Both accounts on one line; current character's account leftmost.
    OneLine,
    /// Both accounts stacked on two lines; current character's account on top.
    TwoLines,
    /// Only the account matching the current character, on a single line.
    #[default]
    CurrentOnly,
}

impl Default for LiveFeedSettings {
    fn default() -> Self {
        Self {
            disable_dimitus_callout: false,
            call_for_party: true,
            include_server_name: false,
            include_realm_name: false,
            event_call_mode: EventCallMode::default(),
            join_position: JoinPosition::default(),
            dungeon_name_style: DungeonNameStyle::default(),
            reward_label_style: RewardLabelStyle::default(),
            include_percent_sign: true,
            dungeon_join_position: JoinPosition::End,
            event_join_position: JoinPosition::None,
            loot_label: LootLabel::default(),
            dust_label: DustLabel::default(),
            xp_label: XpLabel::default(),
            callout_percent: false,
            event_name_style: DungeonNameStyle::default(),
            event_add_upcoming: true,
            reward_mods: default_reward_mods(),
            dungeon_name_overrides: std::collections::BTreeMap::new(),
            event_name_overrides: std::collections::BTreeMap::new(),
            alien_adept_prefix: default_alien_adept_prefix(),
            alien_veteran_prefix: default_alien_veteran_prefix(),
            alien_wave4_boss: true,
            filters: LiveFeedFilterSettings::default(),
            dust_display_mode: DustDisplayMode::default(),
            materials_display_mode: MaterialsDisplayMode::default(),
            show_dm_in_live_feed: true,
            show_who_roster: true,
            show_key_pops: true,
            key_pop_tiers: KeyPopTierFilter::default(),
            show_area_unlocks: true,
            show_dungeon_entries: true,
            show_boss_calls: true,
            show_loot_drops: true,
            show_realm_warnings: true,
            show_dust_full: true,
            custom_clip_buttons: default_clip_buttons(),
            season_warnings: SeasonWarningSettings::default(),
            taskbar: TaskbarSettings::default(),
            log_event_notifications: false,
        }
    }
}

impl LiveFeedSettings {
    /// Migrate pre-v2 callout settings into the v2 model. Copies the old shared
    /// join marker, reward-label style, `%`-sign, event-mode, and Dimitus toggle
    /// into their split successors. Idempotent per settings-version gating in
    /// [`Settings::load`]; only run once when upgrading a v1 file.
    pub fn migrate_v2(&mut self) {
        self.dungeon_join_position = self.join_position;
        self.callout_percent = self.include_percent_sign;
        self.event_add_upcoming =
            matches!(self.event_call_mode, EventCallMode::ShortNamePlusUpcoming);
        match self.reward_label_style {
            RewardLabelStyle::Abbreviated => {
                self.loot_label = LootLabel::Lb;
                self.dust_label = DustLabel::Db;
            }
            RewardLabelStyle::Words => {
                self.loot_label = LootLabel::Loot;
                self.dust_label = DustLabel::Dust;
            }
        }
        // Carry the old Dimitus toggle onto its editable preset entry. The old
        // flag `disable_dimitus_callout == false` meant callouts were enabled.
        if let Some(entry) = self.reward_mods.iter_mut().find(|e| e.id == "DIMITUS") {
            entry.enabled = !self.disable_dimitus_callout;
        }
    }

    /// Reconcile the user's saved reward-mod list against the current presets,
    /// keyed by id. Rebuilds the list in canonical [`default_reward_mods`] order
    /// so a migrated config matches a fresh one (e.g. `synd` stays ahead of
    /// `generous`), while preserving each existing entry's `enabled`/`short`
    /// customizations. Any user-added ids not in the presets are kept, placed
    /// just before the trailing Dimitus entry. Idempotent.
    pub fn reconcile_reward_mods(&mut self) {
        use std::collections::HashSet;
        let user: Vec<RewardModEntry> = std::mem::take(&mut self.reward_mods);
        let default_ids: HashSet<String> =
            default_reward_mods().into_iter().map(|e| e.id).collect();

        let mut result = Vec::new();
        for preset in default_reward_mods() {
            if preset.id == "DIMITUS" {
                continue;
            }
            match user.iter().find(|e| e.id == preset.id) {
                Some(existing) => result.push(existing.clone()),
                None => result.push(preset),
            }
        }
        // Preserve any custom user entries not present in the presets.
        for entry in user
            .iter()
            .filter(|e| e.id != "DIMITUS" && !default_ids.contains(&e.id))
        {
            result.push(entry.clone());
        }
        // Dimitus is always last; keep the user's customization if present.
        match user.iter().find(|e| e.id == "DIMITUS") {
            Some(existing) => result.push(existing.clone()),
            None => {
                if let Some(dimitus) = default_reward_mods()
                    .into_iter()
                    .find(|e| e.id == "DIMITUS")
                {
                    result.push(dimitus);
                }
            }
        }
        self.reward_mods = result;
    }
}

fn default_version() -> u32 {
    SETTINGS_VERSION
}

/// A UTC reset instant entered by the user as separate fields (UI-friendly for
/// DragValue inputs). `year == 0` means "unset". Time is interpreted as UTC.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UtcResetTime {
    #[serde(default)]
    pub year: i32,
    #[serde(default)]
    pub month: u32,
    #[serde(default)]
    pub day: u32,
    #[serde(default)]
    pub hour: u32,
    #[serde(default)]
    pub minute: u32,
}

impl UtcResetTime {
    /// Whether the user has entered a value.
    pub fn is_set(&self) -> bool {
        self.year > 0
    }
}

/// Shared season / battlepass schedule. Entered once per season by the user.
/// The season and battlepass historically end at the same instant, so a single
/// `reset` is shared; `battlepass_reset` is an optional override used only when
/// it `is_set()`.
///
/// The season `reset` can be auto-populated from live `getClientSeasons` data
/// (see `apply_live_season_end`). The battlepass end date has no live source, so
/// it is estimated from the season span (two equal battlepasses per season,
/// boundaries on Tuesdays; see `season::battlepass_target`). A hand-entered
/// `battlepass_reset` overrides the estimate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SeasonConfig {
    /// Display name of the current season (e.g. "Alien Invasion").
    #[serde(default)]
    pub season_name: String,
    /// Display name of the current battlepass (e.g. "The Astral Path").
    #[serde(default)]
    pub battlepass_name: String,
    /// Shared season + battlepass reset instant (UTC).
    #[serde(default)]
    pub reset: UtcResetTime,
    /// Optional battlepass-specific reset (UTC). Used only when `is_set()`;
    /// otherwise the shared `reset` applies to the battlepass too.
    #[serde(default)]
    pub battlepass_reset: UtcResetTime,
    /// Id of the season whose end date was last auto-applied from live
    /// `getClientSeasons` data. `0` means nothing has been auto-applied yet.
    #[serde(default)]
    pub auto_season_id: i32,
    /// The live `endDate` (unix seconds) last auto-applied to `reset`. Provenance
    /// used to avoid clobbering manual overrides on every fetch.
    #[serde(default)]
    pub auto_season_end_unix: i64,
    /// The season start (unix seconds) observed for `auto_season_id`, from the
    /// `getPlayerMissions` pool timestamp. Anchors the battlepass-end estimate.
    #[serde(default)]
    pub auto_season_start_unix: i64,
}

impl SeasonConfig {
    /// Auto-populate the season `reset` (and battlepass start anchor) from live
    /// season data.
    ///
    /// Keyed on the season id so a genuine season rollover refreshes the date
    /// while repeated fetches (or mid-season server corrections) leave a manual
    /// override untouched. On the very first observation an existing manual date
    /// is preserved; only the provenance is recorded. `start_unix` is the season
    /// start (pool timestamp) used to estimate the battlepass end. Returns `true`
    /// when anything changed and the settings should be saved.
    pub fn apply_live_season_end(
        &mut self,
        season_id: i32,
        end_unix: i64,
        start_unix: Option<i64>,
    ) -> bool {
        if end_unix <= 0 {
            return false;
        }
        // Already handled this season: keep whatever value is there (manual or
        // auto) and ignore later corrections to the same season's timestamp.
        if season_id == self.auto_season_id {
            return false;
        }
        let first_ever = self.auto_season_id == 0 && self.auto_season_end_unix == 0;
        self.auto_season_id = season_id;
        self.auto_season_end_unix = end_unix;
        if let Some(start) = start_unix {
            if start > 0 {
                self.auto_season_start_unix = start;
            }
        }
        // On first launch after upgrade, don't overwrite a date the user already
        // entered by hand; just record provenance so future rollovers apply.
        if first_ever && self.reset.is_set() {
            return true;
        }
        if let Some(reset) = crate::season::unix_to_reset(end_unix) {
            self.reset = reset;
        }
        true
    }
}

/// The kinds of widget that can appear in the Widget Bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WidgetKind {
    /// Forge dust (existing behaviour).
    Dust,
    /// Forge fire energy (with a spend warning near cap).
    ForgeFire,
    /// Forge materials total.
    Materials,
    /// Current account fame.
    Fame,
    /// Account gold.
    Gold,
    /// Account rank (stars) with tier colour.
    Rank,
    /// Number of character slots.
    CharSlots,
    /// Number of skins owned.
    Skins,
    /// Cumulative active play time across all characters.
    Playtime,
    /// Season end countdown.
    SeasonTimer,
    /// Battlepass end countdown.
    BattlepassTimer,
    /// Current (live) character: icon + label, with an Attributes tooltip.
    CurrentCharacter,
}

/// Widget Bar configuration: the ordered set of widgets that show in the
/// account-info bar below the tabs. Defaults to all widgets enabled so they are
/// discoverable on first launch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WidgetBarSettings {
    /// Widgets shown on the first (top) row, in order.
    #[serde(default = "default_widgets")]
    pub widgets: Vec<WidgetKind>,
    /// Widgets the user has explicitly moved to the second (bottom) row, in
    /// order. Empty by default so existing configs keep every widget on row 1.
    #[serde(default)]
    pub widgets_row2: Vec<WidgetKind>,
}

fn default_widgets() -> Vec<WidgetKind> {
    // Default to every widget enabled so new (and upgrading) users can see the
    // full Widget Bar and discover what's available, then trim to taste.
    WidgetBarSettings::ALL.to_vec()
}

impl Default for WidgetBarSettings {
    fn default() -> Self {
        Self {
            widgets: default_widgets(),
            widgets_row2: Vec::new(),
        }
    }
}

impl WidgetBarSettings {
    /// All widget kinds, in canonical display order, for building the settings UI.
    pub const ALL: [WidgetKind; 12] = [
        WidgetKind::CurrentCharacter,
        WidgetKind::Dust,
        WidgetKind::ForgeFire,
        WidgetKind::Materials,
        WidgetKind::Fame,
        WidgetKind::Gold,
        WidgetKind::Rank,
        WidgetKind::CharSlots,
        WidgetKind::Skins,
        WidgetKind::Playtime,
        WidgetKind::SeasonTimer,
        WidgetKind::BattlepassTimer,
    ];

    /// Whether a widget kind is currently enabled (on either row).
    pub fn is_enabled(&self, kind: WidgetKind) -> bool {
        self.widgets.contains(&kind) || self.widgets_row2.contains(&kind)
    }

    /// Enable or disable a widget kind. On enable the widget is appended to the
    /// end of row 1 so any custom drag-reorder the user has applied is preserved.
    /// On disable it is removed from whichever row it lives on.
    pub fn set_enabled(&mut self, kind: WidgetKind, enabled: bool) {
        if enabled {
            if !self.is_enabled(kind) {
                self.widgets.push(kind);
            }
        } else {
            self.widgets.retain(|k| *k != kind);
            self.widgets_row2.retain(|k| *k != kind);
        }
    }
}

/// End-of-cycle pinned warning toggles. Shown fixed at the top of
/// the Live Feed on the last day before each reset.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SeasonWarningSettings {
    /// Daily login calendar warning (fully automatic: last day of the month).
    #[serde(default = "default_true")]
    pub daily_calendar_enabled: bool,
    /// Battlepass end warning (uses the shared season config end date).
    #[serde(default = "default_true")]
    pub battlepass_enabled: bool,
    /// Season end warning (uses the shared season config end date).
    #[serde(default = "default_true")]
    pub season_enabled: bool,
}

impl Default for SeasonWarningSettings {
    fn default() -> Self {
        Self {
            daily_calendar_enabled: true,
            battlepass_enabled: true,
            season_enabled: true,
        }
    }
}

/// Window position and size settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowSettings {
    /// Window width in logical pixels.
    pub width: f32,

    /// Window height in logical pixels.
    pub height: f32,

    /// Window X position (screen coordinates).
    pub x: Option<f32>,

    /// Window Y position (screen coordinates).
    pub y: Option<f32>,

    /// Whether the window is maximized.
    pub maximized: bool,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            width: 1280.0,
            height: 720.0,
            x: None, // None means centered
            y: None,
            maximized: false,
        }
    }
}

/// Loot panel filter settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LootSettings {
    /// Minimum bag tier to display (0 = show all).
    pub min_bag_tier: u8,

    /// Visibility state for each bag type.
    /// Key is bag type (0-9): brown, pink, purple, teal, blue, gold, orange, white, red, egg.
    pub visible_bags: HashMap<u8, bool>,
}

impl Default for LootSettings {
    fn default() -> Self {
        let mut visible_bags = HashMap::new();
        // All bag tiers visible by default (0-9: brown, pink, purple, teal, blue, gold, orange, white, red, egg)
        for tier in 0..=9 {
            visible_bags.insert(tier, true);
        }

        Self {
            min_bag_tier: 0,
            visible_bags,
        }
    }
}

/// Party panel view mode: how much per-member detail is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartyViewMode {
    /// Hides guild name, Realmeye link, and Realmscope link.
    Simplified,
    Advanced,
}

impl Default for PartyViewMode {
    fn default() -> Self {
        PartyViewMode::Advanced
    }
}

/// Party panel member list layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartyListLayout {
    /// Always a single scrolling column (default).
    SingleColumn,
    /// Single column, but automatically splits into two side-by-side
    /// columns (no scrolling) when the full list doesn't fit the visible
    /// height.
    AutoTwoColumn,
}

impl Default for PartyListLayout {
    fn default() -> Self {
        PartyListLayout::SingleColumn
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartySettings {
    #[serde(default)]
    pub mod_tools: bool,

    #[serde(default)]
    pub view_mode: PartyViewMode,

    #[serde(default)]
    pub list_layout: PartyListLayout,
}

impl Default for PartySettings {
    fn default() -> Self {
        Self {
            mod_tools: false,
            view_mode: PartyViewMode::default(),
            list_layout: PartyListLayout::default(),
        }
    }
}

/// Combat History tracking / filter settings: which boss groups are recorded to
/// the combat history database, and whether solo fights are kept. Each toggle
/// gates recording at capture time (see `CombatManager::persist`) and also seeds
/// the default group filter on the Combat History page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CombatHistorySettings {
    /// Bosses inside 7+ grave-difficulty dungeons.
    #[serde(default = "default_true")]
    pub track_exaltation: bool,
    /// Bosses inside 5-6.5 grave-difficulty dungeons.
    #[serde(default = "default_true")]
    pub track_expert: bool,
    /// Bosses inside 2.5-4.5 grave-difficulty dungeons.
    #[serde(default = "default_true")]
    pub track_adept: bool,
    /// Bosses inside 0-2 grave-difficulty dungeons.
    #[serde(default = "default_true")]
    pub track_beginner: bool,
    /// Bosses carrying the BEACON_GUARDIAN label (Heroes of Oryx beacons).
    #[serde(default = "default_true")]
    pub track_beacon_guardians: bool,
    /// Realm Rookie Heroes of Oryx (curated realm boss list).
    #[serde(default = "default_true")]
    pub track_rookie_heroes: bool,
    /// Realm Adept Heroes of Oryx (curated realm boss list).
    #[serde(default = "default_true")]
    pub track_adept_heroes: bool,
    /// Realm Veteran Heroes of Oryx (curated realm boss list).
    #[serde(default = "default_true")]
    pub track_veteran_heroes: bool,
    /// Realm Adept Encounters (ADEPT_ENCOUNTER label).
    #[serde(default = "default_true")]
    pub track_adept_encounters: bool,
    /// Realm Veteran Encounters (VETERAN_ENCOUNTER label).
    #[serde(default = "default_true")]
    pub track_veteran_encounters: bool,
    /// Seasonal event bosses (Keyper, Gardener, Appetizer, Biff the Buffed
    /// Bunny, Snowy the Frost God, Jack Frost, Permafrost Lord) plus curated
    /// special bosses.
    #[serde(default = "default_true")]
    pub track_seasonal_encounters: bool,
    /// Lootable treasure crates / chests.
    #[serde(default = "default_true")]
    pub track_treasure_crates: bool,
    /// Whether to record fights where the local player is the only participant.
    #[serde(default = "default_true")]
    pub track_solo: bool,
}

impl Default for CombatHistorySettings {
    fn default() -> Self {
        Self {
            track_exaltation: true,
            track_expert: true,
            track_adept: true,
            track_beginner: true,
            track_beacon_guardians: true,
            track_rookie_heroes: true,
            track_adept_heroes: true,
            track_veteran_heroes: true,
            track_adept_encounters: true,
            track_veteran_encounters: true,
            track_seasonal_encounters: true,
            track_treasure_crates: true,
            track_solo: true,
        }
    }
}

impl CombatHistorySettings {
    /// Whether the toggle for `group` is enabled.
    pub fn tracks(&self, group: crate::assets::BossGroup) -> bool {
        use crate::assets::BossGroup;
        match group {
            BossGroup::Exaltation => self.track_exaltation,
            BossGroup::Expert => self.track_expert,
            BossGroup::Adept => self.track_adept,
            BossGroup::Beginner => self.track_beginner,
            BossGroup::BeaconGuardian => self.track_beacon_guardians,
            BossGroup::RookieHero => self.track_rookie_heroes,
            BossGroup::AdeptHero => self.track_adept_heroes,
            BossGroup::VeteranHero => self.track_veteran_heroes,
            BossGroup::AdeptEncounter => self.track_adept_encounters,
            BossGroup::VeteranEncounter => self.track_veteran_encounters,
            BossGroup::SeasonalEncounter => self.track_seasonal_encounters,
            BossGroup::TreasureCrate => self.track_treasure_crates,
        }
    }

    /// Mutable access to the toggle for `group` (used by the settings UI).
    pub fn track_mut(&mut self, group: crate::assets::BossGroup) -> &mut bool {
        use crate::assets::BossGroup;
        match group {
            BossGroup::Exaltation => &mut self.track_exaltation,
            BossGroup::Expert => &mut self.track_expert,
            BossGroup::Adept => &mut self.track_adept,
            BossGroup::Beginner => &mut self.track_beginner,
            BossGroup::BeaconGuardian => &mut self.track_beacon_guardians,
            BossGroup::RookieHero => &mut self.track_rookie_heroes,
            BossGroup::AdeptHero => &mut self.track_adept_heroes,
            BossGroup::VeteranHero => &mut self.track_veteran_heroes,
            BossGroup::AdeptEncounter => &mut self.track_adept_encounters,
            BossGroup::VeteranEncounter => &mut self.track_veteran_encounters,
            BossGroup::SeasonalEncounter => &mut self.track_seasonal_encounters,
            BossGroup::TreasureCrate => &mut self.track_treasure_crates,
        }
    }
}

/// Chat panel filter settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSettings {
    /// Show public/normal chat messages.
    pub show_public: bool,

    /// Show whisper messages.
    pub show_whisper: bool,

    /// Show guild messages.
    pub show_guild: bool,

    /// Show party messages.
    pub show_party: bool,
}

impl Default for ChatSettings {
    fn default() -> Self {
        Self {
            show_public: true,
            show_whisper: true,
            show_guild: true,
            show_party: true,
        }
    }
}

/// Loot tracking/capture settings (what gets stored in DB).
///
/// These settings control which items are captured to the database,
/// separate from the display filters in LootSettings.
///
/// A bag is tracked if its bag type is enabled (Section 1) OR if it
/// contains at least one item matching an enabled content filter (Section 2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LootTrackingSettings {
    // -- Section 1: Bag types to always track (regardless of contents) --
    /// Always track White bags.
    #[serde(default = "default_true")]
    pub track_white: bool,

    /// Always track Orange bags.
    #[serde(default = "default_true")]
    pub track_orange: bool,

    /// Always track Red bags.
    #[serde(default = "default_true")]
    pub track_red: bool,

    /// Always track Gold bags.
    #[serde(default = "default_true")]
    pub track_gold: bool,

    /// Always track Blue bags.
    #[serde(default = "default_true")]
    pub track_blue: bool,

    /// Always track Egg bags.
    #[serde(default)]
    pub track_egg: bool,

    /// Always track Teal bags.
    #[serde(default)]
    pub track_teal: bool,

    /// Always track Purple bags.
    #[serde(default)]
    pub track_purple: bool,

    /// Always track Pink bags.
    #[serde(default)]
    pub track_pink: bool,

    /// Always track Soulbound bags.
    #[serde(default)]
    pub track_soulbound: bool,

    /// Always track Brown bags.
    #[serde(default)]
    pub track_brown: bool,

    // -- Section 2: Content filters (track any bag containing matching items) --
    /// Track bags containing shiny items.
    #[serde(default = "default_true")]
    pub track_shiny: bool,

    /// Track bags containing items with loot enchants (Lucky Streak, Loot Bonus III/IV).
    #[serde(default = "default_true")]
    pub track_loot_enchants: bool,

    /// Track bags containing items with unique enchants (Candy Coated, Sandstone Resilience, etc.).
    #[serde(default = "default_true")]
    pub track_unique_enchants: bool,

    /// Track bags containing items with awakened enchants.
    #[serde(default = "default_true")]
    pub track_awakened_enchants: bool,

    /// Track Legendary+ rarity items (Legendary and Divine).
    #[serde(default = "default_true")]
    pub track_legendary_plus: bool,

    /// Track UT (Untiered) items.
    #[serde(default = "default_true")]
    pub track_ut_items: bool,

    /// Track bags containing potions (stat potions, greater potions).
    #[serde(default = "default_true")]
    pub track_potions: bool,

    /// Track bags containing marks.
    #[serde(default = "default_true")]
    pub track_marks: bool,

    /// Track bags containing tiered items at or above this tier.
    #[serde(default = "default_true")]
    pub track_tiered_items: bool,

    /// Minimum tier threshold for tiered item tracking (0-14).
    #[serde(default = "default_min_tier")]
    pub min_tiered_tier: i32,

    // -- Legacy field (ignored on load, not written) --
    /// Legacy: replaced by track_loot_enchants + track_unique_enchants + track_awakened_enchants.
    #[serde(default = "default_true", skip_serializing)]
    pub track_special_enchants: bool,
}

fn default_true() -> bool {
    true
}

fn default_alien_adept_prefix() -> String {
    "adept wave".to_string()
}

fn default_alien_veteran_prefix() -> String {
    "veteran wave".to_string()
}

fn default_min_tier() -> i32 {
    10
}

impl Default for LootTrackingSettings {
    fn default() -> Self {
        Self {
            // Bag types
            track_white: true,
            track_orange: true,
            track_red: true,
            track_gold: true,
            track_blue: true,
            track_egg: false,
            track_teal: false,
            track_purple: false,
            track_pink: false,
            track_soulbound: false,
            track_brown: false,
            // Content filters
            track_shiny: true,
            track_loot_enchants: true,
            track_unique_enchants: true,
            track_awakened_enchants: true,
            track_legendary_plus: true,
            track_ut_items: true,
            track_potions: true,
            track_marks: true,
            track_tiered_items: true,
            min_tiered_tier: 10,
            // Legacy
            track_special_enchants: true,
        }
    }
}

impl LootTrackingSettings {
    /// Migrate legacy fields and normalize values after deserialization.
    ///
    /// If old settings had `track_special_enchants: false` but the new granular
    /// enchant fields are missing (defaulted to true), propagate the disabled
    /// state to all three enchant sub-settings.
    pub fn migrate_legacy(&mut self) {
        if !self.track_special_enchants {
            self.track_loot_enchants = false;
            self.track_unique_enchants = false;
            self.track_awakened_enchants = false;
        }
        self.min_tiered_tier = self.min_tiered_tier.clamp(0, 14);
    }
}

/// Main account settings for multi-client connection isolation.
///
/// Stores the user's main account identity so that secondary game clients
/// (mules, alts) are silently ignored.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AccountSettings {
    /// The player's in-game name (from StatType::Name, stat 31).
    #[serde(default)]
    pub account_name: Option<String>,

    /// The player's stable account ID (from StatType::AccountId, stat 38).
    #[serde(default)]
    pub account_id: Option<String>,

    /// Unix seconds of the last time RealmHound observed the game client
    /// contacting the RotMG server (a HELLO/token capture or a live character).
    /// Used to gate account API refreshes so RealmHound never makes the first
    /// server contact of a UTC day on the user's behalf. 0 when never seen.
    #[serde(default)]
    pub last_client_seen_unix: i64,

    /// Unix seconds of the creation time of the main game-client process, as
    /// last confirmed by RealmHound. The daily forge-fire grant only lands on a
    /// fresh client relaunch after 00:00 UTC, so account API refreshes are
    /// gated on this process having started on the current UTC day (a
    /// mid-session area change is not enough). 0 when never confirmed.
    #[serde(default)]
    pub last_client_launch_unix: i64,
}

impl AccountSettings {
    /// Check if a main account has been saved.
    pub fn is_set(&self) -> bool {
        self.account_id.is_some()
    }

    /// Clear the saved account info (used for account reset).
    pub fn clear(&mut self) {
        self.account_name = None;
        self.account_id = None;
        self.last_client_seen_unix = 0;
        self.last_client_launch_unix = 0;
    }
}

/// Trophy Hall tab settings.
///
/// Persists the tracked-loot data source so a RealmShark/Both selection
/// survives restarts instead of resetting to RealmHound.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrophyHallSettings {
    /// Data source key: "realmhound" (default), "realmshark", or "both".
    /// `None` uses the default (RealmHound).
    #[serde(default)]
    pub data_source: Option<String>,
    /// Whether to show dungeons whose collection is intentionally empty
    /// (duplicate drop tables). Off by default.
    #[serde(default)]
    pub show_no_collection_dungeons: bool,
    /// Whether the compact list view (hide name/section columns) is enabled.
    #[serde(default)]
    pub compact_view: bool,
}

/// Treasury tab settings.
///
/// Stores section display order as string keys for forward-compatibility.
/// Unknown keys are silently ignored on load; missing sections get appended.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreasurySettings {
    /// Ordered list of section keys (e.g. "seasonal_characters", "regular_vault").
    #[serde(default)]
    pub section_order: Vec<String>,

    /// Sort mode for the treasury header grid: "by_count", "by_category", or "custom".
    #[serde(default)]
    pub sort_mode: Option<String>,

    /// Custom category order as string keys (e.g. "weapons", "armors").
    /// Only used when sort_mode is "custom".
    #[serde(default)]
    pub custom_category_order: Vec<String>,
}

impl Default for TreasurySettings {
    fn default() -> Self {
        Self {
            section_order: Vec::new(),         // empty = use default order
            sort_mode: None,                   // None = use default (ByCount)
            custom_category_order: Vec::new(), // empty = use default order
        }
    }
}

/// Characters tab toolbar settings.
///
/// Persists the card sort mode, the "stats maxed" display mode, and which
/// inventory sections are shown on every character card. String keys are used
/// for forward-compatibility (unknown values fall back to defaults).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharactersSettings {
    /// Sort mode key: "custom" (default), "fame_desc", "fame_asc",
    /// "maxed_desc", "maxed_asc", "class", "id_asc", "id_desc".
    #[serde(default)]
    pub sort_mode: Option<String>,

    /// Stats-maxed display key: "none" (default), "current", "left",
    /// "current_left".
    #[serde(default)]
    pub stat_display: Option<String>,

    /// Show the main inventory section on cards.
    #[serde(default = "default_true")]
    pub show_inventory: bool,

    /// Show the backpack section on cards.
    #[serde(default = "default_true")]
    pub show_backpack: bool,

    /// Show the backpack-extender section on cards.
    #[serde(default = "default_true")]
    pub show_extender: bool,

    /// Show the potion-belt section on cards.
    #[serde(default = "default_true")]
    pub show_belt: bool,

    /// Last character id seen live, so the Current-Character widget can restore
    /// (dimmed) the last-seen character across app restarts.
    #[serde(default)]
    pub last_live_char_id: Option<i32>,

    /// Stats-page block flow order (indices into the 7 stats blocks). Restored
    /// only when the length matches the current block count.
    #[serde(default)]
    pub stats_block_order: Option<Vec<usize>>,

    /// Stats-page per-block column assignment. Restored only when the length
    /// matches the current block count.
    #[serde(default)]
    pub stats_block_column: Option<Vec<usize>>,

    /// Stats-page per-category visibility (the "Show categories" dropdown).
    /// Restored only when the length matches the current category count.
    #[serde(default)]
    pub stats_category_visible: Option<Vec<bool>>,

    /// Stats-page dungeon sort key: "default", "difficulty_asc",
    /// "difficulty_desc", "most_completed".
    #[serde(default)]
    pub stats_dungeon_sort: Option<String>,

    /// Stats-page "Show stats breakdown" toggle (Gear/Exalts/Crucible columns).
    #[serde(default)]
    pub stats_show_breakdown: bool,
}

impl Default for CharactersSettings {
    fn default() -> Self {
        Self {
            sort_mode: None,
            stat_display: None,
            show_inventory: true,
            show_backpack: true,
            show_extender: true,
            show_belt: true,
            last_live_char_id: None,
            stats_block_order: None,
            stats_block_column: None,
            stats_category_visible: None,
            stats_dungeon_sort: None,
            stats_show_breakdown: false,
        }
    }
}

/// Missions tab settings. Persists the user's hidden missions, manual drag
/// order, collapsed status sections, and the hide-claimed toggle so choices
/// survive app restarts. Ordering is keyed by `mission_id` (stable across
/// resets), so re-arranged cards keep their positions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionsSettings {
    /// Mission uids (`(tree_id << 32) | mission_id`) the user has hidden.
    #[serde(default)]
    pub hidden: Vec<i64>,

    /// Mission uids in user-defined (drag) order. Missions not listed here fall
    /// back to most-progressed order within their status section.
    #[serde(default)]
    pub user_order: Vec<i64>,

    /// Collapsed status-section ids (0 in-progress, 1 claimable, 2 cooldown,
    /// 3 claimed, 4 hidden).
    #[serde(default)]
    pub collapsed: Vec<u8>,

    /// Hide claimed non-repeatable missions.
    #[serde(default = "default_true")]
    pub hide_claimed: bool,

    /// Hide the mission icon and name columns (helpful on smaller screens).
    #[serde(default)]
    pub hide_name_icon: bool,
}

impl Default for MissionsSettings {
    fn default() -> Self {
        Self {
            hidden: Vec::new(),
            user_order: Vec::new(),
            collapsed: Vec::new(),
            hide_claimed: true,
            hide_name_icon: false,
        }
    }
}

/// Quests tab settings. Persists the sections the user has collapsed and the
/// sections toggled off, keyed by the stable per-section id, so choices survive
/// app restarts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QuestsSettings {
    /// Collapsed section ids (see `QuestSection` in the quest panel).
    #[serde(default)]
    pub collapsed: Vec<u8>,

    /// Section ids toggled off (hidden entirely).
    #[serde(default)]
    pub disabled: Vec<u8>,

    /// Quest ids the player has hidden (moved to the "Hidden by player" bucket).
    #[serde(default)]
    pub hidden: Vec<String>,

    /// Hide the quest-name column to save horizontal space.
    #[serde(default)]
    pub hide_names: bool,
}

/// Which dungeon difficulty tiers trigger a public key-pop notification.
/// Mirrors [`realmhound_core::assets::KeyPopTier`]. Defaults to the higher
/// tiers only (Expert and Exaltation) since low-tier pops are frequent and
/// rarely worth a notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyPopTierFilter {
    /// Grave difficulty <= 2.0.
    #[serde(default)]
    pub rookie: bool,
    /// Grave difficulty > 2.0 and <= 4.5.
    #[serde(default)]
    pub adept: bool,
    /// Grave difficulty > 4.5 and <= 6.5.
    #[serde(default = "default_true")]
    pub expert: bool,
    /// Grave difficulty > 6.5.
    #[serde(default = "default_true")]
    pub exaltation: bool,
}

impl Default for KeyPopTierFilter {
    fn default() -> Self {
        Self {
            rookie: false,
            adept: false,
            expert: true,
            exaltation: true,
        }
    }
}

impl KeyPopTierFilter {
    /// Whether the given tier is currently enabled. Dungeons with no known
    /// difficulty rating (`None`) are always allowed so unknown pops are never
    /// silently dropped.
    pub fn allows(&self, tier: Option<crate::assets::KeyPopTier>) -> bool {
        use crate::assets::KeyPopTier;
        match tier {
            Some(KeyPopTier::Rookie) => self.rookie,
            Some(KeyPopTier::Adept) => self.adept,
            Some(KeyPopTier::Expert) => self.expert,
            Some(KeyPopTier::Exaltation) => self.exaltation,
            None => true,
        }
    }
}

/// Sound notification settings.
///
/// Controls which sounds play and their volume.
/// Public default sound files are from RealmShark under the MIT license.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoundSettings {
    /// Master volume (0.0 to 1.0).
    #[serde(default = "default_volume")]
    pub volume: f32,

    /// Enable sound for white bag drops.
    #[serde(default = "default_true")]
    pub whitebag: bool,

    /// Enable sound for orange bag (UT) drops.
    #[serde(default)]
    pub orangebag: bool,

    /// Enable sound for red bag (ST) drops.
    #[serde(default)]
    pub redbag: bool,

    /// Enable sound for blue bag drops.
    #[serde(default)]
    pub bluebag: bool,

    /// Enable sound for gold bag drops.
    #[serde(default)]
    pub goldbag: bool,

    /// Enable sound for egg basket drops.
    #[serde(default)]
    pub eggbag: bool,

    /// Enable sound for key pops (not yet implemented).
    #[serde(default)]
    pub keypop: bool,

    /// Which dungeon difficulty tiers trigger a public key-pop notification
    /// (sound and Live Feed entry). All tiers enabled by default.
    #[serde(default)]
    pub keypop_tiers: KeyPopTierFilter,

    /// Enable sound for party events (not yet implemented).
    #[serde(default)]
    pub party: bool,

    /// Enable sound for guild messages (not yet implemented).
    #[serde(default)]
    pub guild: bool,

    /// Enable sound for private messages (not yet implemented).
    #[serde(default)]
    pub pm: bool,

    /// Enable sound for trade requests (not yet implemented).
    #[serde(default)]
    pub trade: bool,

    /// Play the Dimitus alert sound when entering a dungeon with the Dimitus
    /// modifier (golden outline). Off by default.
    #[serde(default)]
    pub dimitus_dungeon: bool,

    /// Play the bad-mod warning sound when entering a dungeon with a dangerous
    /// (red outline) modifier set. Off by default.
    #[serde(default)]
    pub bad_mod_warning: bool,

    /// Per-sound custom file overrides. Key is the sound type identifier
    /// (e.g. "whitebag", "dimitus_alert"), value is the filename stored in
    /// `%LOCALAPPDATA%\RealmHound\sounds\custom\`.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub custom_sounds: HashMap<String, String>,

    /// Original (human-readable) filename of each custom sound, keyed by the
    /// same identifier as `custom_sounds`. Used only for display; the on-disk
    /// file is named from the sanitized key (see `custom_sounds`). Absent keys
    /// fall back to the stored on-disk filename.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub custom_sound_names: HashMap<String, String>,

    /// Per-sound playback volume (0.0-1.0), keyed by the same identifier as
    /// `custom_sounds` (e.g. "whitebag", "dimitus_alert"). Scales the master
    /// volume for that sound. Absent keys play at full (1.0 x master).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub sound_volumes: HashMap<String, f32>,

    /// Realm-event notification sounds (Veteran/Adept/Seasonal encounters and
    /// the Alien Invasion). Each section is off by default.
    #[serde(default)]
    pub realm_events: RealmEventSounds,

    /// Enchantment notification sounds (tiered enchants and unique/awakened
    /// enchants) that play when a matching enchant appears in a loot bag. Both
    /// categories are off by default.
    #[serde(default)]
    pub enchantments: EnchantmentSounds,
}

fn default_event_volume() -> f32 {
    0.10
}

/// Default volume for enchantment notification categories (50%).
fn default_enchant_volume() -> f32 {
    0.50
}

/// Enchantment notification sounds. Two independent categories, each off by
/// default: tiered enchants (matched by family + minimum tier) and unique /
/// awakened enchants (matched by exact internal id).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnchantmentSounds {
    #[serde(default = "EnchantSection::default_tiered")]
    pub tiered: EnchantSection,
    #[serde(default = "EnchantSection::default_special")]
    pub special: EnchantSection,
}

impl Default for EnchantmentSounds {
    fn default() -> Self {
        Self {
            tiered: EnchantSection::default_tiered(),
            special: EnchantSection::default_special(),
        }
    }
}

/// One enchantment notification category. `entries` is the list of enchants
/// (or enchant families, for the tiered category) that notify; each plays the
/// category default sound unless it carries a custom sound.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnchantSection {
    /// Whether this category notifies at all. Off by default.
    #[serde(default)]
    pub enabled: bool,
    /// Volume (0.0-1.0) used for entries without an explicit override.
    #[serde(default = "default_enchant_volume")]
    pub default_volume: f32,
    /// Configured enchant entries.
    #[serde(default)]
    pub entries: Vec<EnchantEntry>,
}

impl EnchantSection {
    /// Tiered category default: seeded with a Loot Bonus (tier III and above)
    /// example, disabled.
    fn default_tiered() -> Self {
        Self {
            enabled: false,
            default_volume: default_enchant_volume(),
            entries: vec![EnchantEntry {
                key: "LOOT_BONUS".to_string(),
                name: "Loot Bonus".to_string(),
                tier: Some(3),
                volume: default_enchant_volume(),
                icon_id: 0,
            }],
        }
    }

    /// Unique/awakened category default: seeded with a Flurry of Blows example,
    /// disabled.
    fn default_special() -> Self {
        Self {
            enabled: false,
            default_volume: default_enchant_volume(),
            entries: vec![EnchantEntry {
                key: "Flurry of Blows".to_string(),
                name: "Flurry of Blows".to_string(),
                tier: None,
                volume: default_enchant_volume(),
                icon_id: 0,
            }],
        }
    }
}

impl Default for EnchantSection {
    fn default() -> Self {
        Self {
            enabled: false,
            default_volume: default_enchant_volume(),
            entries: Vec::new(),
        }
    }
}

/// One configured enchant notification entry. `key` is a stable internal
/// identity: a family key (e.g. `LOOT_BONUS`) for tiered entries, or an exact
/// internal id (e.g. `FLURRY_OF_BLOWS`) for unique/awakened entries. `tier` is
/// the minimum tier (1-4) for tiered entries and `None` for special entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnchantEntry {
    /// Stable match key (family key or internal id).
    pub key: String,
    /// Display name (cached for the settings list; matching uses `key`).
    #[serde(default)]
    pub name: String,
    /// Minimum tier (1-4) for tiered entries; `None` for special entries.
    #[serde(default)]
    pub tier: Option<u8>,
    /// Playback volume (0.0-1.0) for this entry.
    #[serde(default = "default_enchant_volume")]
    pub volume: f32,
    /// Representative enchant type id used only to render the row icon (not
    /// used for matching). Resolved lazily from assets in the settings UI.
    #[serde(default)]
    pub icon_id: u16,
}

/// Configuration for the realm-event notification sounds.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RealmEventSounds {
    #[serde(default)]
    pub veteran: EventSection,
    #[serde(default)]
    pub adept: EventSection,
    #[serde(default)]
    pub seasonal: EventSection,
    #[serde(default)]
    pub alien: AlienSection,
}

/// How a catalog-backed realm-event section notifies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventMode {
    /// No notifications for this category.
    #[default]
    None,
    /// Every encounter in the category notifies (default sound unless a boss
    /// has a custom sound/volume in `overrides`).
    All,
    /// Only the encounters listed in `overrides` notify (default sound unless
    /// that entry has a custom sound).
    Selected,
}

/// One catalog-backed realm-event section (Veteran/Adept/Seasonal). The `mode`
/// selects whether none, all, or only the listed encounters notify; listed
/// encounters play the section default sound at `default_volume` unless the
/// entry carries a custom sound/volume.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSection {
    /// None / All / Selected. Defaults to None (silent).
    #[serde(default)]
    pub mode: EventMode,
    /// Volume (0.0-1.0) used for encounters without an explicit override.
    #[serde(default = "default_event_volume")]
    pub default_volume: f32,
    /// Per-boss entries: the selection whitelist in `Selected` mode, and
    /// optional customizations (custom volume and/or sound) in `All` mode.
    #[serde(default)]
    pub overrides: Vec<EventOverride>,
}

impl Default for EventSection {
    fn default() -> Self {
        Self {
            mode: EventMode::None,
            default_volume: default_event_volume(),
            overrides: Vec::new(),
        }
    }
}

/// A per-boss override within an [`EventSection`]. A custom sound file (if any)
/// is stored in `SoundSettings::custom_sounds` under the section's key scheme.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventOverride {
    /// Catalog boss object id this override applies to.
    pub boss_id: i32,
    /// Display name (cached for the settings list; matching uses `boss_id`).
    #[serde(default)]
    pub name: String,
    /// Playback volume (0.0-1.0) for this boss.
    #[serde(default = "default_event_volume")]
    pub volume: f32,
}

/// Alien Invasion section: three fixed rows (wave start / UFO / Calbrik), each
/// with its own volume and optional custom sound.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlienSection {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub wave: EventRow,
    #[serde(default)]
    pub ufo: EventRow,
    #[serde(default)]
    pub calbrik: EventRow,
}

impl Default for AlienSection {
    fn default() -> Self {
        Self {
            enabled: false,
            wave: EventRow::default(),
            ufo: EventRow::default(),
            calbrik: EventRow::default(),
        }
    }
}

/// A single fixed alien-invasion row: just a volume (custom sound lives in the
/// `custom_sounds` map under a fixed key).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRow {
    #[serde(default = "default_event_volume")]
    pub volume: f32,
}

impl Default for EventRow {
    fn default() -> Self {
        Self {
            volume: default_event_volume(),
        }
    }
}

fn default_volume() -> f32 {
    0.5
}

impl Default for SoundSettings {
    fn default() -> Self {
        Self {
            volume: default_volume(),
            // Only white bag enabled by default
            whitebag: true,
            orangebag: false,
            redbag: false,
            bluebag: false,
            goldbag: false,
            eggbag: false,
            // Social sounds disabled by default (not yet implemented)
            keypop: false,
            // All key-pop difficulty tiers notify by default
            keypop_tiers: KeyPopTierFilter::default(),
            party: false,
            guild: false,
            pm: false,
            trade: false,
            // Dimitus dungeon alert disabled by default
            dimitus_dungeon: false,
            // Bad-mod warning disabled by default
            bad_mod_warning: false,
            // No custom sounds by default
            custom_sounds: HashMap::new(),
            custom_sound_names: HashMap::new(),
            // No per-sound volume overrides by default (all play at 1.0 x master)
            sound_volumes: HashMap::new(),
            // Realm-event notifications all off by default
            realm_events: RealmEventSounds::default(),
            // Enchantment notifications off by default (seeded examples)
            enchantments: EnchantmentSounds::default(),
        }
    }
}

impl SoundSettings {
    /// Per-sound playback volume (0.0-1.0) for a sound key, defaulting to full
    /// (1.0) when unset. Scales the master volume for that sound.
    pub fn sound_volume(&self, key: &str) -> f32 {
        self.sound_volumes
            .get(key)
            .copied()
            .unwrap_or(1.0)
            .clamp(0.0, 1.0)
    }

    /// Set the per-sound playback volume for a sound key.
    pub fn set_sound_volume(&mut self, key: &str, volume: f32) {
        self.sound_volumes
            .insert(key.to_string(), volume.clamp(0.0, 1.0));
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            window: WindowSettings::default(),
            loot: LootSettings::default(),
            loot_tracking: LootTrackingSettings::default(),
            chat: ChatSettings::default(),
            assets_stamp: None,
            account: AccountSettings::default(),
            treasury: TreasurySettings::default(),
            characters: CharactersSettings::default(),
            missions: MissionsSettings::default(),
            quests: QuestsSettings::default(),
            sound: SoundSettings::default(),
            live_feed: LiveFeedSettings::default(),
            appearance: AppearanceSettings::default(),
            party: PartySettings::default(),
            combat_history: CombatHistorySettings::default(),
            season: SeasonConfig::default(),
            widget_bar: WidgetBarSettings::default(),
            taskbar: TaskbarSettings::default(),
            trophy_hall: TrophyHallSettings::default(),
        }
    }
}

impl Settings {
    /// Get the path to the settings file.
    pub fn settings_path() -> Option<PathBuf> {
        dirs::data_local_dir().map(|d| d.join("RealmHound").join("settings.json"))
    }

    /// Load settings from disk.
    ///
    /// Returns default settings if:
    /// - The settings file doesn't exist
    /// - The file cannot be read
    /// - The JSON is invalid/corrupted
    pub fn load() -> Self {
        let Some(path) = Self::settings_path() else {
            tracing::warn!("[SETTINGS] Could not determine settings path, using defaults");
            return Self::default();
        };

        if !path.exists() {
            tracing::debug!("[SETTINGS] No settings file found, using defaults");
            return Self::default();
        }

        match std::fs::read_to_string(&path) {
            Ok(contents) => match Self::from_json(&contents) {
                Ok(settings) => {
                    tracing::info!("[SETTINGS] Loaded settings from {:?}", path);
                    settings
                }
                Err(e) => {
                    tracing::warn!(
                        "[SETTINGS] Failed to parse settings file, backing up and using defaults: {}",
                        e
                    );
                    back_up_corrupt_file(&path);
                    Self::default()
                }
            },
            Err(e) => {
                tracing::warn!(
                    "[SETTINGS] Failed to read settings file, using defaults: {}",
                    e
                );
                Self::default()
            }
        }
    }

    /// Load settings from an explicit file path without ever relocating a
    /// corrupt document.
    ///
    /// Unlike [`Self::load`], this resolves no OS directory and never renames or
    /// replaces a corrupt file: a missing file yields defaults, a present but
    /// malformed file is an error left in place, and a valid file is migrated
    /// forward. Startup uses this so a corrupt document surfaces recovery and
    /// survives retries instead of being backed up before migration reads it.
    pub fn load_explicit(path: &Path) -> Result<Self, SettingsLoadError> {
        match std::fs::read_to_string(path) {
            Ok(contents) => Self::from_json(&contents).map_err(SettingsLoadError::Parse),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(SettingsLoadError::Read(e)),
        }
    }

    /// Parse a settings document and apply forward migrations. Shared by
    /// [`Self::load`] and [`Self::load_explicit`]; neither relocates the source.
    fn from_json(contents: &str) -> Result<Self, serde_json::Error> {
        let mut settings: Settings = serde_json::from_str(contents)?;
        settings.apply_load_migrations();
        Ok(settings)
    }

    /// Apply forward version migrations to a freshly parsed document.
    fn apply_load_migrations(&mut self) {
        self.loot_tracking.migrate_legacy();
        if self.version < 2 {
            self.live_feed.migrate_v2();
        }
        if self.version < 3 {
            self.live_feed.reconcile_reward_mods();
        }
        if self.version < 4 {
            // The key-pop difficulty filter used to gate both the sound and the
            // Live Feed entry. Carry the old shared filter onto the new Live
            // Feed-only copy so existing users keep their prior visibility.
            self.live_feed.key_pop_tiers = self.sound.keypop_tiers.clone();
        }
        if self.version < 5 {
            // Missions/quests the user has hidden should not appear in the Live
            // Feed Taskbar: turn their compass tracking off (without overriding
            // any explicit prior choice).
            for id in &self.missions.hidden {
                self.live_feed
                    .taskbar
                    .mission_overrides
                    .entry(*id)
                    .or_insert(false);
            }
            for id in &self.quests.hidden {
                self.live_feed
                    .taskbar
                    .quest_overrides
                    .entry(id.clone())
                    .or_insert(false);
            }
        }
        if self.version < 6 {
            // The Taskbar became a global element shown on every tab, so its
            // settings move out of Live Feed to the top level.
            self.taskbar = self.live_feed.taskbar.clone();
        }
        if self.version < 7 {
            // The mission/quest tracking toggles became master switches: on
            // shows every pill, off hides them all. Reset both to on and drop
            // the stale per-card overrides so every compass reads on.
            self.taskbar.track_missions = true;
            self.taskbar.track_quests = true;
            self.taskbar.mission_overrides.clear();
            self.taskbar.quest_overrides.clear();
        }
        self.version = SETTINGS_VERSION;
    }

    /// Save settings to disk.
    ///
    /// Errors are logged but don't crash the application.
    pub fn save(&self) {
        if let Err(e) = self.save_result() {
            tracing::error!("[SETTINGS] Failed to save settings: {}", e);
        }
    }

    /// Save settings to disk, returning the error so a relaunch can abort on failure.
    pub fn save_result(&self) -> Result<(), SettingsSaveError> {
        let path = Self::settings_path().ok_or(SettingsSaveError::NoPath)?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(SettingsSaveError::Io)?;
        }

        let json = serde_json::to_string_pretty(self).map_err(SettingsSaveError::Serialize)?;
        std::fs::write(&path, json).map_err(SettingsSaveError::Io)?;
        tracing::debug!("[SETTINGS] Saved settings to {:?}", path);
        Ok(())
    }
}

/// Failure while persisting settings to disk.
#[derive(Debug, thiserror::Error)]
pub enum SettingsSaveError {
    /// The settings path could not be determined.
    #[error("could not determine settings path")]
    NoPath,
    /// Serializing settings to JSON failed.
    #[error("failed to serialize settings: {0}")]
    Serialize(#[source] serde_json::Error),
    /// Writing the settings file (or creating its directory) failed.
    #[error("failed to write settings file: {0}")]
    Io(#[source] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_live_season_end_populates_when_unset() {
        let mut c = SeasonConfig::default();
        assert!(c.apply_live_season_end(52, 1791277199, Some(1785761800)));
        assert_eq!(c.reset.year, 2026);
        assert_eq!(c.reset.month, 10);
        assert_eq!(c.reset.day, 6);
        assert_eq!(c.auto_season_id, 52);
        assert_eq!(c.auto_season_end_unix, 1791277199);
        assert_eq!(c.auto_season_start_unix, 1785761800);
    }

    #[test]
    fn apply_live_season_end_preserves_first_manual_value() {
        let mut c = SeasonConfig::default();
        c.reset = UtcResetTime {
            year: 2025,
            month: 1,
            day: 1,
            hour: 9,
            minute: 0,
        };
        // First-ever observation must not clobber a hand-entered date, but should
        // record provenance so a later rollover applies.
        assert!(c.apply_live_season_end(52, 1791277199, Some(1785761800)));
        assert_eq!(c.reset.year, 2025);
        assert_eq!(c.auto_season_id, 52);
        assert_eq!(c.auto_season_end_unix, 1791277199);
        assert_eq!(c.auto_season_start_unix, 1785761800);
    }

    #[test]
    fn apply_live_season_end_ignores_same_season_repeat_and_correction() {
        let mut c = SeasonConfig::default();
        assert!(c.apply_live_season_end(52, 1791277199, Some(1785761800)));
        // Same id again (even with a corrected timestamp) is a no-op.
        assert!(!c.apply_live_season_end(52, 1791277199, Some(1785761800)));
        assert!(!c.apply_live_season_end(52, 1791280000, Some(1785761800)));
        assert_eq!(c.auto_season_end_unix, 1791277199);
    }

    #[test]
    fn apply_live_season_end_applies_on_new_season() {
        let mut c = SeasonConfig::default();
        c.apply_live_season_end(52, 1791277199, Some(1785761800));
        // A genuine rollover (new id) refreshes the date even if one exists.
        assert!(c.apply_live_season_end(53, 1793000000, Some(1791277199)));
        assert_eq!(c.auto_season_id, 53);
        assert_eq!(c.auto_season_end_unix, 1793000000);
        assert_eq!(c.auto_season_start_unix, 1791277199);
    }

    #[test]
    fn apply_live_season_end_rejects_non_positive() {
        let mut c = SeasonConfig::default();
        assert!(!c.apply_live_season_end(52, 0, Some(1785761800)));
        assert!(!c.apply_live_season_end(52, -5, None));
        assert_eq!(c.auto_season_id, 0);
    }

    #[test]
    fn combat_history_tracks_every_boss_group_by_default() {
        // Each group defaults to true and track_mut/tracks stay wired (TreasureCrate added later).
        let mut s = CombatHistorySettings::default();
        for g in crate::assets::BossGroup::ALL {
            assert!(s.tracks(g), "{:?} should default to tracked", g);
            *s.track_mut(g) = false;
            assert!(!s.tracks(g), "{:?} toggle not reflected", g);
        }
        assert!(!s.track_treasure_crates);
    }

    #[test]
    fn realm_event_sounds_default_off_and_survive_missing_json() {
        // Defaults: every section off (None mode), per-event/section volumes at 10%.
        let d = SoundSettings::default();
        assert_eq!(d.realm_events.veteran.mode, EventMode::None);
        assert_eq!(d.realm_events.adept.mode, EventMode::None);
        assert_eq!(d.realm_events.seasonal.mode, EventMode::None);
        assert!(!d.realm_events.alien.enabled);
        assert_eq!(d.realm_events.veteran.default_volume, 0.10);
        assert_eq!(d.realm_events.alien.wave.volume, 0.10);

        // Back-compat: older configs without a `realm_events` block still load.
        let legacy = r#"{ "volume": 0.5, "whitebag": true }"#;
        let parsed: SoundSettings = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.realm_events.veteran.mode, EventMode::None);
        assert!(parsed.realm_events.veteran.overrides.is_empty());

        // Round-trip with overrides preserves the data.
        let mut s = SoundSettings::default();
        s.realm_events.veteran.mode = EventMode::Selected;
        s.realm_events.veteran.overrides.push(EventOverride {
            boss_id: 22146,
            name: "Lost Sentry".into(),
            volume: 0.8,
        });
        let json = serde_json::to_string(&s).unwrap();
        let back: SoundSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.realm_events.veteran.mode, EventMode::Selected);
        assert_eq!(back.realm_events.veteran.overrides.len(), 1);
        assert_eq!(back.realm_events.veteran.overrides[0].boss_id, 22146);
        assert_eq!(back.realm_events.veteran.overrides[0].volume, 0.8);
    }

    #[test]
    fn test_default_settings() {
        let settings = Settings::default();
        assert_eq!(settings.version, SETTINGS_VERSION);
        assert_eq!(settings.window.width, 1280.0);
        assert_eq!(settings.window.height, 720.0);
        assert!(!settings.window.maximized);
        assert_eq!(settings.loot.min_bag_tier, 0);
        assert!(settings.chat.show_public);
        assert!(settings.chat.show_whisper);
        assert!(settings.chat.show_guild);
        assert!(settings.chat.show_party);
    }

    #[test]
    fn test_loot_default_all_visible() {
        let loot = LootSettings::default();
        for tier in 0..=7 {
            assert!(loot.visible_bags.get(&tier).copied().unwrap_or(false));
        }
    }

    #[test]
    fn test_loot_tracking_defaults() {
        let tracking = LootTrackingSettings::default();
        // Bag types: high-tier ON, low-tier OFF
        assert!(tracking.track_white);
        assert!(tracking.track_orange);
        assert!(tracking.track_red);
        assert!(tracking.track_gold);
        assert!(tracking.track_blue);
        assert!(!tracking.track_egg);
        assert!(!tracking.track_teal);
        assert!(!tracking.track_purple);
        assert!(!tracking.track_pink);
        assert!(!tracking.track_soulbound);
        assert!(!tracking.track_brown);
        // Content filters: all ON
        assert!(tracking.track_shiny);
        assert!(tracking.track_loot_enchants);
        assert!(tracking.track_unique_enchants);
        assert!(tracking.track_awakened_enchants);
        assert!(tracking.track_legendary_plus);
        assert!(tracking.track_ut_items);
        assert!(tracking.track_potions);
        assert!(tracking.track_marks);
        assert!(tracking.track_tiered_items);
        assert_eq!(tracking.min_tiered_tier, 10);
    }

    #[test]
    fn migrate_v2_copies_legacy_callout_fields() {
        let mut lf = LiveFeedSettings {
            join_position: JoinPosition::Beginning,
            include_percent_sign: false,
            event_call_mode: EventCallMode::ShortNamePlusUpcoming,
            reward_label_style: RewardLabelStyle::Words,
            disable_dimitus_callout: true,
            ..LiveFeedSettings::default()
        };
        lf.migrate_v2();
        assert_eq!(lf.dungeon_join_position, JoinPosition::Beginning);
        assert!(!lf.callout_percent);
        assert!(lf.event_add_upcoming);
        assert_eq!(lf.loot_label, LootLabel::Loot);
        assert_eq!(lf.dust_label, DustLabel::Dust);
        let dimitus = lf.reward_mods.iter().find(|e| e.id == "DIMITUS").unwrap();
        assert!(
            !dimitus.enabled,
            "disabled Dimitus toggle migrates to the preset"
        );
    }

    #[test]
    fn migrate_v2_enables_dimitus_when_legacy_callout_was_on() {
        let mut lf = LiveFeedSettings {
            disable_dimitus_callout: false,
            ..LiveFeedSettings::default()
        };
        // Default seeds Dimitus OFF; migration re-enables it for users who kept
        // the legacy Dimitus callout on.
        assert!(
            !lf.reward_mods
                .iter()
                .find(|e| e.id == "DIMITUS")
                .unwrap()
                .enabled
        );
        lf.migrate_v2();
        assert!(
            lf.reward_mods
                .iter()
                .find(|e| e.id == "DIMITUS")
                .unwrap()
                .enabled
        );
    }

    #[test]
    fn v1_settings_file_migrates_reward_mods_and_percent() {
        // A minimal v1 file: no v2 callout fields present. Loading via serde
        // yields defaults; the load-path migration then copies legacy values.
        let old_json = r#"{
            "version": 1,
            "live_feed": { "include_percent_sign": true, "join_position": "End" }
        }"#;
        let mut loaded: Settings = serde_json::from_str(old_json).unwrap();
        assert_eq!(loaded.version, 1);
        // Simulate the load()-path migration.
        if loaded.version < 2 {
            loaded.live_feed.migrate_v2();
        }
        // Reward mods are seeded from defaults for a file that lacked them.
        assert!(!loaded.live_feed.reward_mods.is_empty());
        assert!(loaded
            .live_feed
            .reward_mods
            .iter()
            .any(|e| e.id == "GENEROUS" && e.short == "generous"));
        // Percent sign migrates onto the new callout_percent flag.
        assert!(loaded.live_feed.callout_percent);
    }

    #[test]
    fn v5_migration_disables_taskbar_tracking_for_hidden() {
        // Hidden missions/quests must have their Taskbar tracking turned off,
        // without clobbering an explicit prior choice.
        let mut s = Settings::default();
        s.version = 4;
        s.missions.hidden = vec![10, 20];
        s.quests.hidden = vec!["qA".to_string(), "qB".to_string()];
        // Pretend the user had explicitly re-enabled tracking on mission 20.
        s.live_feed.taskbar.mission_overrides.insert(20, true);

        // Simulate the load()-path v5 migration.
        if s.version < 5 {
            for id in &s.missions.hidden {
                s.live_feed
                    .taskbar
                    .mission_overrides
                    .entry(*id)
                    .or_insert(false);
            }
            for id in &s.quests.hidden {
                s.live_feed
                    .taskbar
                    .quest_overrides
                    .entry(id.clone())
                    .or_insert(false);
            }
        }

        assert!(!s.live_feed.taskbar.mission_tracked(10));
        assert!(
            s.live_feed.taskbar.mission_tracked(20),
            "explicit choice preserved"
        );
        assert!(!s.live_feed.taskbar.quest_tracked("qA"));
        assert!(!s.live_feed.taskbar.quest_tracked("qB"));
    }

    #[test]
    fn v6_migration_promotes_taskbar_to_top_level() {
        // A pre-v6 file kept Taskbar settings under live_feed; the v6 migration
        // lifts them to the top-level field the app now reads.
        let mut s = Settings::default();
        s.version = 5;
        s.live_feed.taskbar.show_all_choice_options = false;
        s.live_feed.taskbar.mission_overrides.insert(42, true);

        // Simulate the load()-path v6 migration.
        if s.version < 6 {
            s.taskbar = s.live_feed.taskbar.clone();
        }

        assert!(!s.taskbar.show_all_choice_options);
        assert!(s.taskbar.mission_tracked(42));
    }

    #[test]
    fn v7_migration_resets_taskbar_tracking_to_on() {
        // The tracking toggles became master switches; the v7 migration turns
        // both back on and drops the stale per-card overrides so every compass
        // reads on after the update.
        let mut s = Settings::default();
        s.version = 6;
        s.taskbar.track_missions = false;
        s.taskbar.track_quests = false;
        s.taskbar.mission_overrides.insert(7, false);
        s.taskbar.quest_overrides.insert("qX".to_string(), false);

        // Simulate the load()-path v7 migration.
        if s.version < 7 {
            s.taskbar.track_missions = true;
            s.taskbar.track_quests = true;
            s.taskbar.mission_overrides.clear();
            s.taskbar.quest_overrides.clear();
        }

        assert!(s.taskbar.track_missions);
        assert!(s.taskbar.track_quests);
        assert!(
            s.taskbar.mission_tracked(7),
            "override cleared, inherits on"
        );
        assert!(
            s.taskbar.quest_tracked("qX"),
            "override cleared, inherits on"
        );
    }

    #[test]
    fn reconcile_appends_missing_reward_mods_before_dimitus() {
        // Simulate a v2 user whose saved list predates the new unique mods and
        // customized Generous. Only Generous + Dimitus present.
        let mut lf = LiveFeedSettings::default();
        lf.reward_mods = vec![
            RewardModEntry {
                id: "GENEROUS".to_string(),
                name: "Generous".to_string(),
                short: "gen".to_string(),
                enabled: true,
            },
            RewardModEntry {
                id: "DIMITUS".to_string(),
                name: "Dimitus".to_string(),
                short: "dimitus".to_string(),
                enabled: true,
            },
        ];
        lf.reconcile_reward_mods();

        // New mods are added, existing customization preserved, Dimitus stays last.
        let gen = lf.reward_mods.iter().find(|e| e.id == "GENEROUS").unwrap();
        assert_eq!(gen.short, "gen", "existing customization must be preserved");
        assert!(lf
            .reward_mods
            .iter()
            .any(|e| e.id == "SYNDICATE" && e.short == "synd"));
        assert!(lf.reward_mods.iter().any(|e| e.id == "SHATTERSACCEL"));
        assert_eq!(lf.reward_mods.last().unwrap().id, "DIMITUS");

        // Canonical order: a migrated config matches a fresh one, so `synd`
        // precedes `generous` just like a default install.
        let synd_pos = lf
            .reward_mods
            .iter()
            .position(|e| e.id == "SYNDICATE")
            .unwrap();
        let gen_pos = lf
            .reward_mods
            .iter()
            .position(|e| e.id == "GENEROUS")
            .unwrap();
        assert!(
            synd_pos < gen_pos,
            "synd must precede generous after migration"
        );
        let default_ids: Vec<String> = default_reward_mods().into_iter().map(|e| e.id).collect();
        let got_ids: Vec<String> = lf.reward_mods.iter().map(|e| e.id.clone()).collect();
        assert_eq!(
            got_ids, default_ids,
            "migrated order must match fresh defaults"
        );

        // Idempotent: a second pass adds nothing.
        let len = lf.reward_mods.len();
        lf.reconcile_reward_mods();
        assert_eq!(lf.reward_mods.len(), len);
    }

    #[test]
    fn reconcile_preserves_custom_user_entries() {
        let mut lf = LiveFeedSettings::default();
        lf.reward_mods = vec![
            RewardModEntry {
                id: "MYCUSTOMMOD".to_string(),
                name: "My Custom".to_string(),
                short: "custom".to_string(),
                enabled: true,
            },
            RewardModEntry {
                id: "DIMITUS".to_string(),
                name: "Dimitus".to_string(),
                short: "dimitus".to_string(),
                enabled: false,
            },
        ];
        lf.reconcile_reward_mods();

        // Unknown user id survives, placed just before the trailing Dimitus.
        let custom_pos = lf
            .reward_mods
            .iter()
            .position(|e| e.id == "MYCUSTOMMOD")
            .unwrap();
        assert_eq!(custom_pos, lf.reward_mods.len() - 2);
        assert_eq!(lf.reward_mods.last().unwrap().id, "DIMITUS");
        assert!(
            !lf.reward_mods.last().unwrap().enabled,
            "Dimitus customization preserved"
        );
    }

    #[test]
    fn test_serialization_roundtrip() {
        let settings = Settings::default();
        let json = serde_json::to_string_pretty(&settings).unwrap();
        let loaded: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.version, settings.version);
        assert_eq!(loaded.window.width, settings.window.width);
        assert_eq!(
            loaded.loot_tracking.track_white,
            settings.loot_tracking.track_white
        );
        assert_eq!(
            loaded.loot_tracking.track_loot_enchants,
            settings.loot_tracking.track_loot_enchants
        );
        assert_eq!(
            loaded.loot_tracking.track_potions,
            settings.loot_tracking.track_potions
        );
        assert_eq!(
            loaded.loot_tracking.min_tiered_tier,
            settings.loot_tracking.min_tiered_tier
        );
    }

    #[test]
    fn test_backward_compatibility_missing_loot_tracking() {
        // Simulate old settings file without loot_tracking field
        let old_json = r#"{
            "version": 1,
            "window": { "width": 1280.0, "height": 720.0, "maximized": false },
            "loot": { "min_bag_tier": 0, "visible_bags": {} },
            "chat": { "show_public": true, "show_whisper": true, "show_guild": true, "show_party": true }
        }"#;

        let loaded: Settings = serde_json::from_str(old_json).unwrap();
        // Should get defaults for missing loot_tracking
        assert!(loaded.loot_tracking.track_white);
        assert!(loaded.loot_tracking.track_orange);
        assert!(!loaded.loot_tracking.track_brown);
        assert!(loaded.loot_tracking.track_loot_enchants);
        assert!(loaded.loot_tracking.track_unique_enchants);
        assert!(loaded.loot_tracking.track_awakened_enchants);
        assert!(loaded.loot_tracking.track_legendary_plus);
        assert!(loaded.loot_tracking.track_ut_items);
        assert!(loaded.loot_tracking.track_potions);
        assert!(loaded.loot_tracking.track_marks);
        assert!(loaded.loot_tracking.track_tiered_items);
        assert_eq!(loaded.loot_tracking.min_tiered_tier, 10);
        // Should get None for missing assets_stamp
        assert!(loaded.assets_stamp.is_none());
    }

    #[test]
    fn test_assets_stamp_roundtrip() {
        let mut settings = Settings::default();
        assert!(settings.assets_stamp.is_none());

        settings.assets_stamp = Some(1738800000);
        let json = serde_json::to_string_pretty(&settings).unwrap();
        let loaded: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.assets_stamp, Some(1738800000));
    }

    #[test]
    fn test_backward_compatibility_old_loot_tracking_format() {
        // Simulate old settings with only the legacy track_special_enchants field
        let old_json = r#"{
            "version": 1,
            "window": { "width": 1280.0, "height": 720.0, "maximized": false },
            "loot": { "min_bag_tier": 0, "visible_bags": {} },
            "loot_tracking": {
                "track_special_enchants": true,
                "track_legendary_plus": false,
                "track_ut_items": true
            },
            "chat": { "show_public": true, "show_whisper": true, "show_guild": true, "show_party": true }
        }"#;

        let loaded: Settings = serde_json::from_str(old_json).unwrap();
        // Old fields preserved
        assert!(!loaded.loot_tracking.track_legendary_plus);
        assert!(loaded.loot_tracking.track_ut_items);
        // New fields get defaults
        assert!(loaded.loot_tracking.track_white);
        assert!(!loaded.loot_tracking.track_brown);
        assert!(loaded.loot_tracking.track_loot_enchants);
        assert!(loaded.loot_tracking.track_unique_enchants);
        assert!(loaded.loot_tracking.track_awakened_enchants);
        assert!(loaded.loot_tracking.track_potions);
        assert!(loaded.loot_tracking.track_marks);
        assert!(loaded.loot_tracking.track_tiered_items);
        assert_eq!(loaded.loot_tracking.min_tiered_tier, 10);
    }

    #[test]
    fn test_loot_tracking_legacy_field_not_serialized() {
        // The legacy track_special_enchants field should not be written to new settings
        let settings = LootTrackingSettings::default();
        let json = serde_json::to_string(&settings).unwrap();
        assert!(!json.contains("track_special_enchants"));
    }

    #[test]
    fn test_loot_tracking_migrate_legacy_enchants_disabled() {
        // Old settings with track_special_enchants: false should disable all enchant sub-settings
        let old_json = r#"{
            "version": 1,
            "loot_tracking": {
                "track_special_enchants": false,
                "track_legendary_plus": true,
                "track_ut_items": true
            }
        }"#;

        let mut loaded: Settings = serde_json::from_str(old_json).unwrap();
        loaded.loot_tracking.migrate_legacy();
        assert!(!loaded.loot_tracking.track_loot_enchants);
        assert!(!loaded.loot_tracking.track_unique_enchants);
        assert!(!loaded.loot_tracking.track_awakened_enchants);
        // Other settings unaffected
        assert!(loaded.loot_tracking.track_legendary_plus);
        assert!(loaded.loot_tracking.track_ut_items);
    }

    #[test]
    fn test_loot_tracking_migrate_legacy_enchants_enabled() {
        // Old settings with track_special_enchants: true should not change new defaults
        let old_json = r#"{
            "version": 1,
            "loot_tracking": {
                "track_special_enchants": true,
                "track_legendary_plus": true,
                "track_ut_items": true
            }
        }"#;

        let mut loaded: Settings = serde_json::from_str(old_json).unwrap();
        loaded.loot_tracking.migrate_legacy();
        assert!(loaded.loot_tracking.track_loot_enchants);
        assert!(loaded.loot_tracking.track_unique_enchants);
        assert!(loaded.loot_tracking.track_awakened_enchants);
    }

    #[test]
    fn test_backward_compatibility_missing_live_feed() {
        // Simulate old settings file without the live_feed field.
        let old_json = r#"{
            "version": 1,
            "window": { "width": 1280.0, "height": 720.0, "maximized": false },
            "loot": { "min_bag_tier": 0, "visible_bags": {} },
            "chat": { "show_public": true, "show_whisper": true, "show_guild": true, "show_party": true }
        }"#;

        let loaded: Settings = serde_json::from_str(old_json).unwrap();
        // Should get defaults for missing live_feed (Dimitus callout enabled).
        assert!(!loaded.live_feed.disable_dimitus_callout);
        assert_eq!(
            loaded.live_feed.dungeon_name_style,
            crate::settings::DungeonNameStyle::Short
        );
        assert_eq!(
            loaded.live_feed.reward_label_style,
            crate::settings::RewardLabelStyle::Abbreviated
        );
        assert!(loaded.live_feed.include_percent_sign);
    }

    #[test]
    fn test_backward_compatibility_missing_account() {
        // Simulate old settings file without account field
        let old_json = r#"{
            "version": 1,
            "window": { "width": 1280.0, "height": 720.0, "maximized": false },
            "loot": { "min_bag_tier": 0, "visible_bags": {} },
            "chat": { "show_public": true, "show_whisper": true, "show_guild": true, "show_party": true }
        }"#;

        let loaded: Settings = serde_json::from_str(old_json).unwrap();
        // Should get defaults for missing account
        assert!(loaded.account.account_name.is_none());
        assert!(loaded.account.account_id.is_none());
        assert!(!loaded.account.is_set());
    }

    #[test]
    fn test_account_settings_roundtrip() {
        let mut settings = Settings::default();
        assert!(!settings.account.is_set());

        settings.account.account_name = Some("TestPlayer".to_string());
        settings.account.account_id = Some("ABC123".to_string());
        assert!(settings.account.is_set());

        let json = serde_json::to_string_pretty(&settings).unwrap();
        let loaded: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.account.account_name.as_deref(), Some("TestPlayer"));
        assert_eq!(loaded.account.account_id.as_deref(), Some("ABC123"));
    }

    #[test]
    fn test_account_settings_clear() {
        let mut account = AccountSettings {
            account_name: Some("TestPlayer".to_string()),
            account_id: Some("ABC123".to_string()),
            last_client_seen_unix: 0,
            last_client_launch_unix: 0,
        };
        assert!(account.is_set());

        account.clear();
        assert!(!account.is_set());
        assert!(account.account_name.is_none());
        assert!(account.account_id.is_none());
        assert_eq!(account.last_client_launch_unix, 0);
    }

    #[test]
    fn load_explicit_missing_file_yields_defaults() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        let settings = Settings::load_explicit(&path).unwrap();
        assert_eq!(settings.version, SETTINGS_VERSION);
    }

    #[test]
    fn load_explicit_applies_forward_migrations() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        // A legacy v1 document must be migrated to the current version.
        std::fs::write(&path, r#"{"version":1}"#).unwrap();
        let settings = Settings::load_explicit(&path).unwrap();
        assert_eq!(settings.version, SETTINGS_VERSION);
        assert!(settings.taskbar.track_missions);
        assert!(settings.taskbar.track_quests);
    }

    #[test]
    fn load_explicit_corrupt_file_is_error_and_left_in_place() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        let malformed = b"{ not json";
        std::fs::write(&path, malformed).unwrap();

        assert!(matches!(
            Settings::load_explicit(&path),
            Err(SettingsLoadError::Parse(_))
        ));
        // Non-destructive: the corrupt bytes are untouched and no sibling
        // backup is created, so a retry keeps failing against the original.
        assert_eq!(std::fs::read(&path).unwrap(), malformed);
        let siblings = std::fs::read_dir(temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.contains(".corrupt-"))
            });
        assert!(!siblings, "load_explicit must not relocate a corrupt file");
        assert!(matches!(
            Settings::load_explicit(&path),
            Err(SettingsLoadError::Parse(_))
        ));
    }
}

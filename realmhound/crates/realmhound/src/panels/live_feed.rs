//! Live Feed panel - unified activity stream combining loot drops, boss callouts, and party alerts.
//!
//! This panel consolidates the most important in-game events into a single, real-time feed.

use crate::ui_ext::HoverTooltipExt;
use std::collections::{HashMap, VecDeque};

use chrono::{DateTime, Local, Utc};
use eframe::egui::{self, Color32, RichText, ScrollArea, Ui};
use realmhound_core::{
    assets::get_dungeon_portal_map,
    dungeon_modifiers::{ModDanger, ModType, OutlineKind},
    dust::{DustAmounts, DustState, DustType},
    loot::{LootBagType, LootDropRecord, LootItemRecord, ProcessedLootDrop, ProcessedLootItem},
};

use crate::panels::chat::ChatType;
use crate::panels::loot_filters::{render_filter_bar, LootFilters, PropertyFilters};
use crate::panels::{AppAction, Panel, PanelContext};
use crate::rendering::emotes::{self, EmoteSegment, EMOTE_PADDING};
use crate::rendering::{EmbeddedIcon, ModTierIcon, SpriteRenderer};
use crate::shadcn_ui::Shadcn;
use crate::tab_icons::{TabIconSprite, TAB_ICON_SIZE};

/// Maximum number of entries to keep in the feed.
const MAX_FEED_ENTRIES: usize = 100;

/// Which hover tooltip a pinned warning row shows. The season row gets a rich
/// icon + colour-coded breakdown of the end-of-season storage transfer rules;
/// the others have none.
enum PinnedTooltip {
    None,
    SeasonTransfer,
}

/// Estimated dungeon portal join window. The portal spawn time isn't in any
/// packet we receive, so we count down from local entry; the true joinable
/// window is always somewhat shorter than this upper bound.
const DUNGEON_JOIN_WINDOW: std::time::Duration = std::time::Duration::from_secs(30);

/// Window during which a repeated boss callout for the same boss is suppressed.
/// Kept short: it exists only to collapse near-duplicate spawn packets (the same
/// `.new.` taunt arriving twice, or a straggler just after a realm-join replay).
/// Fast, repeatable encounters (e.g. Skull Shrine) legitimately respawn within a
/// minute, so a long window would swallow genuine new spawns -- each distinct
/// spawn must notify. The Mysterious Crystal's multi-line flavor text is handled
/// separately via its pinned row (see `update_crystal_pin`).
const BOSS_CALL_DEDUP_WINDOW: chrono::Duration = chrono::Duration::seconds(5);

/// Feature flag: Enable active encounters display at top of live feed.
/// Disabled pending user feedback - shows realm bosses/events as sprites.
const ENABLE_ACTIVE_ENCOUNTERS: bool = false;

/// A single entry in the live feed.
#[derive(Debug, Clone)]
pub enum FeedEntry {
    /// Loot drop entry (reuses ProcessedLootDrop from loot tracking)
    Loot(LootEntry),
    /// Boss callout entry (Oryx announcements)
    BossCall(BossCallEntry),
    /// Kick alert for banned player in party
    KickAlert(KickAlertEntry),
    /// Dungeon entered (name + modifiers)
    Dungeon(DungeonEntry),
    /// A forge-dust type just reached capacity.
    DustFull(DustFullEntry),
    /// Direct message (whisper) received.
    DirectMessage(DmEntry),
    /// `/who` roster capture.
    WhoRoster(WhoRosterEntry),
    /// Generic warning entry with colored outline.
    Warning(WarningEntry),
    /// Public dungeon key-pop ("Opened by <player>").
    KeyPop(KeyPopEntry),
    /// Special area unlock (Incantation / Rune / Vial pop).
    AreaUnlock(AreaUnlockEntry),
}

/// Loot drop entry data.
#[derive(Debug, Clone)]
pub struct LootEntry {
    /// Timestamp when the drop occurred
    pub timestamp: DateTime<Local>,
    /// Bag type
    pub bag_type: LootBagType,
    /// Character ID
    pub char_id: i32,
    /// Character name at drop time
    pub char_name: String,
    /// Character class ID
    pub class_id: i32,
    /// Character skin ID (0 = default)
    pub skin_id: i32,
    /// Clothing dye/cloth texture
    pub player_tex1: u32,
    /// Accessory dye/cloth texture
    pub player_tex2: u32,
    /// Player's exalt bonus
    pub player_exalt_bonus: i32,
    /// Whether player is seasonal
    pub player_is_seasonal: bool,
    /// Player's fame at drop time
    pub player_fame: i32,
    /// Dungeon where the drop occurred
    pub dungeon: String,
    /// Mob type ID that dropped the bag
    pub mob_type: i32,
    /// Mob name
    pub mob_name: String,
    /// Items in the bag
    pub items: Vec<ProcessedLootItem>,
}

impl LootEntry {
    /// Create a LootEntry from a ProcessedLootDrop.
    pub fn from_drop(drop: &ProcessedLootDrop) -> Self {
        Self {
            timestamp: chrono::DateTime::from_timestamp_millis(drop.timestamp)
                .map(|utc| utc.with_timezone(&chrono::Local))
                .unwrap_or_else(chrono::Local::now),
            bag_type: drop.bag_type,
            char_id: drop.player.char_id,
            char_name: drop.player.name.clone(),
            class_id: drop.player.class_id,
            skin_id: drop.player.skin_id,
            player_tex1: drop.player.tex1,
            player_tex2: drop.player.tex2,
            player_exalt_bonus: drop.player.exalt_bonus as i32,
            player_is_seasonal: drop.player.is_seasonal,
            player_fame: drop.player.fame,
            dungeon: drop.player.dungeon.clone(),
            mob_type: drop.mob_type,
            mob_name: drop.mob_name.clone(),
            items: drop.items.clone(),
        }
    }

    /// Build a `LootDropRecord` view for the shared loot-card renderer. The DB
    /// row ids are synthetic (0) because Live Feed entries are not persisted;
    /// they are unused by the read-only renderer.
    pub fn to_drop_record(&self) -> LootDropRecord {
        LootDropRecord {
            id: 0,
            timestamp: self.timestamp.timestamp_millis(),
            dungeon: self.dungeon.clone(),
            map_seed: 0,
            char_id: self.char_id,
            char_name: self.char_name.clone(),
            class_id: self.class_id,
            skin_id: self.skin_id,
            tex1: self.player_tex1,
            tex2: self.player_tex2,
            fame: self.player_fame,
            is_seasonal: self.player_is_seasonal,
            exalt_bonus: self.player_exalt_bonus,
            loot_drop_active: false,
            loot_tier_active: false,
            bag_type: self.bag_type.id(),
            mob_type: self.mob_type,
            mob_name: self.mob_name.clone(),
            items: self
                .items
                .iter()
                .map(|it| LootItemRecord {
                    id: 0,
                    drop_id: 0,
                    slot: it.slot,
                    item_id: it.item_id,
                    item_name: it.item_name.clone(),
                    enchant_count: it.enchant_ids.len() as i32,
                    enchant_ids: String::new(),
                    parsed_enchant_ids: it.enchant_ids.clone(),
                })
                .collect(),
        }
    }
}

/// Boss callout entry data.
#[derive(Debug, Clone)]
pub struct BossCallEntry {
    /// Timestamp when the callout occurred
    pub timestamp: DateTime<Local>,
    /// Boss object ID (for sprite rendering)
    pub boss_id: i32,
    /// Boss name / announcement text
    pub boss_name: String,
    /// Server name at time of callout (e.g., "EUWest")
    pub server_name: Option<String>,
    /// Realm name at time of callout (e.g., "Medusa")
    pub realm_name: Option<String>,
    /// True once this callout has been superseded (e.g. a later Alien Invasion
    /// wave spawned in the same realm). Expired entries fade out and are no
    /// longer copyable, like expired dungeons.
    pub expired: bool,
}

impl BossCallEntry {
    /// Create a new boss callout entry.
    pub fn new(
        boss_id: i32,
        boss_name: String,
        server_name: Option<String>,
        realm_name: Option<String>,
    ) -> Self {
        Self {
            timestamp: Local::now(),
            boss_id,
            boss_name,
            server_name,
            realm_name,
            expired: false,
        }
    }
}

/// Kick alert entry data.
#[derive(Debug, Clone)]
pub struct KickAlertEntry {
    /// Timestamp when the alert was triggered
    pub timestamp: DateTime<Local>,
    /// Name of the banned player
    pub player_name: String,
    /// Whether this is from a join request (vs already in party)
    pub is_join_request: bool,
}

impl KickAlertEntry {
    /// Create a new kick alert entry.
    pub fn new(player_name: String) -> Self {
        Self {
            timestamp: Local::now(),
            player_name,
            is_join_request: false,
        }
    }

    /// Create a kick alert for a join request from a banned player.
    pub fn join_request(player_name: String) -> Self {
        Self {
            timestamp: Local::now(),
            player_name,
            is_join_request: true,
        }
    }
}

/// Dust-full notification entry: a forge-dust type just reached capacity.
#[derive(Debug, Clone)]
pub struct DustFullEntry {
    /// Timestamp when the dust type filled up.
    pub timestamp: DateTime<Local>,
    /// Whether the account is seasonal (true) or regular (false).
    pub is_seasonal: bool,
    /// The dust type that filled up.
    pub dust_type: DustType,
}

impl DustFullEntry {
    /// Create a new dust-full entry.
    pub fn new(is_seasonal: bool, dust_type: DustType) -> Self {
        Self {
            timestamp: Local::now(),
            is_seasonal,
            dust_type,
        }
    }

    /// Account label for display ("Seasonal" or "Regular").
    pub fn account_label(&self) -> &'static str {
        if self.is_seasonal {
            "Seasonal"
        } else {
            "Regular"
        }
    }
}

/// Direct-message (whisper) entry data.
#[derive(Debug, Clone)]
pub struct DmEntry {
    /// Timestamp when the message was received.
    pub timestamp: DateTime<Local>,
    /// Sender name.
    pub sender: String,
    /// Message content.
    pub text: String,
}

impl DmEntry {
    /// Create a new DM entry.
    pub fn new(sender: String, text: String) -> Self {
        Self {
            timestamp: Local::now(),
            sender,
            text,
        }
    }
}

/// `/who` roster entry data.
#[derive(Debug, Clone)]
pub struct WhoRosterEntry {
    /// Timestamp when the roster was captured.
    pub timestamp: DateTime<Local>,
    /// Player names, sorted alphabetically (case-insensitive).
    pub names: Vec<String>,
}

impl WhoRosterEntry {
    /// Create a new roster entry from already-parsed names.
    pub fn new(names: Vec<String>) -> Self {
        Self {
            timestamp: Local::now(),
            names,
        }
    }
}

/// Public dungeon key-pop entry data. Unlike dungeon entries, key pops are
/// observed (not entered by the local player), so they stay clickable and never
/// expire.
#[derive(Debug, Clone)]
pub struct KeyPopEntry {
    /// Timestamp when the key was popped.
    pub timestamp: DateTime<Local>,
    /// Dungeon key object id for sprite rendering.
    pub key_id: i32,
    /// Resolved dungeon display name.
    pub dungeon_name: String,
    /// Name of the player who opened the dungeon.
    pub opener: String,
    /// Whether to offer a "Thanks ..." callout. False when the local player
    /// opened the key (thanking yourself makes no sense).
    pub thankable: bool,
}

impl KeyPopEntry {
    /// Create a new key-pop entry.
    pub fn new(key_id: i32, dungeon_name: String, opener: String, thankable: bool) -> Self {
        Self {
            timestamp: Local::now(),
            key_id,
            dungeon_name,
            opener,
            thankable,
        }
    }

    /// The "Thanks <opener> for the key" clipboard callout, or `None` when the
    /// local player opened the key.
    pub fn callout(&self) -> Option<String> {
        self.thankable
            .then(|| format!("Thanks {} for the key", self.opener))
    }
}

/// Special "area unlock" entry: an immediate single-item pop (Wine Cellar
/// Incantation or Vial of Pure Darkness) or the combined Lost Halls Rune
/// monuments shown on Oryx's Sanctuary entry. Renders each contribution's item
/// icon next to its popper and carries a ready-made clipboard callout.
#[derive(Debug, Clone)]
pub struct AreaUnlockEntry {
    /// Timestamp when the entry was created.
    pub timestamp: DateTime<Local>,
    /// Entry title (e.g. "Wine Cellar unlocked", "Sanctuary runes").
    pub title: String,
    /// One `(item_id, popper)` per contributing pop, in display order.
    pub contributions: Vec<(i32, String)>,
    /// Ready-made clipboard callout ("Thanks <name> for the <item>"). Empty when
    /// the local player was the (only) opener, so there is no one to thank.
    pub callout: String,
}

impl AreaUnlockEntry {
    /// Create a new area-unlock entry. These never expire so the thanks callout
    /// stays copyable indefinitely.
    pub fn new(title: String, contributions: Vec<(i32, String)>, callout: String) -> Self {
        Self {
            timestamp: Local::now(),
            title,
            contributions,
            callout,
        }
    }

    /// The clipboard callout text.
    pub fn callout(&self) -> String {
        self.callout.clone()
    }

    /// Whether this entry has a copyable thanks callout.
    pub fn has_callout(&self) -> bool {
        !self.callout.is_empty()
    }
}

/// Parse a `/who` reply into a sorted, de-duplicated player list.
///
/// The server text looks like `Players online (14): alice, bob, carol`. Returns
/// `None` for any text that is not a `/who` reply. A reply with an empty roster
/// (`Players online (0): `) yields `Some(vec![])`. Names are sorted
/// case-insensitively.
fn parse_who_roster(text: &str) -> Option<Vec<String>> {
    let trimmed = text.trim();
    if !trimmed.starts_with("Players online (") {
        return None;
    }
    let colon = trimmed.find("):")?;
    let list = trimmed[colon + 2..].trim();

    let mut names: Vec<String> = list
        .split(',')
        .map(|n| n.trim())
        .filter(|n| !n.is_empty())
        .map(|n| n.to_string())
        .collect();

    names.sort_by_key(|n| n.to_lowercase());
    names.dedup_by_key(|n| n.to_lowercase());
    Some(names)
}

/// Parse a `/server` reply into `(server_name, optional_realm_name)`.
///
/// The in-game `/server` command prints one line identifying the current server
/// and location, e.g. `EUWest NexusPortal.Meridian` inside a realm or
/// `EUWest Nexus` in the Nexus / Vault / hub. The location half mirrors the
/// `realm_name_field` from MapInfo, so a realm portal reads `NexusPortal.<Realm>`
/// and any hub reads `Nexus`. Returns `None` for any line that is not a
/// recognizable `/server` reply.
fn parse_server_command(text: &str) -> Option<(String, Option<String>)> {
    let trimmed = text.trim();
    // Exactly a "<Server> <Location>" pair split on the first whitespace.
    let (server, location) = trimmed.split_once(char::is_whitespace)?;
    let server = server.trim();
    let location = location.trim();
    // The server token is always a bare alphanumeric word (server names carry no
    // spaces or punctuation); reject anything else so ordinary chat that happens
    // to mention a realm can't be mistaken for a `/server` reply.
    if server.is_empty() || !server.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    if location == "Nexus" {
        Some((server.to_string(), None))
    } else if let Some(realm) = location.strip_prefix("NexusPortal.") {
        let realm = realm.trim();
        if realm.is_empty() {
            None
        } else {
            Some((server.to_string(), Some(realm.to_string())))
        }
    } else {
        None
    }
}

/// Recognize a `/server` reply from a full text packet, returning
/// `(server, optional_realm)`. Requires a system-message sender: server
/// announcements carry a negative object id (or empty sender) and no recipient,
/// whereas player whisper/party/guild chat has a recipient or `#`-prefixed name
/// and public chat has a non-negative object id. Gating this way means ordinary
/// chat that happens to read like `<Server> Nexus` can never spoof the header.
fn parse_server_command_packet(
    packet: &realmhound_core::protocol::TextPacket,
) -> Option<(String, Option<String>)> {
    let is_system = packet.recipient.is_empty() && (packet.object_id < 0 || packet.name.is_empty());
    if !is_system {
        return None;
    }
    parse_server_command(&packet.text)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningSeverity {
    Orange,
    Red,
}

#[derive(Debug, Clone)]
pub struct WarningEntry {
    pub timestamp: DateTime<Local>,
    pub text: String,
    pub severity: WarningSeverity,
    pub copyable: bool,
}

impl WarningEntry {
    pub fn new(text: String, severity: WarningSeverity, copyable: bool) -> Self {
        Self {
            timestamp: Local::now(),
            text,
            severity,
            copyable,
        }
    }
}

/// A single modifier's display data for the Live Feed second line:
/// its name, in-game type (icon), and danger category (text color).
#[derive(Debug, Clone)]
pub struct ModChip {
    /// Display name (e.g. "Weak Boss III").
    pub name: String,
    /// In-game mod type, drives the leading type icon.
    pub mod_type: ModType,
    /// Danger category, drives the name text color.
    pub danger: ModDanger,
    /// Whether this is the Dimitus modifier (shows its portrait, not a glyph).
    pub is_dimitus: bool,
    /// Tier-specific modifier icon, if this is one of the boss/minion or player
    /// mods. Drawn as the leading icon in place of the (absent) Normal type icon.
    pub modifier_icon: Option<ModTierIcon>,
}

/// Result of a DM entry interaction.
enum DmAction {
    /// No interaction.
    None,
    /// Left-click: copy `/t name` to clipboard.
    CopyReply(String),
    /// Right-click menu: navigate to Chat with this sender.
    ViewChat(String),
}

/// Dungeon-entered entry data.
#[derive(Debug, Clone)]
pub struct DungeonEntry {
    /// Timestamp when the dungeon was entered
    pub timestamp: DateTime<Local>,
    /// Monotonic expiry time for the join countdown (immune to wall-clock
    /// changes). Anchored to the observed portal spawn (or entry time as a
    /// fallback) plus [`DUNGEON_JOIN_WINDOW`]. Set to "now" to force-expire when
    /// the player leaves this map.
    pub expires_at: std::time::Instant,
    /// Display name of the dungeon
    pub dungeon_name: String,
    /// Portal object id for sprite rendering (None if unknown)
    pub portal_id: Option<i32>,
    /// Modifier display names (e.g. "Weak Boss III"); falls back to a prettified
    /// token for unknown modifiers. Retained for display and dedup signature.
    pub modifiers: Vec<String>,
    /// Per-modifier display chips (name + type + danger color) for the Live Feed
    /// second line. Parallel to [`modifiers`].
    pub mods: Vec<ModChip>,
    /// Outline / background fill category for the whole callout.
    pub outline: OutlineKind,
    /// Dungeon grade (e.g. "S", "A"), if present
    pub grade: Option<String>,
    /// Total loot bonus (%) summed across the modifiers (0 if none).
    pub loot_bonus: i32,
    /// Total dust bonus (%) summed across the modifiers (0 if none).
    pub dust_bonus: i32,
    /// Total XP bonus (%) summed across the modifiers (0 if none).
    pub xp_bonus: i32,
    /// Clipboard callout for this dungeon entry, e.g.
    /// `snake 15lb synd j`. `None` for non-callable dungeons (Crystal Cavern,
    /// Cultist Hideout, The Void) and for Dimitus dungeons only when the
    /// "Disable Dimitus clipboard callouts" setting is enabled. Dungeons without
    /// a curated nickname fall back to their lowercased full name.
    pub callout: Option<String>,
    /// Whether this dungeon carries the Dimitus modifier (gets its own icon).
    pub is_dimitus: bool,
    /// Raw modifier tokens retained for callout recomputation when settings change.
    pub modifier_tokens: Vec<String>,
    /// Server name at time of dungeon entry (for callout assembly).
    pub server_name: Option<String>,
    /// Realm name at time of dungeon entry (for callout assembly).
    pub realm_name: Option<String>,
    /// True when the join window is estimated rather than anchored to an observed
    /// portal spawn (e.g. joined via a party call). Estimated entries use a
    /// shorter window and mark the countdown with a `?`.
    pub estimated: bool,
    /// Map seed (fp) of the instance, used to match the CombatManager's per-run
    /// timer freeze to this row.
    pub map_seed: i32,
    /// Monotonic entry time, driving the live "time in dungeon" run timer.
    pub entered_at: std::time::Instant,
    /// Committed per-run elapsed (ms) once the run has ended; `None` while the
    /// run is still in progress and the timer ticks live.
    pub frozen_elapsed_ms: Option<i64>,
}

/// Join window for entries with an observed portal spawn anchor.
const DUNGEON_WINDOW_ANCHORED: std::time::Duration = DUNGEON_JOIN_WINDOW;
/// Shorter join window for estimated (party-call) entries: the portal spawned
/// before we joined, so trim 5s to better reflect the real joinable time.
const DUNGEON_WINDOW_ESTIMATED: std::time::Duration = std::time::Duration::from_secs(25);

/// Time from a realm-close message to the castle teleport.
const CASTLE_TELEPORT_WINDOW: std::time::Duration = std::time::Duration::from_secs(120);

impl DungeonEntry {
    /// Create a new dungeon entry from a display name and decoded modifier tokens.
    ///
    /// Modifier display names and reward bonuses are resolved from locally
    /// extracted game assets; unknown tokens are prettified and contribute 0.
    pub fn new(
        dungeon_name: String,
        portal_id: Option<i32>,
        modifier_tokens: &[String],
        grade: Option<String>,
        params: &crate::panels::dungeon_callout::DungeonCalloutParams<'_>,
        server_name: Option<String>,
        realm_name: Option<String>,
        anchor: std::time::Instant,
        estimated: bool,
    ) -> Self {
        let modifiers: Vec<String> = modifier_tokens
            .iter()
            .map(|token| {
                realmhound_core::dungeon_modifiers::modifier_display_name(token)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| prettify_modifier(token))
            })
            .collect();
        let mods: Vec<ModChip> = modifier_tokens
            .iter()
            .zip(modifiers.iter())
            .map(|(token, name)| ModChip {
                name: name.clone(),
                mod_type: realmhound_core::dungeon_modifiers::mod_type(token)
                    .unwrap_or(ModType::Normal),
                danger: realmhound_core::dungeon_modifiers::mod_danger(token),
                is_dimitus: realmhound_core::dungeon_modifiers::canonical(token) == "DIMITUS",
                modifier_icon: ModTierIcon::from_token(token),
            })
            .collect();
        let outline = realmhound_core::dungeon_modifiers::outline_kind(modifier_tokens);
        let bonus = realmhound_core::dungeon_modifiers::total_bonus(modifier_tokens);
        let is_dimitus =
            realmhound_core::dungeon_modifiers::contains_modifier(modifier_tokens, "DIMITUS");
        // Build the clipboard callout from the dungeon's short nickname (or the
        // lowercased full name as a fallback) plus modifier-derived tags.
        // Returns None for non-callable dungeons and, when Dimitus is disabled
        // in the reward-mod list, Dimitus dungeons.
        let callout = crate::panels::dungeon_callout::dungeon_callout_for(
            &dungeon_name,
            modifier_tokens,
            params,
        );
        // Non-callable dungeons (Crystal Cavern, Cultist Hideout, The Void) can't
        // be joined from the outside, so they start expired with no timer.
        let window = if estimated {
            DUNGEON_WINDOW_ESTIMATED
        } else {
            DUNGEON_WINDOW_ANCHORED
        };
        let expires_at = if crate::panels::dungeon_callout::is_non_callable(&dungeon_name) {
            std::time::Instant::now()
        } else {
            anchor + window
        };
        Self {
            timestamp: Local::now(),
            expires_at,
            dungeon_name,
            portal_id,
            modifiers,
            mods,
            outline,
            grade,
            loot_bonus: bonus.loot,
            dust_bonus: bonus.dust,
            xp_bonus: bonus.xp,
            callout,
            is_dimitus,
            modifier_tokens: modifier_tokens.to_vec(),
            server_name,
            realm_name,
            estimated,
            map_seed: 0,
            entered_at: std::time::Instant::now(),
            frozen_elapsed_ms: None,
        }
    }

    /// The nominal join window length for this entry, used as the countdown
    /// fraction denominator so the circle starts full.
    pub fn window(&self) -> std::time::Duration {
        if self.estimated {
            DUNGEON_WINDOW_ESTIMATED
        } else {
            DUNGEON_WINDOW_ANCHORED
        }
    }

    /// Remaining join window as of `now`, saturating at zero once the window
    /// elapses. Drives the countdown circle and the expired styling.
    pub fn remaining(&self, now: std::time::Instant) -> std::time::Duration {
        self.expires_at.saturating_duration_since(now)
    }

    /// Force this entry into the expired state (e.g. when the player leaves the
    /// map and can no longer call the dungeon they were in).
    pub fn deactivate(&mut self) {
        self.expires_at = std::time::Instant::now();
    }

    /// Elapsed time for the dungeon run timer: the frozen committed value once
    /// the run has ended, else the live time since entry.
    pub fn run_elapsed(&self, now: std::time::Instant) -> std::time::Duration {
        match self.frozen_elapsed_ms {
            Some(ms) => std::time::Duration::from_millis(ms.max(0) as u64),
            None => now.saturating_duration_since(self.entered_at),
        }
    }
}

/// Format a dungeon run duration as `M:SS` (or `H:MM:SS` past an hour).
fn format_run_timer(d: std::time::Duration) -> String {
    let total = d.as_secs();
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{}:{:02}", m, s)
    }
}

/// Convert a raw modifier id token (e.g. "SOUVENIR_1") into a display string
/// (e.g. "Souvenir 1") by splitting on underscores and title-casing each word.
fn prettify_modifier(token: &str) -> String {
    title_case_words(&token.replace('_', " "))
}

/// Format a raw map display name for the feed.
///
/// First resolves known localization keys via the dungeon portal map. If the
/// name is still an unresolved localization key (e.g. "{s.spider_den}"), strips
/// the `{s.` / `}` wrapper and title-cases the words so it renders cleanly.
fn format_dungeon_name(raw: &str) -> String {
    let normalized = get_dungeon_portal_map().normalize_dungeon_name(raw);

    let trimmed = normalized.trim();
    if let Some(inner) = trimmed
        .strip_prefix("{s.")
        .and_then(|s| s.strip_suffix('}'))
    {
        return title_case_words(&inner.replace('_', " "));
    }

    normalized
}

/// Title-case each whitespace-separated word in `input`.
fn title_case_words(input: &str) -> String {
    input
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Active encounter data (realm boss/event visible in range).
#[derive(Debug, Clone)]
pub struct ActiveEncounter {
    /// Object type ID (for sprite rendering)
    pub object_type: u16,
    /// Display name
    pub name: String,
    /// When this encounter was first seen (for ordering - newest first)
    pub added_at: std::time::Instant,
}

/// Live feed panel state and UI.
pub struct LiveFeedPanel {
    /// Feed entries (newest at front)
    entries: VecDeque<FeedEntry>,
    /// Whether to auto-scroll to newest entries
    auto_scroll: bool,
    /// Whether we need to scroll to top on the next frame (set when new entry added)
    needs_scroll_to_top: bool,
    /// Status message for user feedback
    status_message: Option<(String, std::time::Instant)>,
    /// True until a map load (HELLO) is captured this session. While set, the app
    /// launched mid-map and capture only begins on the next map change.
    awaiting_first_map: bool,
    /// Latches once the main account has been Connected at least once this
    /// session. Prevents the mid-game capture hint from re-flashing during the
    /// brief disconnects that occur on every portal / map change.
    has_captured_connection: bool,
    /// Active encounters (realm bosses/events currently in view)
    /// Key is object_id (instance ID)
    active_encounters: HashMap<i32, ActiveEncounter>,

    // === Realm Status Bar State ===
    /// Current server name (e.g., "EUWest")
    server_name: Option<String>,
    /// Current realm name (e.g., "Medusa") - remembered when entering dungeons
    realm_name: Option<String>,
    /// Current map/location display name (e.g., "Spider Den", "Nexus")
    location_name: String,
    /// Realm score: (current_score, max_score) - None if not in realm
    realm_score: Option<(i32, i32)>,
    /// True when showing remembered score from before entering dungeon
    is_score_stale: bool,
    /// True when the player is inside a dungeon (from MapChanged.is_dungeon).
    currently_in_dungeon: bool,
    /// Countdown expiry to the castle teleport after a realm close.
    /// `None` when no realm-close countdown is active. Cleared on a fresh realm
    /// entry, hub entry, or disconnect; kept running while diving into a
    /// dungeon from the closing realm.
    castle_timer_expires_at: Option<std::time::Instant>,
    /// True once the realm has closed and the Oryx endgame (Castle/Chamber/Wine
    /// Cellar/Sanctuary) sequence has begun. While set, dungeons
    /// opened with a key inside the castle areas stay callable even though the
    /// stale realm score still reads 100% (CLOSED). Reset alongside
    /// `castle_timer_expires_at` when the realm context genuinely ends (a fresh
    /// realm, a hub, or disconnect).
    realm_closed: bool,

    // === Dust Status Bar State ===
    /// Cached dust amounts for both seasonal and regular accounts.
    dust_state: DustState,
    /// Whether current character is seasonal (for dust display emphasis).
    is_seasonal: bool,
    /// How the forge-dust readout lays out the two accounts (mirrors
    /// `LiveFeedSettings::dust_display_mode`).
    dust_display_mode: realmhound_core::settings::DustDisplayMode,
    /// How the forge-materials readout lays out the two accounts (mirrors
    /// `LiveFeedSettings::materials_display_mode`).
    materials_display_mode: realmhound_core::settings::MaterialsDisplayMode,
    /// Signature (fp, name, modifiers, grade) of the most recently pushed dungeon
    /// entry. Suppresses duplicate dungeon entries on repeated MapInfo packets;
    /// cleared when leaving the dungeon (entering a hub/realm). The `fp` (map
    /// seed) distinguishes separate instances of the same dungeon type.
    last_dungeon_signature: Option<(i32, String, Vec<String>, Option<String>)>,
    /// Prefix clipboard callouts with `/p`. Mirrors
    /// `Settings.live_feed.call_for_party`. Toggled from the tab header.
    call_for_party: bool,
    /// Prefix clipboard callouts with the short server name.
    /// Mirrors `Settings.live_feed.include_server_name`. Toggled from the header.
    include_server_name: bool,
    /// Mirrors `Settings.live_feed.include_realm_name`. Toggled from the header.
    include_realm_name: bool,
    /// Where the join marker `j` is placed in dungeon callouts.
    /// Mirrors `Settings.live_feed.dungeon_join_position`.
    dungeon_join_position: realmhound_core::settings::JoinPosition,
    /// Where the join marker `j` is placed in event callouts.
    /// Mirrors `Settings.live_feed.event_join_position`.
    event_join_position: realmhound_core::settings::JoinPosition,
    /// Whether dungeon callouts use short nicknames or full names.
    /// Mirrors `Settings.live_feed.dungeon_name_style`.
    dungeon_name_style: realmhound_core::settings::DungeonNameStyle,
    /// Whether event callouts use short names or full names.
    /// Mirrors `Settings.live_feed.event_name_style`.
    event_name_style: realmhound_core::settings::DungeonNameStyle,
    /// Whether event callouts append the upcoming-dungeon hint.
    /// Mirrors `Settings.live_feed.event_add_upcoming`.
    event_add_upcoming: bool,
    /// How the loot-boost tag is rendered. Mirrors `Settings.live_feed.loot_label`.
    loot_label: realmhound_core::settings::LootLabel,
    /// How the dust-boost tag is rendered. Mirrors `Settings.live_feed.dust_label`.
    dust_label: realmhound_core::settings::DustLabel,
    /// How the XP-boost tag is rendered. Mirrors `Settings.live_feed.xp_label`.
    xp_label: realmhound_core::settings::XpLabel,
    /// Whether callout reward values include a `%` sign.
    /// Mirrors `Settings.live_feed.callout_percent`.
    callout_percent: bool,
    /// Editable reward-modifier callout tags. Mirrors `Settings.live_feed.reward_mods`.
    reward_mods: Vec<realmhound_core::settings::RewardModEntry>,
    /// Dungeon short-name overrides. Mirrors `Settings.live_feed.dungeon_name_overrides`.
    dungeon_name_overrides: std::collections::BTreeMap<String, String>,
    /// Event short-name overrides. Mirrors `Settings.live_feed.event_name_overrides`.
    event_name_overrides: std::collections::BTreeMap<String, String>,
    /// Alien Invasion callout prefixes / options. Mirror the matching
    /// `Settings.live_feed.alien_*` fields.
    alien_adept_prefix: String,
    alien_veteran_prefix: String,
    alien_wave4_boss: bool,
    /// Set when a header toggle changes so the app can persist it.
    pending_header_settings_save: bool,
    /// Observed portal spawn times keyed by portal object id. Populated from
    /// `UpdateReceived` new-objects classified as portals, pruned on drops and
    /// cleared on map change. Used to anchor the dungeon join countdown to the
    /// real spawn instead of the local entry time.
    portal_spawns: HashMap<i32, std::time::Instant>,
    /// Spawn instant of the portal the player just used (from the outgoing
    /// UsePortal packet), consumed by the next `push_dungeon` to anchor its
    /// countdown. `None` falls back to the entry time.
    pending_portal_spawn: Option<std::time::Instant>,
    /// Bag-type filters that hide matching loot from the feed display. Tracking
    /// is unaffected. Independent from the Loot History filters, persisted via
    /// `LiveFeedSettings::filters`.
    loot_filters: LootFilters,
    /// Item-property (UT/ST/Shiny + rarity) display filters for the feed.
    prop_filters: PropertyFilters,
    /// Set when a filter-bar toggle changes so the app can persist it.
    pending_filter_settings_save: bool,
    /// Whether DM entries are shown in the feed (mirrors `LiveFeedSettings::show_dm_in_live_feed`).
    show_dms: bool,
    /// Whether to show DMs in the filter (mirrors `LiveFeedFilterSettings::show_dms`).
    show_dms_filter: bool,
    /// Whether `/who` roster entries are shown (mirrors `LiveFeedSettings::show_who_roster`).
    show_who_roster: bool,
    /// Whether public key-pop entries are shown (mirrors `LiveFeedSettings::show_key_pops`).
    show_key_pops: bool,
    /// Whether area-unlock entries are shown (mirrors `LiveFeedSettings::show_area_unlocks`).
    show_area_unlocks: bool,
    /// Whether dungeon-entry callouts are shown (mirrors `LiveFeedSettings::show_dungeon_entries`).
    show_dungeon_entries: bool,
    /// Whether realm event / boss callouts are shown (mirrors `LiveFeedSettings::show_boss_calls`).
    show_boss_calls: bool,
    /// Whether loot-drop entries are shown (mirrors `LiveFeedSettings::show_loot_drops`).
    show_loot_drops: bool,
    /// Whether realm-close / Oryx-lag warnings are shown (mirrors `LiveFeedSettings::show_realm_warnings`).
    show_realm_warnings: bool,
    /// Whether forge-dust cap warnings are shown (mirrors `LiveFeedSettings::show_dust_full`).
    show_dust_full: bool,
    /// User-configurable quick-clipboard buttons (mirrors `LiveFeedSettings::custom_clip_buttons`).
    custom_clip_buttons: Vec<realmhound_core::settings::QuickClipButton>,
    /// Set when custom clip buttons are edited and need persisting.
    pending_clip_buttons_save: bool,
    /// End-of-cycle warning toggles.
    season_warnings: realmhound_core::settings::SeasonWarningSettings,
    /// Current season display name (from shared season config).
    season_name: String,
    /// Current battlepass display name (from shared season config).
    battlepass_name: String,
    /// Parsed season reset instant (UTC), or `None` when unset/invalid.
    season_reset: Option<DateTime<Utc>>,
    /// Parsed battlepass reset instant (UTC), or `None` when unset/invalid.
    battlepass_reset: Option<DateTime<Utc>>,
    /// Pending actions to be picked up by `show()` and returned to the app.
    pending_actions: Vec<super::AppAction>,
    /// Active Crystal Prisoner pin: the Mysterious Crystal setpiece re-announces
    /// itself (~every 2 min) while alive. Instead of a scrolling entry we pin a
    /// single row and show how long ago it last spoke. `None` until a Crystal
    /// callout is witnessed in the current realm.
    crystal_pin: Option<CrystalPin>,
}

/// A pinned Mysterious Crystal callout, kept until the player unpins it or it
/// expires (see [`CRYSTAL_STALE_AFTER`] / [`CRYSTAL_REMOVE_AFTER_STALE`]).
#[derive(Debug, Clone)]
struct CrystalPin {
    /// When the Crystal last announced itself (drives the live "Ns ago" timer).
    last_spoke: std::time::Instant,
    /// Wall-clock time of the last callout, preserved so the pin can be turned
    /// into a normal Live Feed entry (on unpin / expiry) with its real time.
    last_spoke_at: DateTime<Local>,
    /// Boss object id for the sprite.
    boss_id: i32,
    server_name: Option<String>,
    realm_name: Option<String>,
}

/// After this long with no Crystal callout, the pinned Crystal icon is dimmed --
/// it may have been killed.
const CRYSTAL_STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(300);
/// Once a pin has been stale (dimmed) for this much longer with no callout, it
/// is auto-removed and folded into the scrolling feed as a normal entry.
const CRYSTAL_REMOVE_AFTER_STALE: std::time::Duration = std::time::Duration::from_secs(180);

impl Default for LiveFeedPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveFeedPanel {
    /// Create a new live feed panel.
    pub fn new() -> Self {
        Self {
            entries: VecDeque::with_capacity(MAX_FEED_ENTRIES),
            auto_scroll: true,
            needs_scroll_to_top: false,
            status_message: None,
            awaiting_first_map: true,
            has_captured_connection: false,
            active_encounters: HashMap::new(),
            // Realm status bar state
            server_name: None,
            realm_name: None,
            location_name: String::new(),
            realm_score: None,
            is_score_stale: false,
            currently_in_dungeon: false,
            castle_timer_expires_at: None,
            realm_closed: false,
            // Dust status bar state
            dust_state: DustState::new(),
            is_seasonal: false,
            dust_display_mode: realmhound_core::settings::DustDisplayMode::default(),
            materials_display_mode: realmhound_core::settings::MaterialsDisplayMode::default(),
            last_dungeon_signature: None,
            call_for_party: true,
            include_server_name: false,
            include_realm_name: false,
            dungeon_join_position: realmhound_core::settings::JoinPosition::End,
            event_join_position: realmhound_core::settings::JoinPosition::None,
            dungeon_name_style: realmhound_core::settings::DungeonNameStyle::default(),
            event_name_style: realmhound_core::settings::DungeonNameStyle::default(),
            event_add_upcoming: true,
            loot_label: realmhound_core::settings::LootLabel::default(),
            dust_label: realmhound_core::settings::DustLabel::default(),
            xp_label: realmhound_core::settings::XpLabel::default(),
            callout_percent: false,
            reward_mods: realmhound_core::settings::default_reward_mods(),
            dungeon_name_overrides: std::collections::BTreeMap::new(),
            event_name_overrides: std::collections::BTreeMap::new(),
            alien_adept_prefix: "adept wave".to_string(),
            alien_veteran_prefix: "veteran wave".to_string(),
            alien_wave4_boss: true,
            pending_header_settings_save: false,
            portal_spawns: HashMap::new(),
            pending_portal_spawn: None,
            loot_filters: LootFilters::default(),
            prop_filters: PropertyFilters::default(),
            pending_filter_settings_save: false,
            show_dms: true,
            show_dms_filter: true,
            show_who_roster: true,
            show_key_pops: true,
            show_area_unlocks: true,
            show_dungeon_entries: true,
            show_boss_calls: true,
            show_loot_drops: true,
            show_realm_warnings: true,
            show_dust_full: true,
            custom_clip_buttons: Default::default(),
            pending_clip_buttons_save: false,
            season_warnings: realmhound_core::settings::SeasonWarningSettings::default(),
            season_name: String::new(),
            battlepass_name: String::new(),
            season_reset: None,
            battlepass_reset: None,
            pending_actions: Vec::new(),
            crystal_pin: None,
        }
    }

    /// Apply all persisted Live Feed settings to the panel's local mirrors.
    /// Called by the app on startup and whenever
    /// the settings modal changes them. Recomputes callouts on existing dungeon
    /// entries so setting changes apply retroactively.
    pub fn apply_live_feed_settings(
        &mut self,
        settings: &realmhound_core::settings::LiveFeedSettings,
    ) {
        // Detect whether callout-affecting settings changed (so existing feed
        // entries can be re-rendered).
        let callout_settings_changed = self.dungeon_name_style != settings.dungeon_name_style
            || self.loot_label != settings.loot_label
            || self.dust_label != settings.dust_label
            || self.xp_label != settings.xp_label
            || self.callout_percent != settings.callout_percent
            || self.reward_mods != settings.reward_mods
            || self.dungeon_name_overrides != settings.dungeon_name_overrides;

        self.call_for_party = settings.call_for_party;
        self.include_server_name = settings.include_server_name;
        self.include_realm_name = settings.include_realm_name;
        self.dungeon_join_position = settings.dungeon_join_position;
        self.event_join_position = settings.event_join_position;
        self.dungeon_name_style = settings.dungeon_name_style;
        self.event_name_style = settings.event_name_style;
        self.event_add_upcoming = settings.event_add_upcoming;
        self.loot_label = settings.loot_label;
        self.dust_label = settings.dust_label;
        self.xp_label = settings.xp_label;
        self.callout_percent = settings.callout_percent;
        self.reward_mods = settings.reward_mods.clone();
        self.dungeon_name_overrides = settings.dungeon_name_overrides.clone();
        self.event_name_overrides = settings.event_name_overrides.clone();
        self.alien_adept_prefix = settings.alien_adept_prefix.clone();
        self.alien_veteran_prefix = settings.alien_veteran_prefix.clone();
        self.alien_wave4_boss = settings.alien_wave4_boss;

        self.loot_filters = LootFilters::from_live_feed_settings(&settings.filters);
        self.prop_filters = PropertyFilters::from_live_feed_settings(&settings.filters);
        self.dust_display_mode = settings.dust_display_mode;
        self.materials_display_mode = settings.materials_display_mode;
        self.show_dms = settings.show_dm_in_live_feed;
        self.show_dms_filter = settings.filters.show_dms;
        self.show_who_roster = settings.show_who_roster;
        self.show_key_pops = settings.show_key_pops;
        self.show_area_unlocks = settings.show_area_unlocks;
        self.show_dungeon_entries = settings.show_dungeon_entries;
        self.show_boss_calls = settings.show_boss_calls;
        self.show_loot_drops = settings.show_loot_drops;
        self.show_realm_warnings = settings.show_realm_warnings;
        self.show_dust_full = settings.show_dust_full;
        self.custom_clip_buttons = settings.custom_clip_buttons.clone();
        self.season_warnings = settings.season_warnings.clone();

        if callout_settings_changed {
            self.recompute_dungeon_callouts();
        }
    }

    /// Apply the shared season / battlepass config, parsing
    /// the user-entered reset times into absolute UTC instants used by the
    /// pinned end-of-cycle warnings.
    pub fn apply_season_config(&mut self, season: &realmhound_core::settings::SeasonConfig) {
        self.season_name = season.season_name.clone();
        self.battlepass_name = season.battlepass_name.clone();
        self.season_reset = realmhound_core::season::reset_to_datetime(&season.reset);
        self.battlepass_reset =
            realmhound_core::season::battlepass_target(season, chrono::Utc::now());
    }

    /// Borrow the current dungeon-callout formatting parameters from the panel's
    /// settings mirrors.
    fn dungeon_callout_params(&self) -> crate::panels::dungeon_callout::DungeonCalloutParams<'_> {
        crate::panels::dungeon_callout::DungeonCalloutParams {
            name_style: self.dungeon_name_style,
            name_overrides: &self.dungeon_name_overrides,
            loot_label: self.loot_label,
            dust_label: self.dust_label,
            xp_label: self.xp_label,
            percent: self.callout_percent,
            reward_mods: &self.reward_mods,
        }
    }

    /// Recompute callouts on all existing dungeon entries using the current
    /// settings mirrors. Called when callout-affecting settings change so that
    /// existing feed entries reflect the new formatting.
    fn recompute_dungeon_callouts(&mut self) {
        let params = crate::panels::dungeon_callout::DungeonCalloutParams {
            name_style: self.dungeon_name_style,
            name_overrides: &self.dungeon_name_overrides,
            loot_label: self.loot_label,
            dust_label: self.dust_label,
            xp_label: self.xp_label,
            percent: self.callout_percent,
            reward_mods: &self.reward_mods,
        };
        for entry in &mut self.entries {
            if let FeedEntry::Dungeon(d) = entry {
                d.callout = crate::panels::dungeon_callout::dungeon_callout_for(
                    &d.dungeon_name,
                    &d.modifier_tokens,
                    &params,
                );
            }
        }
    }

    /// Assemble a full clipboard callout from a bare body (no `/p`, server name,
    /// realm name, or join marker), applying the party prefix, short server name,
    /// realm name, and join marker placement.
    ///
    /// Layout: `[/p ][j ][<SERVER> ][<REALM> ]<body>[ j]` where `j` is placed
    /// right after the `/p` prefix when configured to the beginning, or appended
    /// at the end.
    ///
    /// `join` is the marker placement to use, which differs between dungeon and
    /// event callouts.
    ///
    /// `server` is the server name to prefix (when enabled). Callers pass the
    /// server captured on the feed entry (e.g. `boss.server_name` or
    /// `dungeon.server_name`) so an old row copies the server it was seen on.
    ///
    /// `realm` is the realm name to include (when enabled). Same capture logic.
    fn assemble_callout(
        &self,
        body: &str,
        server: Option<&str>,
        realm: Option<&str>,
        join: realmhound_core::settings::JoinPosition,
    ) -> String {
        use realmhound_core::settings::JoinPosition;
        let mut out = String::new();
        if self.call_for_party {
            out.push_str("/p ");
        }
        if join == JoinPosition::Beginning {
            out.push_str("j ");
        }
        if self.include_server_name {
            if let Some(server) = server {
                out.push_str(realmhound_core::protocol::short_server_name(server));
                out.push(' ');
            }
        }
        if self.include_realm_name {
            if let Some(realm) = realm {
                out.push_str(realm);
                out.push(' ');
            }
        }
        out.push_str(body);
        if join == JoinPosition::End {
            out.push_str(" j");
        }
        out
    }

    /// Initialize dust state from cached account data.
    pub fn init_dust_from_account_data(
        &mut self,
        account_data: &realmhound_core::vault::AccountData,
    ) {
        if let Some(ref regular) = account_data.regular_dust {
            self.dust_state.regular = Some(regular.clone());
        }
        if let Some(ref seasonal) = account_data.seasonal_dust {
            self.dust_state.seasonal = Some(seasonal.clone());
        }
    }

    /// Update dust amounts from a DustUpdated event. Pushes a Live Feed
    /// notification for each dust type that just transitioned to full. The first
    /// observation of an account only seeds state, so an account already at cap
    /// on login does not spam notifications.
    pub fn update_dust(&mut self, amounts: DustAmounts, is_seasonal: bool) {
        let previous = self.dust_state.get(is_seasonal).cloned();
        self.dust_state.update(amounts.clone(), is_seasonal);
        self.is_seasonal = is_seasonal;

        let Some(previous) = previous else {
            return;
        };

        let previously_full = previous.full_types();
        for dust_type in amounts.full_types() {
            if !previously_full.contains(&dust_type) {
                self.push_entry(FeedEntry::DustFull(DustFullEntry::new(
                    is_seasonal,
                    dust_type,
                )));
            }
        }
    }

    /// Set the current character's seasonal status (for display emphasis).
    pub fn set_seasonal(&mut self, is_seasonal: bool) {
        self.is_seasonal = is_seasonal;
    }

    /// Current dust amounts for both account types (rendered in the top bar).
    pub fn dust_state(&self) -> &DustState {
        &self.dust_state
    }

    /// Whether the current character is seasonal.
    pub fn is_seasonal(&self) -> bool {
        self.is_seasonal
    }

    /// How the forge-dust readout should arrange the two accounts.
    pub fn dust_display_mode(&self) -> realmhound_core::settings::DustDisplayMode {
        self.dust_display_mode
    }

    /// How the forge-materials readout should arrange the two accounts.
    pub fn materials_display_mode(&self) -> realmhound_core::settings::MaterialsDisplayMode {
        self.materials_display_mode
    }

    // === Realm Status Bar Methods ===

    /// Set the server name (from IP address lookup). Called when a new server
    /// connection to a known nexus IP is detected. Unmapped IPs (in-realm and
    /// dungeon backend hosts) are ignored so the last known name persists.
    pub fn set_server_name(&mut self, name: String) {
        self.server_name = Some(name);
    }

    /// Authoritatively set the header server and realm from a `/server` command
    /// reply. Unlike IP lookup or MapInfo (which can be stale when you join a
    /// realm via another server's party), `/server` is always correct about the
    /// current server and realm, so it overrides both. Passing `None` for the
    /// realm means a hub (Nexus/Vault/etc.), which clears realm state.
    pub fn apply_server_command(&mut self, server: String, realm: Option<String>) {
        self.server_name = Some(server);
        match realm {
            Some(r) => {
                // A different realm than we were tracking invalidates any score
                // and castle-close countdown we were showing; the name is
                // authoritative, that transient state isn't.
                let same = self.realm_name.as_deref() == Some(r.as_str());
                self.realm_name = Some(r);
                // Don't touch currently_in_dungeon here: a `/server` reply
                // reports the parent realm even while you're inside a dungeon,
                // so clearing the flag would wrongly hide the dungeon portal
                // icon and suppress the Oryx-lag warning. MapChanged is the
                // authoritative source for dungeon state.
                if !same {
                    self.realm_score = None;
                    self.is_score_stale = false;
                    self.castle_timer_expires_at = None;
                    self.realm_closed = false;
                }
            }
            None => {
                // A `/server` reply reads "<Server> Nexus" both in the real
                // Nexus/hub and inside a dungeon that was *opened from* the
                // Nexus (the reply mirrors the dungeon's parent area), so it
                // can't tell the two apart. It therefore must not clear
                // `currently_in_dungeon` -- doing so wrongly erased the dungeon
                // portal icon for Nexus-opened dungeons. MapChanged is
                // the authoritative source for dungeon state and clears the flag
                // on the real exit to the Nexus. Realm-only state is still safe
                // to clear here.
                self.realm_name = None;
                self.realm_score = None;
                self.is_score_stale = false;
                self.castle_timer_expires_at = None;
                self.realm_closed = false;
            }
        }
    }

    /// Parse realm_name field (e.g., "NexusPortal.Meridian") to extract the realm name.
    /// The format is "Prefix.RealmName" with a dot separator.
    /// Returns the realm name part, or None if no dot separator.
    fn parse_realm_name_for_realm(realm_name: &str) -> Option<String> {
        if realm_name.is_empty() {
            return None;
        }
        // Format: "NexusPortal.Meridian" -> "Meridian"
        let parts: Vec<&str> = realm_name.splitn(2, '.').collect();
        if parts.len() == 2 && !parts[1].is_empty() {
            Some(parts[1].to_string())
        } else {
            None
        }
    }

    /// Update location state from MapInfo packet data.
    ///
    /// Logic:
    /// - Server name is set separately via set_server_name() from IP lookup
    /// - If entering a realm (has realm score): Store fresh realm data
    /// - If entering dungeon (no realm score but had previous realm): Keep realm data, mark stale
    /// - If entering Nexus/Vault: Clear realm data
    ///
    /// Data format notes:
    /// - `display_name`: readable map name (e.g., "Nexus", "Spider Den", "Realm of the Mad God")  
    /// - `realm_name_field`: "NexusPortal.RealmName" in realms (e.g., "NexusPortal.Medusa"),
    ///   or just location name elsewhere (e.g., "Nexus")
    pub fn update_location(
        &mut self,
        display_name: &str,
        realm_name_field: &str,
        current_score: i32,
        max_score: i32,
    ) {
        self.location_name = format_dungeon_name(display_name);

        // Check if we're in a realm (has valid score)
        let in_realm = max_score > 0;

        // Parse realm name from realm_name field (e.g., "NexusPortal.Medusa" -> "Medusa")
        // Server name comes from IP lookup, not from this field
        if let Some(realm) = Self::parse_realm_name_for_realm(realm_name_field) {
            if in_realm {
                // Realm entry - only cancel a pending castle teleport countdown
                // when this is actually a *different* realm than before. If
                // it's the same realm we're returning to (e.g. after diving
                // into a dungeon from a closed realm), the countdown must
                // keep running uninterrupted.
                let same_realm = self
                    .realm_name
                    .as_ref()
                    .map(|r| r == &realm)
                    .unwrap_or(false);
                if !same_realm {
                    self.castle_timer_expires_at = None;
                    self.realm_closed = false;
                }
                self.realm_name = Some(realm);
                self.realm_score = Some((current_score, max_score));
                self.is_score_stale = false;
            } else {
                // Entering dungeon - check if it's from a different realm than we tracked
                let same_realm = self
                    .realm_name
                    .as_ref()
                    .map(|r| r == &realm)
                    .unwrap_or(false);

                if same_realm && self.realm_score.is_some() {
                    // Same realm - keep score but mark stale
                    self.is_score_stale = true;
                } else {
                    // Different realm or no previous score - clear score (unknown for this realm)
                    self.realm_score = None;
                    self.is_score_stale = false;
                }

                // Update to new realm name
                self.realm_name = Some(realm);
            }
        } else if !realm_name_field.is_empty() {
            // Single word realm_name (no dot) - this is Nexus/Vault/etc
            // Clear realm state (but keep server name from IP lookup)
            self.castle_timer_expires_at = None;
            self.realm_closed = false;
            self.realm_name = None;
            self.realm_score = None;
            self.is_score_stale = false;
        } else {
            // Empty realm_name - clear realm state only
            self.castle_timer_expires_at = None;
            self.realm_closed = false;
            self.realm_name = None;
            self.realm_score = None;
            self.is_score_stale = false;
        }
    }

    /// Update realm score from RealmScoreUpdate packet.
    /// This means we're actively in a realm, so clear stale flag.
    pub fn update_realm_score(&mut self, score: i32) {
        if let Some((_, max)) = self.realm_score {
            self.realm_score = Some((score, max));
            self.is_score_stale = false;
        } else if let Some(max) = self.get_max_realm_score() {
            // We have a stored max but score tuple was cleared somehow
            self.realm_score = Some((score, max));
            self.is_score_stale = false;
        }
        // If we don't have max_score, we can't update meaningfully
    }

    /// Get max realm score if stored (for RealmScoreUpdate handling).
    fn get_max_realm_score(&self) -> Option<i32> {
        self.realm_score.map(|(_, max)| max)
    }

    /// Clear all realm state (on disconnect/reconnect).
    pub fn clear_realm_state(&mut self) {
        self.server_name = None;
        self.realm_name = None;
        self.location_name.clear();
        self.realm_score = None;
        self.is_score_stale = false;
        self.currently_in_dungeon = false;
        self.castle_timer_expires_at = None;
        self.realm_closed = false;
        self.crystal_pin = None;
    }

    /// Get the realm score as a percentage (0-100), or None if no score.
    pub fn get_score_percent(&self) -> Option<i32> {
        self.realm_score.map(|(current, max)| {
            if max > 0 {
                ((current as f32 / max as f32) * 100.0).floor() as i32
            } else {
                0
            }
        })
    }

    /// Get color for score percentage based on urgency thresholds.
    /// 0-49%: Green, 50-84%: Yellow/Orange, 85-100%: Red
    fn score_color(percent: i32) -> Color32 {
        if percent < 50 {
            Color32::from_rgb(18, 219, 0) // Green - safe
        } else if percent < 85 {
            Color32::from_rgb(255, 128, 15) // Yellow/Orange - moderate
        } else {
            Color32::from_rgb(222, 56, 61) // Red - urgent
        }
    }

    /// Render the realm status inline for the header row: `Server | Location |
    /// [Realm] | Score%`, sized to its content so the callout toggles sit right
    /// after it. Returns true if clicked (for copy-to-clipboard handling).
    /// Falls back to the "Live Feed" title when there is no location/realm data.
    fn render_status_inline(&self, ui: &mut Ui) -> bool {
        // Fallback to the tab title when there's nothing to show.
        if self.location_name.is_empty() && self.server_name.is_none() {
            ui.label(
                RichText::new("Live Feed")
                    .heading()
                    .color(Color32::from_rgb(100, 200, 255)),
            );
            return false;
        }

        let text_color = Color32::from_rgb(176, 176, 176); // Light gray
        let separator_color = Color32::from_rgb(80, 80, 100);
        let font = egui::FontId::proportional(15.0);

        // Build the colored text segments.
        let mut segments: Vec<(String, Color32)> = Vec::new();
        let in_realm = self.realm_score.is_some() && !self.is_score_stale;
        let has_location_content = in_realm && self.realm_name.is_some()
            || !in_realm && (!self.location_name.is_empty() || self.realm_name.is_some());
        let has_score = self.get_score_percent().is_some();

        if let Some(ref server) = self.server_name {
            segments.push((server.clone(), text_color));
            if has_location_content || has_score {
                segments.push((" | ".to_string(), separator_color));
            }
        }
        if in_realm {
            if let Some(ref realm) = self.realm_name {
                segments.push((realm.clone(), text_color));
            }
        } else {
            if !self.location_name.is_empty() {
                segments.push((self.location_name.clone(), text_color));
            }
            if let Some(ref realm) = self.realm_name {
                segments.push((" | ".to_string(), separator_color));
                segments.push((realm.clone(), text_color));
            }
        }
        if let Some(percent) = self.get_score_percent() {
            segments.push((" | ".to_string(), separator_color));
            let score_color = Self::score_color(percent);
            let score_text = if percent >= 100 {
                "CLOSED".to_string()
            } else if self.is_score_stale {
                format!("~{}%", percent)
            } else {
                format!("{}%", percent)
            };
            segments.push((score_text, score_color));
        }

        // Measure the laid-out galleys to size the clickable element.
        let galleys: Vec<(std::sync::Arc<egui::Galley>, Color32)> = segments
            .iter()
            .map(|(t, c)| {
                (
                    ui.fonts_mut(|f| f.layout_no_wrap(t.clone(), font.clone(), *c)),
                    *c,
                )
            })
            .collect();
        let total_w: f32 = galleys.iter().map(|(g, _)| g.rect.width()).sum();
        let height = galleys
            .iter()
            .map(|(g, _)| g.rect.height())
            .fold(0.0_f32, f32::max)
            .max(20.0);

        // Click-to-copy only when we have realm context and realm isn't closed.
        let is_closed = self.get_score_percent().map_or(false, |p| p >= 100);
        let copyable = self.realm_score.is_some() && !is_closed;
        let sense = if copyable {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(egui::vec2(total_w, height), sense);

        let mut x = rect.left();
        let center_y = rect.center().y;
        let painter = ui.painter();
        for (galley, color) in galleys {
            let w = galley.rect.width();
            let h = galley.rect.height();
            painter.galley(egui::pos2(x, center_y - h / 2.0), galley, color);
            x += w;
        }

        if copyable {
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let tooltip = if self.is_score_stale {
                "Last known realm score \u{2022} Click to copy /p command"
            } else {
                "Click to copy /p command"
            };
            let clicked = response.clicked();
            response.hover_tip(tooltip);
            clicked
        } else {
            false
        }
    }

    /// Build the /p command string for clipboard copy.
    fn build_copy_command(&self) -> String {
        let mut parts = Vec::new();

        if let Some(ref server) = self.server_name {
            parts.push(server.clone());
        }

        // Use realm name if available, otherwise location
        if let Some(ref realm) = self.realm_name {
            parts.push(realm.clone());
        } else if !self.location_name.is_empty() {
            parts.push(self.location_name.clone());
        }

        // Add score if available
        if let Some(percent) = self.get_score_percent() {
            parts.push(format!("{}%", percent));
        }

        format!("/p {}", parts.join(" "))
    }

    // === End Realm Status Bar Methods ===

    /// Push a new entry to the feed.
    /// Automatically evicts oldest entries if the feed exceeds MAX_FEED_ENTRIES.
    pub fn push_entry(&mut self, entry: FeedEntry) {
        // Add to front (newest first)
        self.entries.push_front(entry);

        // Evict oldest if over capacity
        while self.entries.len() > MAX_FEED_ENTRIES {
            self.entries.pop_back();
        }

        // Trigger scroll to top if auto-scroll is enabled
        if self.auto_scroll {
            self.needs_scroll_to_top = true;
        }
    }

    /// Push a loot drop entry.
    pub fn push_loot(&mut self, drop: &ProcessedLootDrop) {
        self.push_entry(FeedEntry::Loot(LootEntry::from_drop(drop)));
    }

    /// Whether an entry is shown under the active feed filters. Loot entries are
    /// filtered by bag type and item properties; each notification/warning
    /// category (DMs, who, dungeons, boss calls, loot, realm warnings, dust)
    /// also has its own Live Feed visibility toggle. Kick alerts always pass.
    fn entry_passes_filter(&self, entry: &FeedEntry) -> bool {
        match entry {
            FeedEntry::Loot(loot) => {
                self.show_loot_drops
                    && self.loot_filters.is_visible(loot.bag_type)
                    && self.prop_filters.matches_items(&loot.items)
            }
            FeedEntry::DirectMessage(_) => self.show_dms && self.show_dms_filter,
            FeedEntry::WhoRoster(_) => self.show_who_roster,
            FeedEntry::Dungeon(_) => self.show_dungeon_entries,
            FeedEntry::BossCall(_) => self.show_boss_calls,
            FeedEntry::Warning(_) => self.show_realm_warnings,
            FeedEntry::DustFull(_) => self.show_dust_full,
            _ => true,
        }
    }

    /// Snapshot the active loot filters into a persistable settings struct.
    fn current_filter_settings(&self) -> realmhound_core::settings::LiveFeedFilterSettings {
        let mut s = realmhound_core::settings::LiveFeedFilterSettings::default();
        self.loot_filters.write_to_live_feed_settings(&mut s);
        self.prop_filters.write_to_live_feed_settings(&mut s);
        s.show_dms = self.show_dms_filter;
        s
    }

    /// Push a boss callout entry. Returns `true` when the caller should play the
    /// notification sound.
    ///
    /// Captures current server and realm name for historical reference.
    ///
    /// The Mysterious Crystal setpiece is special-cased into a pinned row (see
    /// [`Self::update_crystal_pin`]) and sounds only on its first callout per
    /// realm. For every other boss, repeated callouts for the same boss in the
    /// same realm within `BOSS_CALL_DEDUP_WINDOW` are suppressed (one Live Feed
    /// entry, one sound).
    pub fn push_boss_call(&mut self, boss_id: i32, boss_name: String) -> bool {
        if Self::is_crystal_call(&boss_name) {
            return self.update_crystal_pin(boss_id);
        }
        let now = Local::now();
        for entry in &self.entries {
            if let FeedEntry::BossCall(existing) = entry {
                // Entries are newest-first, so once a callout is older than the
                // window every earlier callout is too.
                if now.signed_duration_since(existing.timestamp) > BOSS_CALL_DEDUP_WINDOW {
                    break;
                }
                // Scope dedup to the current realm so an identical boss in a
                // different realm entered within the window still notifies.
                if existing.boss_id == boss_id
                    && existing.boss_name == boss_name
                    && existing.realm_name == self.realm_name
                {
                    return false;
                }
            }
        }
        let server = self.server_name.clone();
        let realm = self.realm_name.clone();
        // When a new Alien Invasion wave spawns, the previous wave(s) in the
        // same realm are superseded: fade them out and make them non-copyable.
        if Self::is_alien_wave(&boss_name) {
            for entry in &mut self.entries {
                if let FeedEntry::BossCall(existing) = entry {
                    if existing.realm_name == realm && Self::is_alien_wave(&existing.boss_name) {
                        existing.expired = true;
                    }
                }
            }
        }
        self.push_entry(FeedEntry::BossCall(BossCallEntry::new(
            boss_id, boss_name, server, realm,
        )));
        true
    }

    /// Whether a boss callout is an Alien Invasion wave (Adept or Veteran).
    /// Subsequent waves in the same realm supersede earlier ones.
    fn is_alien_wave(boss_name: &str) -> bool {
        boss_name.starts_with("Alien Invasion") && boss_name.contains("Wave")
    }

    /// Whether a boss callout is the Mysterious Crystal / Crystal Prisoner
    /// setpiece (routed to the pinned row instead of the scrolling feed).
    fn is_crystal_call(boss_name: &str) -> bool {
        boss_name.contains("Mysterious Crystal") || boss_name.contains("Crystal Prisoner")
    }

    /// Refresh (or create) the pinned Crystal Prisoner row for the current
    /// realm. Returns `true` only on the first callout in a given realm, so the
    /// sound plays once per realm and repeat announcements only reset the timer.
    fn update_crystal_pin(&mut self, boss_id: i32) -> bool {
        let same_realm = self
            .crystal_pin
            .as_ref()
            .map(|p| p.realm_name == self.realm_name && p.server_name == self.server_name)
            .unwrap_or(false);
        self.crystal_pin = Some(CrystalPin {
            last_spoke: std::time::Instant::now(),
            last_spoke_at: Local::now(),
            boss_id,
            server_name: self.server_name.clone(),
            realm_name: self.realm_name.clone(),
        });
        !same_realm
    }

    /// Convert the current Crystal pin into a normal, scrolling Live Feed entry
    /// (used on unpin and on auto-expiry) so the sighting isn't lost.
    fn fold_crystal_pin_into_feed(&mut self, pin: &CrystalPin) {
        let mut entry = BossCallEntry::new(
            pin.boss_id,
            "Mysterious Crystal".to_string(),
            pin.server_name.clone(),
            pin.realm_name.clone(),
        );
        entry.timestamp = pin.last_spoke_at;
        self.push_entry(FeedEntry::BossCall(entry));
    }

    /// Push a kick alert entry.
    pub fn push_kick_alert(&mut self, player_name: String) {
        self.push_entry(FeedEntry::KickAlert(KickAlertEntry::new(player_name)));
    }

    /// Push a join request alert for a banned player trying to join.
    pub fn push_join_request_alert(&mut self, player_name: String) {
        self.push_entry(FeedEntry::KickAlert(KickAlertEntry::join_request(
            player_name,
        )));
    }

    /// Push a direct-message entry.
    pub fn push_dm(&mut self, sender: String, text: String) {
        self.push_entry(FeedEntry::DirectMessage(DmEntry::new(sender, text)));
    }

    /// Push a `/who` roster entry from already-sorted names.
    pub fn push_who_roster(&mut self, names: Vec<String>) {
        self.push_entry(FeedEntry::WhoRoster(WhoRosterEntry::new(names)));
    }

    /// Push a public dungeon key-pop entry.
    pub fn push_key_pop(
        &mut self,
        key_id: i32,
        dungeon_name: String,
        opener: String,
        thankable: bool,
    ) {
        self.push_entry(FeedEntry::KeyPop(KeyPopEntry::new(
            key_id,
            dungeon_name,
            opener,
            thankable,
        )));
    }

    /// Push a special area-unlock entry (single-item pop or combined monuments).
    pub fn push_area_unlock(
        &mut self,
        title: String,
        contributions: Vec<(i32, String)>,
        callout: String,
    ) {
        self.push_entry(FeedEntry::AreaUnlock(AreaUnlockEntry::new(
            title,
            contributions,
            callout,
        )));
    }

    pub fn push_warning(&mut self, text: String, severity: WarningSeverity, copyable: bool) {
        self.push_entry(FeedEntry::Warning(WarningEntry::new(
            text, severity, copyable,
        )));
    }

    pub fn force_realm_closed(&mut self) {
        if let Some((_, max)) = self.realm_score {
            self.realm_score = Some((max, max));
            self.is_score_stale = false;
        }
        // The realm is closing and the Oryx endgame begins; dungeons opened with
        // a key inside the castle areas from here on stay callable.
        self.realm_closed = true;
        // Start the castle teleport countdown; ignore repeated
        // realm-close messages so the window isn't reset each time.
        if self.castle_timer_expires_at.is_none() {
            self.castle_timer_expires_at = Some(std::time::Instant::now() + CASTLE_TELEPORT_WINDOW);
        }
    }

    /// Remaining time until the castle teleport, if a realm-close countdown is
    /// active.
    fn castle_timer_remaining(&self, now: std::time::Instant) -> Option<std::time::Duration> {
        self.castle_timer_expires_at
            .map(|expiry| expiry.saturating_duration_since(now))
    }

    /// Object id for "Guild Explanation Portal" (RealmEye's reference sprite
    /// for the Guild Hall entrance, see realmeye.com/wiki/guild-hall). The
    /// actual `GuildHallPortal`-class object has no renderable texture in the
    /// local client assets, so this decorative Nexus portal (properly classed
    /// `Portal`, id `0x0748`) is used as the Guild Hall icon instead.
    const GUILD_HALL_PORTAL_OBJECT_ID: i32 = 1864;

    /// Object id for "Cloth Bazaar Portal" (`DisplayId` "Grand Bazaar",
    /// `Class` `Portal`, id `0x0750`) - the Grand Bazaar entrance portal, see
    /// realmeye.com/wiki/grand-bazaar. Hardcoded like the Guild Hall icon so
    /// it doesn't depend on the local asset extraction resolving the same
    /// display name.
    const BAZAAR_PORTAL_OBJECT_ID: i32 = 1872;

    /// Object ids for Oryx's non-joinable special areas (see
    /// `MapInfoPacket::is_non_joinable_special`). Matches the ids already used
    /// for dungeon-entry portal sprites in `dungeon_portals.rs`, except Oryx's
    /// Sanctuary which has no hardcoded entry there.
    const ORYX_CASTLE_PORTAL_OBJECT_ID: i32 = 3465;
    const ORYX_CHAMBER_PORTAL_OBJECT_ID: i32 = 1588;
    const ORYX_SANCTUARY_PORTAL_OBJECT_ID: i32 = 6218;
    const WINE_CELLAR_PORTAL_OBJECT_ID: i32 = 578;

    /// Location icon shown to the left of the Server.World status text.
    /// Vault/Guild Hall/Grand Bazaar/Pet Yard/Daily Quest Room
    /// (Tinkerer) get their own sprite, Nexus and an active realm resolve to
    /// their portal sprites, and unrecognized locations show nothing.
    fn location_icon(&self) -> Option<TabIconSprite> {
        // Actively in a realm: show the realm portal regardless of the raw
        // display name (varies by realm type).
        if self.realm_score.is_some() && !self.is_score_stale {
            return Some(TabIconSprite::ObjectId(1796)); // Realm portal (gray arch)
        }

        let name = self.location_name.trim().to_lowercase();
        // The server's actual display name is the single word "Guildhall" (no
        // space), and gains a level suffix as it's upgraded ("Guildhall II"/
        // "III"/"IV") or is renamed at max level ("Grand Guildhall"). Compare
        // whitespace-insensitively so "Guild Hall" variants also match, since
        // every level uses the same portal sprite.
        let compact_name: String = name.chars().filter(|c| !c.is_whitespace()).collect();
        if compact_name.contains("guildhall") {
            return Some(TabIconSprite::ObjectId(Self::GUILD_HALL_PORTAL_OBJECT_ID));
        }
        // The Bazaar has gone by several display names over time ("Bazaar",
        // "Cloth Bazaar", "Grand Bazaar") plus "Marketplace"; all resolve to
        // the same portal (mirrors the name set in
        // `MapInfoPacket::is_hub_or_realm`).
        if compact_name.contains("bazaar") || compact_name == "marketplace" {
            return Some(TabIconSprite::ObjectId(Self::BAZAAR_PORTAL_OBJECT_ID));
        }
        // Oryx's non-joinable special areas: reduce to alphanumerics only
        // (mirrors `MapInfoPacket::is_non_joinable_special`) so both the
        // possessive ("Oryx's Castle") and plain ("Oryx Castle") spellings of
        // the display name match.
        let canon: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        match canon.as_str() {
            "oryxscastle" | "oryxcastle" => {
                return Some(TabIconSprite::ObjectId(Self::ORYX_CASTLE_PORTAL_OBJECT_ID))
            }
            "oryxschamber" | "oryxchamber" => {
                return Some(TabIconSprite::ObjectId(Self::ORYX_CHAMBER_PORTAL_OBJECT_ID))
            }
            "oryxssanctuary" | "oryxsanctuary" => {
                return Some(TabIconSprite::ObjectId(
                    Self::ORYX_SANCTUARY_PORTAL_OBJECT_ID,
                ))
            }
            "winecellar" => {
                return Some(TabIconSprite::ObjectId(Self::WINE_CELLAR_PORTAL_OBJECT_ID))
            }
            _ => {}
        }

        match name.as_str() {
            "vault" => Some(TabIconSprite::ObjectId(1824)), // Vault Portal
            "pet yard" => Some(TabIconSprite::SheetIndex("lofiObj3", 0x0d)), // Pet Yard Portal
            "daily quest room" => Some(TabIconSprite::ObjectId(5974)), // Daily Quest Portal (Tinkerer)
            "nexus" => get_dungeon_portal_map()
                .get_portal_id("Nexus")
                .map(TabIconSprite::ObjectId),
            // Inside a dungeon: show that dungeon's own portal sprite (the same
            // id resolved for its feed entry), so the location icon matches the
            // map. A realm close overrides this with the countdown timer, which
            // is handled ahead of this in the header render.
            _ if self.currently_in_dungeon => get_dungeon_portal_map()
                .get_portal_id(&self.location_name)
                .map(TabIconSprite::ObjectId),
            _ => None,
        }
    }

    pub fn is_in_dungeon(&self) -> bool {
        self.currently_in_dungeon
    }

    /// Whether the DM-in-live-feed feature is enabled (mirrors setting).
    pub fn dm_display_enabled(&self) -> bool {
        self.show_dms
    }

    /// Push a dungeon-entered entry.
    ///
    /// `dungeon_name` is the raw map display name (localization keys are
    /// normalized for display). Consecutive entries for the same dungeon
    /// instance (same `fp` seed, name, modifiers, grade) are suppressed to
    /// avoid duplicates on repeated MapInfo packets. A new instance of the
    /// same dungeon type (different `fp`) always produces a fresh entry.
    pub fn push_dungeon(
        &mut self,
        fp: i32,
        dungeon_name: &str,
        modifier_tokens: &[String],
        grade: Option<String>,
    ) {
        // Log any modifier tokens that don't resolve in the bonus table so the
        // table can be extended with their exact raw form.
        let unmatched: Vec<&String> = modifier_tokens
            .iter()
            .filter(|t| realmhound_core::dungeon_modifiers::modifier_info(t).is_none())
            .collect();
        if !unmatched.is_empty() {
            tracing::info!(
                "[DUNGEON_MOD_UNMATCHED] dungeon='{}' tokens={:?}",
                dungeon_name,
                unmatched
            );
        }

        let portal_id = get_dungeon_portal_map().get_portal_id(dungeon_name);
        let display_name = format_dungeon_name(dungeon_name);
        // Anchor the countdown to the observed portal spawn (always < 30s ago)
        // when known, else fall back to entry time. A pending anchor older than
        // the join window can't belong to the portal we just entered (it lives
        // only 30s), so treat it as unknown. Without a real spawn anchor the
        // window is only estimated (e.g. joined via a party call).
        let fresh_spawn = self
            .pending_portal_spawn
            .filter(|spawn| spawn.elapsed() < DUNGEON_JOIN_WINDOW);
        let estimated = fresh_spawn.is_none();
        let anchor = fresh_spawn.unwrap_or_else(std::time::Instant::now);
        let params = self.dungeon_callout_params();
        let mut entry = DungeonEntry::new(
            display_name,
            portal_id,
            modifier_tokens,
            grade,
            &params,
            self.server_name.clone(),
            self.realm_name.clone(),
            anchor,
            estimated,
        );
        entry.map_seed = fp;
        let signature = (
            fp,
            entry.dungeon_name.clone(),
            entry.modifiers.clone(),
            entry.grade.clone(),
        );

        // Suppress repeated MapInfo packets for the same dungeon.
        if self.last_dungeon_signature.as_ref() == Some(&signature) {
            return;
        }

        // Genuinely new dungeon: consume the spawn anchor and expire any prior
        // still-active dungeon entry (you can't call a dungeon you just left).
        self.pending_portal_spawn = None;
        self.deactivate_active_dungeons();

        self.last_dungeon_signature = Some(signature);
        // A realm at 100% is closing, so a dungeon entered from within it can't
        // be joined -- expire it immediately. Once the realm has fully closed
        // and the Oryx endgame has begun (`realm_closed`), a new dungeon can
        // only be a key-opened instance inside the castle areas, which stays
        // joinable. The natural Oryx areas (Castle/Chamber/Wine
        // Cellar/Sanctuary) never reach here -- they aren't dungeon feed entries.
        if !self.realm_closed && self.get_score_percent().map_or(false, |p| p >= 100) {
            entry.deactivate();
        }
        self.push_entry(FeedEntry::Dungeon(entry));
    }

    /// Force-expire every still-active dungeon entry in the feed. Called when the
    /// player changes maps, since they can no longer join the dungeon they left.
    /// Also settles the live run timer to a fallback (time-in-dungeon) value for
    /// any entry that has not already been frozen by the authoritative
    /// CombatManager freeze (e.g. non-groupable dungeons it does not track).
    fn deactivate_active_dungeons(&mut self) {
        let now = std::time::Instant::now();
        for entry in self.entries.iter_mut() {
            if let FeedEntry::Dungeon(d) = entry {
                d.deactivate();
                if d.frozen_elapsed_ms.is_none() {
                    d.frozen_elapsed_ms = Some(now.duration_since(d.entered_at).as_millis() as i64);
                }
            }
        }
    }

    /// Settle the live dungeon run timer for the row matching `map_seed` to the
    /// CombatManager's committed per-run elapsed. Overrides any earlier fallback
    /// freeze so the authoritative (entry -> final boss death) value wins.
    pub fn apply_dungeon_freeze(&mut self, map_seed: i32, elapsed_ms: i64) {
        for entry in self.entries.iter_mut().rev() {
            if let FeedEntry::Dungeon(d) = entry {
                if d.map_seed == map_seed {
                    d.frozen_elapsed_ms = Some(elapsed_ms.max(0));
                    return;
                }
            }
        }
    }

    /// Forget the last dungeon signature (call when leaving a dungeon) so that
    /// re-entering the same dungeon produces a new feed entry.
    pub fn clear_dungeon_signature(&mut self) {
        self.last_dungeon_signature = None;
    }

    /// Clear all entries from the feed.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.crystal_pin = None;
    }

    /// Add an active encounter (realm boss/event in view).
    pub fn add_encounter(&mut self, object_id: i32, object_type: u16, name: String) {
        if !ENABLE_ACTIVE_ENCOUNTERS {
            return;
        }
        // Only insert if not already present (preserve original timestamp)
        self.active_encounters
            .entry(object_id)
            .or_insert(ActiveEncounter {
                object_type,
                name,
                added_at: std::time::Instant::now(),
            });
    }

    /// Remove an encounter when it leaves view (drops).
    pub fn remove_encounter(&mut self, object_id: i32) {
        self.active_encounters.remove(&object_id);
    }

    /// Clear all active encounters (on map change).
    pub fn clear_encounters(&mut self) {
        self.active_encounters.clear();
    }

    /// Set a status message that will display briefly.
    fn set_status(&mut self, message: &str) {
        self.status_message = Some((message.to_string(), std::time::Instant::now()));
    }

    /// Copy text to clipboard.
    fn copy_to_clipboard(ctx: &egui::Context, text: &str) {
        ctx.copy_text(text.to_string());
    }

    /// Format timestamp for display.
    fn format_timestamp(timestamp: DateTime<Local>) -> String {
        timestamp.format("%H:%M:%S").to_string()
    }

    /// Render context-aware NPC quick-clip buttons and user-custom buttons.
    #[allow(deprecated)]
    fn render_quick_clip_buttons(
        &mut self,
        ui: &mut Ui,
        sprite_renderer: &mut SpriteRenderer,
        shadcn: &Shadcn,
    ) {
        // Thessal (Ocean Trench only)
        if self.location_name == "Ocean Trench" {
            let resp = Self::render_npc_button(shadcn, ui, sprite_renderer, 1902, "Thessal");
            if resp.clicked() {
                let msg = "He lives and reigns and conquers the world";
                Self::copy_to_clipboard(ui.ctx(), msg);
                self.set_status(&format!("Copied: {}", msg));
            }
        }

        // Umi (Moonlight Village only)
        if self.location_name == "Moonlight Village" {
            let umi_id = ui.make_persistent_id("umi_popup");
            let umi_resp = Self::render_npc_button(shadcn, ui, sprite_renderer, 20601, "Umi");
            if umi_resp.clicked() {
                ui.memory_mut(|mem| mem.toggle_popup(umi_id));
            }

            egui::popup_below_widget(
                ui,
                umi_id,
                &umi_resp,
                egui::PopupCloseBehavior::CloseOnClick,
                |ui| {
                    ui.set_min_width(280.0);
                    let questions = [
                        ("Did you have a favorite dancer?", "Carosburg"),
                        ("What Folktales do you know about?", "The Happy Prince"),
                        ("What kind of foods do you like to eat?", "Mushroom"),
                    ];
                    for (question, answer) in &questions {
                        if ui.button(*question).clicked() {
                            Self::copy_to_clipboard(ui.ctx(), answer);
                            self.set_status(&format!("Copied: {}", answer));
                        }
                    }
                },
            );
        }

        // Custom buttons
        let mut to_delete: Option<usize> = None;
        for idx in 0..self.custom_clip_buttons.len() {
            let is_empty = self.custom_clip_buttons[idx].message.is_empty();
            let label = if self.custom_clip_buttons[idx].label.is_empty() {
                format!("Clip {}", idx + 1)
            } else {
                self.custom_clip_buttons[idx].label.clone()
            };
            let hover = if is_empty {
                "Right-click to configure".to_string()
            } else {
                format!(
                    "Click to copy: {}\nRight-click to edit",
                    self.custom_clip_buttons[idx].message
                )
            };

            let popup_id = ui.make_persistent_id(format!("clip_edit_{idx}"));
            let resp = shadcn.button_compact(ui, &label);
            if resp.clicked() && !is_empty {
                let msg = self.custom_clip_buttons[idx].message.clone();
                Self::copy_to_clipboard(ui.ctx(), &msg);
                self.set_status(&format!("Copied: {}", msg));
            }
            if resp.secondary_clicked() {
                ui.memory_mut(|mem| mem.toggle_popup(popup_id));
            }
            let resp = resp.hover_tip(hover);
            self.render_clip_edit_popup(ui, popup_id, &resp, idx, &mut to_delete, shadcn);
        }

        if let Some(idx) = to_delete {
            self.custom_clip_buttons.remove(idx);
            self.pending_clip_buttons_save = true;
        }

        // Add a new custom button. Capped at 10; the "+" disappears at the cap
        // and returns once a button is deleted.
        const MAX_CUSTOM_CLIP_BUTTONS: usize = 10;
        if self.custom_clip_buttons.len() < MAX_CUSTOM_CLIP_BUTTONS
            && shadcn
                .button_compact(ui, "+")
                .hover_tip("Add a custom clipboard button (10 max)")
                .clicked()
        {
            self.custom_clip_buttons.push(Default::default());
            self.pending_clip_buttons_save = true;
        }
    }

    /// Render the right-click edit popup for a custom clip button.
    #[allow(deprecated)]
    fn render_clip_edit_popup(
        &mut self,
        ui: &mut Ui,
        popup_id: egui::Id,
        below: &egui::Response,
        idx: usize,
        to_delete: &mut Option<usize>,
        shadcn: &Shadcn,
    ) {
        egui::popup_below_widget(
            ui,
            popup_id,
            below,
            egui::PopupCloseBehavior::CloseOnClickOutside,
            |ui| {
                ui.set_min_width(250.0);
                ui.label(RichText::new(format!("Custom Button {}", idx + 1)).strong());
                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    ui.label("Label:");
                    let resp = shadcn.text_edit_counted(
                        ui,
                        &mut self.custom_clip_buttons[idx].label,
                        10,
                        80.0,
                        Some("max 10"),
                    );
                    if resp.changed() {
                        self.pending_clip_buttons_save = true;
                    }
                });

                ui.horizontal(|ui| {
                    ui.label("Message:");
                    let resp = shadcn.text_edit_counted(
                        ui,
                        &mut self.custom_clip_buttons[idx].message,
                        128,
                        200.0,
                        Some("max 128"),
                    );
                    if resp.changed() {
                        self.pending_clip_buttons_save = true;
                    }
                });

                ui.add_space(4.0);
                if ui.button("🗑 Delete button").clicked() {
                    *to_delete = Some(idx);
                    ui.memory_mut(|mem| mem.close_popup(popup_id));
                }
            },
        );
    }

    /// Render an icon button (styled like the custom clipboard buttons) with a
    /// sprite + label. Returns the Response for popup anchoring.
    fn render_npc_button(
        shadcn: &Shadcn,
        ui: &mut Ui,
        sprite_renderer: &mut SpriteRenderer,
        object_id: i32,
        label: &str,
    ) -> egui::Response {
        let (response, icon_rect) = shadcn.icon_button(ui, label);
        sprite_renderer.draw_sprite_in_rect(ui, object_id, icon_rect);
        response
    }
    /// Render the live feed panel UI.
    pub fn render<F, G>(
        &mut self,
        ui: &mut Ui,
        sprite_renderer: &mut SpriteRenderer,
        get_label: F,
        get_appearance: G,
        shadcn: &Shadcn,
    ) where
        F: Fn(i32) -> Option<String>,
        G: Fn(i32) -> Option<(i32, u32, u32)>,
    {
        // Entry visibility under the current loot filters (loot bags can be
        // hidden; tracking is unaffected).
        let mut visible_count = self
            .entries
            .iter()
            .filter(|e| self.entry_passes_filter(e))
            .count();
        let mut hidden_count = self.entries.len() - visible_count;

        // Loot filter bar (line 1): hides matching bags from the feed display
        // while loot tracking continues. Snapshot to know when to persist.
        let filters_before = self.loot_filters.clone();
        let prop_before = self.prop_filters;
        let dm_filter_before = self.show_dms_filter;
        let mut status_clicked = false;
        shadcn.header_band(ui, shadcn.secondary_header_fill(), |ui| {
        // Line 1: loot filter bar. Only rendered when loot-drop entries are
        // shown; otherwise this whole row (and its separator) collapses instead
        // of lingering with a lone control. The DMs filter now lives in the
        // header row below next to the other checkboxes.
        if self.show_loot_drops {
            ui.horizontal_wrapped(|ui| {
                ui.set_min_height(28.0);
                render_filter_bar(ui, sprite_renderer, &mut self.loot_filters, &mut self.prop_filters, shadcn);
            });
            shadcn.full_width_separator(ui);
        }

        // Taskbar is now rendered app-wide below the Widget Bar (on every tab),
        // so the Live Feed page no longer draws it here.

        // Header: realm status replaces the old "Live Feed" title (clickable to
        // copy a /p command when in a realm), followed by the callout toggles
        // and the right-hand Clear / Auto-scroll / entry-count cluster.
        let mut clear_clicked = false;
        let mut auto_scroll = self.auto_scroll;
        egui::containers::Sides::new()
            .shrink_left()
            .spacing(8.0)
            .height(Shadcn::BAND_ROW_HEIGHT)
            .show(
                ui,
                |ui| {
                    // Left side wraps: status + callout toggles + quick clip
                    // buttons flow left-to-right and fold onto new lines once the
                    // width Sides leaves for them (after reserving the right
                    // cluster) is full.
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), Shadcn::BAND_ROW_HEIGHT),
                        egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
                        |ui| {
                        // Fixed band height with content vertically centered (same as
                        // band_row) so the toolbar's top/bottom margins stay symmetric
                        // and aligned with the sibling bands. set_min_height would
                        // instead top-pin content, leaving uneven margins. Wrap is on
                        // so clip buttons fold onto a new line once the row is full.
                        ui.spacing_mut().item_spacing.y = 4.0;

            // Realm/Nexus/Vault location icon, reserved to the
            // left of the status text. Shows nothing when there's no location
            // data yet (mirrors render_status_inline's own fallback).
            let show_location = !self.location_name.is_empty() || self.server_name.is_some();
            if show_location {
                ui.add_space(6.0);
                let (icon_rect, _) = ui.allocate_exact_size(
                    egui::vec2(TAB_ICON_SIZE, TAB_ICON_SIZE),
                    egui::Sense::hover(),
                );
                if ui.is_rect_visible(icon_rect) {
                    // Castle teleport countdown after a realm close: replace
                    // the realm portal icon with the same green -> red
                    // depleting-pie timer used for dungeon join countdowns,
                    // then let the portal icon reappear once it elapses.
                    let now = std::time::Instant::now();
                    let castle_remaining = self.castle_timer_remaining(now);
                    if let Some(remaining) = castle_remaining {
                        if remaining.is_zero() {
                            self.castle_timer_expires_at = None;
                            sprite_renderer.render_icon(ui, self.location_icon(), icon_rect);
                        } else {
                            ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
                            let fraction = remaining.as_secs_f32() / CASTLE_TELEPORT_WINDOW.as_secs_f32();
                            let secs = remaining.as_secs_f32().ceil() as u64;
                            Self::draw_countdown_circle(
                                ui,
                                icon_rect.center(),
                                icon_rect.width() / 2.0,
                                fraction,
                                secs,
                                false,
                            );
                        }
                    } else {
                        sprite_renderer.render_icon(ui, self.location_icon(), icon_rect);
                    }
                }
                ui.add_space(6.0);
            }

            status_clicked = self.render_status_inline(ui);

            // Callout toggles: kept in the tab header (not Settings)
            // so they can be flipped on the fly when switching servers or calling
            // for the realm instead of the party. Persisted via an AppAction.
            ui.add_space(12.0);
            ui.separator();
            ui.add_space(6.0);
            if ui
                .checkbox(&mut self.call_for_party, "Call for the party")
                .hover_tip("Prefix callouts with /p so they post to your party. Turn off to copy the callout without /p.")
                .changed()
            {
                self.pending_header_settings_save = true;
            }
            if ui
                .checkbox(&mut self.include_server_name, "Include server name in callouts")
                .hover_tip("Prefix callouts with the short server name (e.g. USMW2 davy j).")
                .changed()
            {
                self.pending_header_settings_save = true;
            }
            if ui
                .checkbox(&mut self.include_realm_name, "Include realm name in callouts")
                .hover_tip("Include the realm name in callouts (e.g. USMW2 Medusa rav rot j).")
                .changed()
            {
                self.pending_header_settings_save = true;
            }
            // DMs feed filter. Only shown when direct messages are enabled in
            // settings; when disabled the checkbox is removed entirely.
            if self.show_dms
                && ui
                    .checkbox(
                        &mut self.show_dms_filter,
                        RichText::new("DMs").color(ChatType::Whisper.color()).size(12.0),
                    )
                    .hover_tip("Show direct messages in the feed")
                    .changed()
            {
                self.pending_filter_settings_save = true;
            }

                        // Quick clipboard buttons flow inline after the toggles,
                        // filling leftover toolbar width first and wrapping onto
                        // new lines only once the row is full.
                        ui.add_space(6.0);
                        ui.separator();
                        ui.add_space(2.0);
                        self.render_quick_clip_buttons(ui, sprite_renderer, shadcn);
                    });
                },
                |ui| {
                    // Right cluster: Clear / Auto-scroll / Entries (added
                    // right-to-left by Sides). Uses locals so this closure does
                    // not borrow `self`, avoiding a double mutable borrow with the
                    // left side.
                    if shadcn.button_compact(ui, "🗑 Clear").clicked() {
                        clear_clicked = true;
                    }
                    ui.checkbox(&mut auto_scroll, "Auto-scroll");
                    if hidden_count > 0 {
                        ui.label(format!("Entries: {} ({} hidden)", visible_count, hidden_count));
                    } else {
                        ui.label(format!("Entries: {}", visible_count));
                    }
                },
            );

        self.auto_scroll = auto_scroll;
        if clear_clicked {
            self.clear();
            self.set_status("Feed cleared");
            visible_count = 0;
            hidden_count = 0;
        }

        });

        // Recompute counts after the toolbar so DMs and loot-filter toggles from
        // this frame are reflected in the entry count and empty-state.
        if self.loot_filters != filters_before
            || self.prop_filters != prop_before
            || self.show_dms_filter != dm_filter_before
        {
            self.pending_filter_settings_save = true;
            visible_count = self
                .entries
                .iter()
                .filter(|e| self.entry_passes_filter(e))
                .count();
        }

        if status_clicked {
            // Status was clicked - copy /p command to clipboard
            let command = self.build_copy_command();
            Self::copy_to_clipboard(ui.ctx(), &command);
            self.set_status(&format!("Copied: {}", command));
        }

        // Status message (temporary)
        if let Some((msg, time)) = &self.status_message {
            if time.elapsed().as_secs() < 3 {
                ui.label(RichText::new(msg).color(Color32::GREEN));
            } else {
                self.status_message = None;
            }
        }

        // Active Encounters section (fixed at top, not scrolling)
        if ENABLE_ACTIVE_ENCOUNTERS && !self.active_encounters.is_empty() {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Active Encounters:")
                        .size(12.0)
                        .color(Color32::from_rgb(255, 200, 100)),
                );
                ui.add_space(8.0);

                // Sort encounters by time added (newest first = on left)
                let mut encounters: Vec<_> = self.active_encounters.values().collect();
                encounters.sort_by(|a, b| b.added_at.cmp(&a.added_at));

                // Deduplicate by name (multi-segment bosses have same name)
                let mut seen_names: std::collections::HashSet<&str> =
                    std::collections::HashSet::new();
                for encounter in encounters {
                    if seen_names.insert(&encounter.name) {
                        // Render sprite
                        let sprite_size = 24.0;
                        let (rect, response) = ui.allocate_exact_size(
                            egui::vec2(sprite_size, sprite_size),
                            egui::Sense::hover(),
                        );

                        if ui.is_rect_visible(rect) {
                            // Background
                            ui.painter()
                                .rect_filled(rect, 3.0, Color32::from_rgb(50, 40, 40));
                            // Draw sprite using object type ID
                            sprite_renderer.draw_sprite_in_rect(
                                ui,
                                encounter.object_type as i32,
                                rect,
                            );
                        }

                        // Tooltip with name
                        response.hover_tip(&encounter.name);

                        ui.add_space(2.0);
                    }
                }
            });
            shadcn.separator(ui);
        }

        // Pinned end-of-cycle warnings: rendered fixed here, above
        // the scrolling feed and before any empty-state early returns so they
        // remain visible even with no feed entries.
        self.render_pinned_warnings(ui, sprite_renderer, shadcn);

        // Pinned Crystal Prisoner callout (Mysterious Crystal setpiece). Gated
        // by the "Realm event bosses" toggle like other boss callouts; its
        // lifecycle is timestamp-based, so it re-evaluates correctly when shown.
        if self.show_boss_calls {
            self.render_crystal_pin(ui, sprite_renderer, shadcn);
        }

        // Every Live Feed notification category disabled and nothing visible:
        // explain that it's a settings choice rather than showing the generic
        // "No activity yet". Guarded on `visible_count == 0` so always-pass
        // entries (kick alerts) and any enabled warnings that do fire are never
        // hidden by this branch. Warning/dust toggles live in a separate card
        // and are intentionally excluded here so disabling the notification
        // categories is enough to surface this message.
        let all_suppressed = !self.show_dms
            && !self.show_who_roster
            && !self.show_key_pops
            && !self.show_area_unlocks
            && !self.show_dungeon_entries
            && !self.show_boss_calls
            && !self.show_loot_drops;
        if all_suppressed && visible_count == 0 {
            crate::panels::empty_state(
                ui,
                "All notifications are currently suppressed by Live Feed settings.",
                Some("Enable categories under Settings \u{2192} Live Feed to see activity here."),
            );
            return;
        }

        // Empty state
        if self.entries.is_empty() {
            crate::panels::empty_state(
                ui,
                "No activity yet",
                Some("Loot drops, boss callouts, and party alerts will appear here."),
            );
            if self.awaiting_first_map {
                ui.add_space(10.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new(
                            "If you launched RealmHound mid-game, capture begins on your \
                             next map change (entering a realm, dungeon, or Nexus).",
                        )
                        .size(12.0)
                        .color(Color32::from_rgb(220, 190, 90)),
                    );
                });
            }
            return;
        }

        // Everything is hidden by the active filters.
        if visible_count == 0 {
            crate::panels::empty_state(
                ui,
                "All entries hidden by filters",
                Some("Adjust the filter bar above to show loot. Hidden bags are still tracked."),
            );
            return;
        }

        // Scrollable feed (newest at top)
        let mut scroll = ScrollArea::vertical().auto_shrink([false, false]);

        // Only force scroll to top when a new entry was added (not every frame)
        // This allows manual scrolling while auto-scroll is enabled
        if self.needs_scroll_to_top {
            scroll = scroll.vertical_scroll_offset(0.0);
            self.needs_scroll_to_top = false;
        }

        scroll.show(ui, |ui| {
            let now = std::time::Instant::now();
            let mut name_to_copy: Option<String> = None;
            let mut boss_callout_to_copy: Option<String> = None;
            let mut dungeon_callout_to_copy: Option<String> = None;
            let mut alert_to_dismiss: Option<DateTime<Local>> = None;
            let mut dm_reply_to_copy: Option<String> = None;
            let mut dm_view_chat: Option<String> = None;
            let mut warning_to_copy: Option<String> = None;
            let mut who_names_to_copy: Option<Vec<String>> = None;
            let mut callout_to_copy: Option<String> = None;

            for entry in self.entries.iter() {
                if !self.entry_passes_filter(entry) {
                    continue;
                }
                match entry {
                    FeedEntry::Loot(loot) => {
                        // Reuse the Loot History card renderer in read-only mode
                        // (no select checkbox, sprites non-clickable). The small
                        // left bleed lines the card border up with the outlined
                        // dungeon/boss rows above it.
                        let record = loot.to_drop_record();
                        crate::panels::loot::LootPanel::render_loot_card(
                            ui,
                            &record,
                            sprite_renderer,
                            shadcn,
                            &get_label,
                            &get_appearance,
                            None,
                            false,
                            false,
                            2.0,
                        );
                    }
                    FeedEntry::BossCall(boss) => {
                        if self.render_boss_entry(ui, boss, sprite_renderer) {
                            let body = self.build_event_body(boss);
                            boss_callout_to_copy = Some(self.assemble_callout(
                                &body,
                                boss.server_name.as_deref(),
                                boss.realm_name.as_deref(),
                                self.event_join_position,
                            ));
                        }
                    }
                    FeedEntry::KickAlert(alert) => {
                        let (name, dismiss) = self.render_kick_alert(ui, alert, shadcn);
                        if let Some(n) = name {
                            name_to_copy = Some(n);
                        }
                        if dismiss {
                            alert_to_dismiss = Some(alert.timestamp);
                        }
                    }
                    FeedEntry::Dungeon(dungeon) => {
                        if let Some(callout) =
                            self.render_dungeon_entry(ui, dungeon, sprite_renderer, now)
                        {
                            dungeon_callout_to_copy = Some(callout);
                        }
                    }
                    FeedEntry::DustFull(dust) => {
                        self.render_dust_full_entry(ui, dust, sprite_renderer);
                    }
                    FeedEntry::DirectMessage(dm) => {
                        let action = self.render_dm_entry(ui, dm, sprite_renderer);
                        match action {
                            DmAction::CopyReply(name) => dm_reply_to_copy = Some(name),
                            DmAction::ViewChat(name) => dm_view_chat = Some(name),
                            DmAction::None => {}
                        }
                    }
                    FeedEntry::Warning(warning) => {
                        if self.render_warning_entry(ui, warning) {
                            warning_to_copy = Some(warning.text.clone());
                        }
                    }
                    FeedEntry::WhoRoster(roster) => {
                        if self.render_who_roster_entry(ui, roster) {
                            who_names_to_copy = Some(roster.names.clone());
                        }
                    }
                    FeedEntry::KeyPop(keypop) => {
                        if self.render_key_pop_entry(ui, keypop, sprite_renderer) {
                            callout_to_copy = keypop.callout();
                        }
                    }
                    FeedEntry::AreaUnlock(unlock) => {
                        if self.render_area_unlock_entry(ui, unlock, sprite_renderer) {
                            callout_to_copy = Some(unlock.callout());
                        }
                    }
                }
            }

            // Handle copy after iteration to avoid borrow issues
            if let Some(name) = name_to_copy {
                let command = format!("/pkick {}", name);
                Self::copy_to_clipboard(ui.ctx(), &command);
                self.set_status(&format!("Copied: {}", command));
            }

            // Handle boss callout copy
            if let Some(callout) = boss_callout_to_copy {
                Self::copy_to_clipboard(ui.ctx(), &callout);
                self.set_status(&format!("Copied: {}", callout));
            }

            // Handle dungeon callout copy (row clicked). The returned value is
            // the fully assembled callout with /p prefix, server name, realm name,
            // and join marker per settings.
            if let Some(command) = dungeon_callout_to_copy {
                Self::copy_to_clipboard(ui.ctx(), &command);
                self.set_status(&format!("Copied: {}", command));
            }

            // Handle warning copy (click on copyable warning entry)
            if let Some(text) = warning_to_copy {
                Self::copy_to_clipboard(ui.ctx(), &text);
                self.set_status(&format!("Copied: {}", text));
            }

            // Handle /who roster copy (names only, one per line).
            if let Some(names) = who_names_to_copy {
                let joined = names.join("\n");
                Self::copy_to_clipboard(ui.ctx(), &joined);
                self.set_status(&format!("Copied {} player names", names.len()));
            }

            // Handle key-pop / area-unlock "Thanks ..." callout copy.
            if let Some(callout) = callout_to_copy {
                Self::copy_to_clipboard(ui.ctx(), &callout);
                self.set_status(&format!("Copied: {}", callout));
            }

            // Handle alert dismiss
            if let Some(ts) = alert_to_dismiss {
                self.entries.retain(|e| {
                    if let FeedEntry::KickAlert(alert) = e {
                        alert.timestamp != ts
                    } else {
                        true
                    }
                });
            }

            // Handle DM reply copy (left click)
            if let Some(name) = dm_reply_to_copy {
                let command = format!("/t {} ", name);
                Self::copy_to_clipboard(ui.ctx(), &command);
                self.set_status(&format!("Copied: {}", command));
            }

            // Handle DM view chat (right click -> context menu)
            if let Some(name) = dm_view_chat {
                self.pending_actions
                    .push(super::AppAction::ViewChatWith { sender: name });
            }
        });
    }

    /// Render a boss callout entry.
    /// Returns true if left-click copy was triggered.
    fn render_boss_entry(
        &self,
        ui: &mut Ui,
        boss: &BossCallEntry,
        sprite_renderer: &mut SpriteRenderer,
    ) -> bool {
        let sprite_size = 36.0;
        let outline_width = 2.0;
        let total_size = sprite_size + outline_width * 2.0;
        let mut clicked = false;
        let expired = boss.expired;
        let expired_grey = Color32::from_gray(120);

        // Allocate the entire row. Expired callouts can't be copied, so they
        // only sense hover (for the explanatory tooltip).
        let row_height = total_size + 4.0;
        let (row_rect, row_response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), row_height),
            if expired {
                egui::Sense::hover()
            } else {
                egui::Sense::click()
            },
        );

        // Highlight on hover
        if !expired && row_response.hovered() {
            ui.painter().rect_filled(
                row_rect,
                4.0,
                Color32::from_rgba_unmultiplied(255, 255, 255, 15),
            );
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        if row_response.clicked() {
            clicked = true;
        }

        // Layout content manually within the allocated rect
        let mut x = row_rect.left() + 10.0;
        let center_y = row_rect.center().y;

        // Timestamp (same style as loot entries)
        let time_str = Self::format_timestamp(boss.timestamp);
        let time_color = if expired { expired_grey } else { Color32::GRAY };
        let time_galley =
            ui.painter()
                .layout_no_wrap(time_str, egui::FontId::monospace(12.0), time_color);
        ui.painter().galley(
            egui::pos2(x, center_y - time_galley.rect.height() / 2.0),
            time_galley.clone(),
            time_color,
        );
        x += time_galley.rect.width() + 8.0;

        // Boss sprite with outline
        let sprite_rect = egui::Rect::from_center_size(
            egui::pos2(x + total_size / 2.0, center_y),
            egui::vec2(total_size, total_size),
        );

        // Draw white square outline (greyed once expired)
        let outline_color = if expired {
            expired_grey
        } else {
            Color32::WHITE
        };
        ui.painter().rect_stroke(
            sprite_rect,
            0.0,
            egui::Stroke::new(outline_width, outline_color),
            egui::StrokeKind::Outside,
        );

        // Draw sprite inside the outline
        let inner_sprite_rect = egui::Rect::from_min_size(
            sprite_rect.min + egui::vec2(outline_width, outline_width),
            egui::vec2(sprite_size, sprite_size),
        );
        if expired {
            sprite_renderer.draw_sprite_in_rect_tinted(
                ui,
                boss.boss_id,
                inner_sprite_rect,
                Color32::from_rgba_unmultiplied(255, 255, 255, 90),
            );
        } else {
            sprite_renderer.draw_sprite_in_rect(ui, boss.boss_id, inner_sprite_rect);
        }
        x += total_size + 6.0;

        // Boss name
        let name_color = if expired {
            expired_grey
        } else {
            Color32::from_rgb(255, 255, 100)
        };
        let boss_galley = ui.painter().layout_no_wrap(
            boss.boss_name.clone(),
            egui::FontId::proportional(18.0),
            name_color,
        );
        ui.painter().galley(
            egui::pos2(x, center_y - boss_galley.rect.height() / 2.0),
            boss_galley.clone(),
            name_color,
        );
        x += boss_galley.rect.width() + 8.0;

        // Server | Realm location info
        let location_parts: Vec<&str> = [boss.server_name.as_deref(), boss.realm_name.as_deref()]
            .into_iter()
            .flatten()
            .collect();

        if !location_parts.is_empty() {
            let location_text = format!("| {}", location_parts.join(" "));
            let loc_color = if expired {
                expired_grey
            } else {
                Color32::from_rgb(150, 150, 150)
            };
            let location_galley = ui.painter().layout_no_wrap(
                location_text,
                egui::FontId::proportional(14.0),
                loc_color,
            );
            ui.painter().galley(
                egui::pos2(x, center_y - location_galley.rect.height() / 2.0),
                location_galley,
                loc_color,
            );
        }

        // Clipboard callout preview, right-aligned green text (mirrors dungeon
        // entries). Shown only while the callout is still copyable; expired
        // callouts can no longer be posted so the preview is omitted.
        if !expired {
            let callout_color = Color32::from_rgb(150, 230, 150);
            let preview = self.assemble_callout(
                &self.build_event_body(boss),
                boss.server_name.as_deref(),
                boss.realm_name.as_deref(),
                self.event_join_position,
            );
            ui.painter().text(
                egui::pos2(row_rect.right() - 10.0, center_y),
                egui::Align2::RIGHT_CENTER,
                format!("\u{1F4CB} {}", preview),
                egui::FontId::monospace(12.0),
                callout_color,
            );
        }

        // Tooltip: live callouts are copyable; expired ones explain why not.
        if expired {
            row_response.hover_tip("Expired -- a newer wave has spawned in this realm");
        } else {
            row_response.hover_tip("Click to copy callout");
        }

        ui.add_space(4.0);
        clicked
    }

    /// Format an elapsed duration as a compact "ago" string: `45s`, `1m26s`,
    /// `12m03s`. Minutes are shown with zero-padded seconds; under a minute
    /// shows just seconds.
    fn format_ago(elapsed: std::time::Duration) -> String {
        let total = elapsed.as_secs();
        let mins = total / 60;
        let secs = total % 60;
        if mins > 0 {
            format!("{}m{:02}s", mins, secs)
        } else {
            format!("{}s", secs)
        }
    }

    /// Render the pinned Mysterious Crystal row (Crystal setpiece).
    /// Shows a live "last spoke Ns ago" timer, dimming the icon once the Crystal
    /// has been silent longer than [`CRYSTAL_STALE_AFTER`] (likely killed).
    fn render_crystal_pin(
        &mut self,
        ui: &mut Ui,
        sprite_renderer: &mut SpriteRenderer,
        shadcn: &Shadcn,
    ) {
        // Snapshot everything we need up front so we can freely mutate
        // `self.crystal_pin` afterwards (auto-expiry / unpin).
        let Some((elapsed, boss_id, server_name, realm_name)) =
            self.crystal_pin.as_ref().map(|p| {
                (
                    p.last_spoke.elapsed(),
                    p.boss_id,
                    p.server_name.clone(),
                    p.realm_name.clone(),
                )
            })
        else {
            return;
        };

        // Auto-expire: once stale for CRYSTAL_REMOVE_AFTER_STALE longer, fold the
        // sighting into the scrolling feed and drop the pin.
        if elapsed > CRYSTAL_STALE_AFTER + CRYSTAL_REMOVE_AFTER_STALE {
            if let Some(pin) = self.crystal_pin.take() {
                self.fold_crystal_pin_into_feed(&pin);
            }
            return;
        }

        let stale = elapsed > CRYSTAL_STALE_AFTER;

        let sprite_size = 36.0;
        let outline_width = 2.0;
        let total_size = sprite_size + outline_width * 2.0;
        let row_height = total_size + 4.0;
        let accent = Color32::from_rgb(120, 210, 235);

        ui.add_space(3.0);
        shadcn.full_width_separator(ui);
        ui.add_space(3.0);
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), row_height),
            egui::Sense::click(),
        );

        // Clickable body: highlight + pointer cursor so it reads as copy-able.
        if response.hovered() {
            ui.painter().rect_filled(
                rect,
                4.0,
                Color32::from_rgba_unmultiplied(255, 255, 255, 10),
            );
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        let center_y = rect.center().y;
        let mut x = rect.left() + 6.0;

        // Pin glyph in the top-left corner -- click to unpin (keeps the sighting
        // in the scrolling feed like any other callout).
        let pin_size = 20.0;
        let pin_rect =
            egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(pin_size, pin_size));
        let pin_resp = ui.interact(
            pin_rect,
            response.id.with("crystal_unpin"),
            egui::Sense::click(),
        );
        let pin_hovered = pin_resp.hovered();
        if pin_hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        ui.painter().text(
            pin_rect.center(),
            egui::Align2::CENTER_CENTER,
            "📌",
            egui::FontId::proportional(15.0),
            if pin_hovered {
                Color32::WHITE
            } else {
                Color32::from_rgb(170, 170, 170)
            },
        );
        pin_resp
            .clone()
            .on_hover_text("Unpin -- keep this sighting in the Live Feed log");
        if pin_resp.clicked() {
            if let Some(pin) = self.crystal_pin.take() {
                self.fold_crystal_pin_into_feed(&pin);
            }
            return;
        }
        x += pin_size + 4.0;

        // Crystal sprite with a white outline (dimmed when stale).
        let sprite_rect = egui::Rect::from_center_size(
            egui::pos2(x + total_size / 2.0, center_y),
            egui::vec2(total_size, total_size),
        );
        let outline_color = if stale {
            Color32::from_rgb(110, 110, 110)
        } else {
            Color32::WHITE
        };
        ui.painter().rect_stroke(
            sprite_rect,
            0.0,
            egui::Stroke::new(outline_width, outline_color),
            egui::StrokeKind::Outside,
        );
        let inner_sprite_rect = egui::Rect::from_min_size(
            sprite_rect.min + egui::vec2(outline_width, outline_width),
            egui::vec2(sprite_size, sprite_size),
        );
        if stale {
            sprite_renderer.draw_sprite_in_rect_tinted(
                ui,
                boss_id,
                inner_sprite_rect,
                Color32::from_rgba_unmultiplied(255, 255, 255, 90),
            );
        } else {
            sprite_renderer.draw_sprite_in_rect(ui, boss_id, inner_sprite_rect);
        }
        x += total_size + 8.0;

        // Name.
        let name_color = if stale {
            Color32::from_rgb(150, 150, 150)
        } else {
            accent
        };
        let name_galley = ui.painter().layout_no_wrap(
            "Mysterious Crystal".to_string(),
            egui::FontId::proportional(18.0),
            name_color,
        );
        ui.painter().galley(
            egui::pos2(x, center_y - name_galley.rect.height() / 2.0),
            name_galley.clone(),
            name_color,
        );
        x += name_galley.rect.width() + 8.0;

        // Location: | server realm
        let location_parts: Vec<&str> = [server_name.as_deref(), realm_name.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        if !location_parts.is_empty() {
            let location_text = format!("| {}", location_parts.join(" "));
            let loc_galley = ui.painter().layout_no_wrap(
                location_text,
                egui::FontId::proportional(14.0),
                Color32::from_rgb(150, 150, 150),
            );
            ui.painter().galley(
                egui::pos2(x, center_y - loc_galley.rect.height() / 2.0),
                loc_galley.clone(),
                Color32::from_rgb(150, 150, 150),
            );
            x += loc_galley.rect.width() + 10.0;
        }

        // Live "last spoke Ns ago" timer, inline after the location.
        let ago_color = if stale {
            Color32::from_rgb(120, 120, 120)
        } else {
            Color32::from_rgb(180, 180, 180)
        };
        let ago_text = format!("{} ago", Self::format_ago(elapsed));
        ui.painter().text(
            egui::pos2(x, center_y),
            egui::Align2::LEFT_CENTER,
            ago_text,
            egui::FontId::proportional(14.0),
            ago_color,
        );

        // Assemble the same `/p cry j` callout the folded feed entry produces,
        // used both for the green preview and the click-to-copy action.
        let entry = BossCallEntry::new(
            boss_id,
            "Mysterious Crystal".to_string(),
            server_name.clone(),
            realm_name.clone(),
        );
        let callout = self.assemble_callout(
            &self.build_event_body(&entry),
            server_name.as_deref(),
            realm_name.as_deref(),
            self.event_join_position,
        );

        // Right-aligned green callout preview, mirroring boss-call feed entries
        // so the pin reads as copy-able and shows exactly what will be copied.
        let preview_color = if stale {
            Color32::from_rgb(110, 150, 110)
        } else {
            Color32::from_rgb(150, 230, 150)
        };
        ui.painter().text(
            egui::pos2(rect.right() - 10.0, center_y),
            egui::Align2::RIGHT_CENTER,
            format!("\u{1F4CB} {}", callout),
            egui::FontId::monospace(12.0),
            preview_color,
        );

        // Click the row body (the pin glyph handles its own click above and
        // returns early) to copy the Crystal callout.
        if response.clicked() {
            Self::copy_to_clipboard(ui.ctx(), &callout);
            self.set_status(&format!("Copied: {}", callout));
        }

        if stale {
            response.hover_tip(
                "Crystal was likely killed: its callout was heard last more than 5 minutes ago.\nClick to copy the /p callout.",
            );
        } else {
            response.hover_tip(
                "Mysterious Crystal is alive (announces itself about every 2 minutes).\nClick to copy the /p callout.",
            );
        }

        ui.add_space(3.0);
        shadcn.full_width_separator(ui);

        // Keep the elapsed timer ticking live while the pin is visible.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(1));
    }

    /// honoring the event name style, upcoming toggle, and user overrides. The
    /// Mysterious Crystal "cry" short name and all others come from the
    /// event-call table; unknown encounters fall back to their full name.
    fn build_event_body(&self, boss: &BossCallEntry) -> String {
        if let Some((veteran, wave)) = crate::panels::event_call::parse_alien_wave(&boss.boss_name)
        {
            return crate::panels::event_call::alien_wave_call_body(
                veteran,
                wave,
                &self.alien_adept_prefix,
                &self.alien_veteran_prefix,
                self.alien_wave4_boss,
            );
        }
        crate::panels::event_call::event_call_body(
            &boss.boss_name,
            self.event_name_style,
            self.event_add_upcoming,
            &self.event_name_overrides,
        )
    }

    /// Render a kick alert entry.
    /// Returns (player_name_to_copy, should_dismiss).
    fn render_kick_alert(
        &self,
        ui: &mut Ui,
        alert: &KickAlertEntry,
        shadcn: &Shadcn,
    ) -> (Option<String>, bool) {
        let mut name_to_copy = None;
        let mut dismiss = false;

        // Red background for alert
        let alert_bg = Color32::from_rgb(80, 20, 20);
        let alert_bg_hover = Color32::from_rgb(100, 30, 30);
        let alert_text_color = Color32::from_rgb(255, 100, 100);

        let row_height = 32.0;

        // Draw clickable row
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), row_height),
            egui::Sense::click(),
        );

        // Highlight on hover
        let bg = if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            alert_bg_hover
        } else {
            alert_bg
        };
        ui.painter().rect_filled(rect, 4.0, bg);

        // Handle click
        if response.clicked() {
            name_to_copy = Some(alert.player_name.clone());
        }

        // Draw content
        let text_pos = egui::pos2(rect.left() + 10.0, rect.center().y);

        // Timestamp (same style as loot entries - monospace size 12)
        let time_str = Self::format_timestamp(alert.timestamp);
        ui.painter().text(
            text_pos,
            egui::Align2::LEFT_CENTER,
            &time_str,
            egui::FontId::monospace(12.0),
            Color32::GRAY,
        );

        // Alert message (no emoji - they don't render well)
        let alert_text = if alert.is_join_request {
            format!("Banned player requesting to join: {}!", alert.player_name)
        } else {
            format!("Banned player in party: {}!", alert.player_name)
        };
        ui.painter().text(
            egui::pos2(rect.left() + 80.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            &alert_text,
            egui::FontId::proportional(14.0),
            alert_text_color,
        );

        // Right-click context menu for dismiss
        response.context_menu(|ui| {
            if shadcn.btn(ui, "Dismiss").clicked() {
                dismiss = true;
                ui.close();
            }
        });

        // Tooltip
        response.hover_tip("Click to copy /pkick command");

        ui.add_space(4.0);

        (name_to_copy, dismiss)
    }

    /// Render a dust-full notification entry: red-outlined row with the dust
    /// sprite and an `<account> <type> dust is full!` message.
    fn render_dust_full_entry(
        &self,
        ui: &mut Ui,
        dust: &DustFullEntry,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        let red = Color32::from_rgb(255, 100, 100);
        let sprite_size = 22.0;
        let row_height = 32.0;

        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), row_height),
            egui::Sense::hover(),
        );

        // Red outline around the row.
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            4.0,
            egui::Stroke::new(1.5_f32, red),
            egui::StrokeKind::Outside,
        );

        let center_y = rect.center().y;
        let mut x = rect.left() + 10.0;

        // Timestamp (same style as other entries).
        let time_str = Self::format_timestamp(dust.timestamp);
        ui.painter().text(
            egui::pos2(x, center_y),
            egui::Align2::LEFT_CENTER,
            &time_str,
            egui::FontId::monospace(12.0),
            Color32::GRAY,
        );
        x += 70.0;

        // Respective dust sprite.
        let sprite_rect = egui::Rect::from_center_size(
            egui::pos2(x + sprite_size / 2.0, center_y),
            egui::vec2(sprite_size, sprite_size),
        );
        sprite_renderer.draw_sprite_in_rect(ui, dust.dust_type.object_id(), sprite_rect);
        x += sprite_size + 8.0;

        // Message.
        let message = format!(
            "{} {} dust is full!",
            dust.account_label(),
            dust.dust_type.name()
        );
        ui.painter().text(
            egui::pos2(x, center_y),
            egui::Align2::LEFT_CENTER,
            &message,
            egui::FontId::proportional(14.0),
            red,
        );

        ui.add_space(4.0);
    }

    /// Render the pinned end-of-cycle warnings: the automatic daily
    /// login calendar warning plus optional battlepass and season warnings, each
    /// shown only on the final day before their 00:00 UTC reset.
    fn render_pinned_warnings(
        &self,
        ui: &mut Ui,
        sprite_renderer: &mut SpriteRenderer,
        shadcn: &Shadcn,
    ) {
        use realmhound_core::season;
        use std::time::Duration as StdDuration;

        let now = Utc::now();
        let mut rendered_any = false;
        // Earliest time (as a std Duration from now) at which the displayed set
        // of warnings could change, so we can schedule a repaint even while
        // idle. egui is demand-driven, so without this an app sitting idle just
        // outside the 24h window would never wake up to show the warning.
        let mut next_wake: Option<StdDuration> = None;

        // Compute the next wake for a given future target: at the next whole
        // minute while inside the window (to tick the countdown), else at the
        // instant the 24h window opens.
        let mut consider_wake = |target: chrono::DateTime<Utc>| {
            if let Some(rem) = season::remaining(now, target) {
                let secs = if season::within_final_window(now, target, 24) {
                    60 - now.timestamp().rem_euclid(60)
                } else {
                    (rem - chrono::Duration::hours(24)).num_seconds().max(1)
                } as u64;
                let candidate = StdDuration::from_secs(secs.max(1));
                next_wake = Some(next_wake.map_or(candidate, |cur| cur.min(candidate)));
            }
        };

        // Uniform 3px margins between the top separator, each pinned row, and the
        // bottom separator. Zero the container's vertical item
        // spacing so the only gaps are the explicit 3px added before each row.
        let prev_item_spacing_y = ui.spacing().item_spacing.y;
        ui.spacing_mut().item_spacing.y = 0.0;

        // 1. Daily login calendar (fully automatic: last day of the month).
        if self.season_warnings.daily_calendar_enabled {
            let target = season::daily_calendar_end(now);
            consider_wake(target);
            if let Some(rem) = season::remaining(now, target) {
                if season::within_final_window(now, target, 24) {
                    let text = format!(
                        "Daily Login Calendar ends in {}! Don't forget to claim your items",
                        season::format_remaining(rem)
                    );
                    ui.add_space(3.0);
                    self.render_pinned_warning_row(ui, sprite_renderer, &text, PinnedTooltip::None);
                    rendered_any = true;
                }
            }
        }

        // 2. Battlepass (uses the configured shared/override reset).
        if self.season_warnings.battlepass_enabled {
            if let Some(target) = self.battlepass_reset {
                consider_wake(target);
                if let Some(rem) = season::remaining(now, target) {
                    if season::within_final_window(now, target, 24) {
                        let name = if self.battlepass_name.trim().is_empty() {
                            "Battlepass".to_string()
                        } else {
                            self.battlepass_name.clone()
                        };
                        let text = format!(
                            "{} ends in {}! Don't forget to claim your items",
                            name,
                            season::format_remaining(rem)
                        );
                        ui.add_space(3.0);
                        self.render_pinned_warning_row(
                            ui,
                            sprite_renderer,
                            &text,
                            PinnedTooltip::None,
                        );
                        rendered_any = true;
                    }
                }
            }
        }

        // 3. Season (with a hover tooltip listing the seasonal storage transfer rules).
        if self.season_warnings.season_enabled {
            if let Some(target) = self.season_reset {
                consider_wake(target);
                if let Some(rem) = season::remaining(now, target) {
                    if season::within_final_window(now, target, 24) {
                        let name = if self.season_name.trim().is_empty() {
                            "Season".to_string()
                        } else {
                            self.season_name.clone()
                        };
                        let text = format!(
                            "{} ends in {}! Don't forget to organize your seasonal storage",
                            name,
                            season::format_remaining(rem)
                        );
                        ui.add_space(3.0);
                        self.render_pinned_warning_row(
                            ui,
                            sprite_renderer,
                            &text,
                            PinnedTooltip::SeasonTransfer,
                        );
                        rendered_any = true;
                    }
                }
            }
        }

        if rendered_any {
            ui.add_space(3.0);
            shadcn.full_width_separator(ui);
        }
        ui.spacing_mut().item_spacing.y = prev_item_spacing_y;

        if let Some(wake) = next_wake {
            ui.ctx().request_repaint_after(wake);
        }
    }

    /// Render one pinned warning row with an orange outline and warning glyph.
    fn render_pinned_warning_row(
        &self,
        ui: &mut Ui,
        sprite_renderer: &mut SpriteRenderer,
        text: &str,
        tooltip: PinnedTooltip,
    ) {
        let outline_color = Color32::from_rgb(225, 160, 50);
        let text_color = Color32::from_rgb(255, 190, 70);
        let row_height = 32.0;

        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), row_height),
            egui::Sense::hover(),
        );

        ui.painter().rect_stroke(
            rect.shrink(1.0),
            4.0,
            egui::Stroke::new(1.5_f32, outline_color),
            egui::StrokeKind::Outside,
        );

        let center_y = rect.center().y;
        let mut x = rect.left() + 10.0;

        ui.painter().text(
            egui::pos2(x, center_y),
            egui::Align2::LEFT_CENTER,
            "⚠",
            egui::FontId::proportional(16.0),
            outline_color,
        );
        x += 24.0;

        ui.painter().text(
            egui::pos2(x, center_y),
            egui::Align2::LEFT_CENTER,
            text,
            egui::FontId::proportional(14.0),
            text_color,
        );

        match tooltip {
            PinnedTooltip::None => {}
            PinnedTooltip::SeasonTransfer => {
                response.hover_tip_ui(|ui| {
                    Self::render_season_transfer_tooltip(ui, sprite_renderer);
                });
            }
        }
    }

    /// Rich hover for the season warning: what happens to each seasonal storage
    /// container when a season ends. Sources are grouped as bulleted lists joined
    /// by a bracket to their shared destination, with a game icon before every
    /// container and names colour-coded (green = seasonal, yellow = regular) so
    /// the transfer rules read at a glance.
    fn render_season_transfer_tooltip(ui: &mut Ui, sr: &mut SpriteRenderer) {
        use crate::ui_colors::{REGULAR_COLOR, SEASONAL_COLOR};
        const RED: Color32 = Color32::from_rgb(235, 90, 90);
        const WHITE: Color32 = Color32::from_rgb(230, 230, 230);
        const BRACKET: Color32 = Color32::from_rgb(190, 190, 190);

        // Container object types (from ObjectID.list).
        const SEASONAL_VAULT: i32 = 0x32b;
        const MATERIALS_CHEST: i32 = 0x46f;
        const PET_INVENTORIES: i32 = 0x753;
        const SEASONAL_SPOILS: i32 = 0x2944;
        const GIFT_CHEST: i32 = 0x744;
        const POTION_RACK: i32 = 0xa112;
        const REGULAR_FORGE: i32 = 0x7e96;
        const ROW_H: f32 = 20.0;

        fn icon(ui: &mut Ui, sr: &mut SpriteRenderer, id: i32) {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
            sr.draw_sprite_in_rect(ui, id, rect);
        }
        fn emb(ui: &mut Ui, sr: &mut SpriteRenderer, ic: EmbeddedIcon) {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
            sr.draw_embedded_icon(ui, ic, rect);
        }
        fn word(ui: &mut Ui, s: &str, c: Color32) {
            ui.label(RichText::new(s).color(c));
        }
        fn bullet(ui: &mut Ui, sr: &mut SpriteRenderer, id: i32, name: &str, c: Color32) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                word(ui, "•", WHITE);
                icon(ui, sr, id);
                word(ui, name, c);
            });
        }
        // A left-facing bracket spanning `height`, signifying the bulleted group
        // to its left all transfers to the destination on its right.
        fn bracket(ui: &mut Ui, height: f32) {
            let (r, _) = ui.allocate_exact_size(egui::vec2(12.0, height), egui::Sense::hover());
            let cx = r.center().x;
            let (top, bot, mid) = (r.top() + 2.0, r.bottom() - 2.0, r.center().y);
            let p = ui.painter();
            let s = egui::Stroke::new(1.5_f32, BRACKET);
            p.line_segment([egui::pos2(cx, top), egui::pos2(cx, bot)], s);
            p.line_segment([egui::pos2(cx, top), egui::pos2(cx - 5.0, top)], s);
            p.line_segment([egui::pos2(cx, bot), egui::pos2(cx - 5.0, bot)], s);
            p.line_segment([egui::pos2(cx, mid), egui::pos2(cx + 5.0, mid)], s);
        }

        ui.set_max_width(480.0);
        ui.vertical(|ui| {
            word(ui, "When the season ends:", WHITE);
            ui.add_space(4.0);

            egui::Grid::new("season_transfer_grid")
                .spacing([6.0, 12.0])
                .show(ui, |ui| {
                    // Seasonal Vault / Materials Chest / Pet Inventories -> Seasonal Spoils.
                    let lh = ui
                        .vertical(|ui| {
                            bullet(ui, sr, SEASONAL_VAULT, "Seasonal Vault", SEASONAL_COLOR);
                            bullet(ui, sr, MATERIALS_CHEST, "Materials Chest", SEASONAL_COLOR);
                            bullet(ui, sr, PET_INVENTORIES, "Pet Inventories", SEASONAL_COLOR);
                        })
                        .response
                        .rect
                        .height();
                    bracket(ui, lh);
                    ui.vertical(|ui| {
                        ui.add_space(((lh - ROW_H) / 2.0).max(0.0));
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            word(ui, "→", WHITE);
                            icon(ui, sr, SEASONAL_SPOILS);
                            word(ui, "Seasonal Spoils", REGULAR_COLOR);
                            word(ui, "Chest", WHITE);
                        });
                    });
                    ui.end_row();

                    // Seasonal Gift Chest / Potion Rack -> Regular Gift Chest.
                    let lh = ui
                        .vertical(|ui| {
                            bullet(ui, sr, GIFT_CHEST, "Seasonal Gift Chest", SEASONAL_COLOR);
                            bullet(ui, sr, POTION_RACK, "Potion Rack", SEASONAL_COLOR);
                        })
                        .response
                        .rect
                        .height();
                    bracket(ui, lh);
                    ui.vertical(|ui| {
                        ui.add_space(((lh - ROW_H) / 2.0).max(0.0));
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            word(ui, "→", WHITE);
                            icon(ui, sr, GIFT_CHEST);
                            word(ui, "Regular Gift Chest", REGULAR_COLOR);
                        });
                    });
                    ui.end_row();

                    // Dismantled Seasonal Forge materials -> Regular Forge materials.
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        emb(ui, sr, EmbeddedIcon::ForgeMythical);
                        emb(ui, sr, EmbeddedIcon::ForgeLegendary);
                        emb(ui, sr, EmbeddedIcon::ForgeRare);
                        emb(ui, sr, EmbeddedIcon::ForgeCommon);
                        word(ui, "Seasonal Forge", SEASONAL_COLOR);
                        word(ui, "materials", WHITE);
                    });
                    ui.label("");
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        word(ui, "→", WHITE);
                        icon(ui, sr, REGULAR_FORGE);
                        word(ui, "Regular Forge", REGULAR_COLOR);
                        word(ui, "materials", WHITE);
                    });
                    ui.end_row();

                    // Seasonal dust does NOT transfer.
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        icon(ui, sr, DustType::Green.object_id());
                        icon(ui, sr, DustType::Red.object_id());
                        icon(ui, sr, DustType::Purple.object_id());
                        word(ui, "Seasonal dust", SEASONAL_COLOR);
                        word(ui, "does", WHITE);
                        word(ui, "NOT transfer", RED);
                    });
                    ui.label("");
                    word(ui, "- spend it before the season ends", WHITE);
                    ui.end_row();
                });
        });
    }

    fn render_warning_entry(&self, ui: &mut Ui, warning: &WarningEntry) -> bool {
        let (outline_color, text_color) = match warning.severity {
            WarningSeverity::Orange => (
                Color32::from_rgb(225, 160, 50),
                Color32::from_rgb(255, 190, 70),
            ),
            WarningSeverity::Red => (
                Color32::from_rgb(215, 75, 75),
                Color32::from_rgb(255, 100, 100),
            ),
        };
        let row_height = 32.0;

        let sense = if warning.copyable {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        };
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), row_height), sense);

        // Colored outline around the row.
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            4.0,
            egui::Stroke::new(1.5_f32, outline_color),
            egui::StrokeKind::Outside,
        );

        let center_y = rect.center().y;
        let mut x = rect.left() + 10.0;

        // Timestamp.
        let time_str = Self::format_timestamp(warning.timestamp);
        ui.painter().text(
            egui::pos2(x, center_y),
            egui::Align2::LEFT_CENTER,
            &time_str,
            egui::FontId::monospace(12.0),
            Color32::GRAY,
        );
        x += 70.0;

        // Warning text.
        ui.painter().text(
            egui::pos2(x, center_y),
            egui::Align2::LEFT_CENTER,
            &warning.text,
            egui::FontId::proportional(14.0),
            text_color,
        );

        // Tooltip for copyable entries.
        if warning.copyable {
            response.clone().hover_tip("Click to copy");
        }

        ui.add_space(4.0);

        warning.copyable && response.clicked()
    }

    /// Render a `/who` roster summary row. Returns `true` when the
    /// row is clicked (caller copies the names, one per line).
    fn render_who_roster_entry(&self, ui: &mut Ui, roster: &WhoRosterEntry) -> bool {
        let outline_color = Color32::from_rgb(120, 180, 120);
        let text_color = Color32::from_rgb(170, 220, 170);
        let row_height = 32.0;

        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), row_height),
            egui::Sense::click(),
        );

        if response.hovered() {
            ui.painter().rect_filled(
                rect,
                4.0,
                Color32::from_rgba_unmultiplied(120, 200, 120, 12),
            );
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        ui.painter().rect_stroke(
            rect.shrink(1.0),
            4.0,
            egui::Stroke::new(1.5_f32, outline_color),
            egui::StrokeKind::Outside,
        );

        let center_y = rect.center().y;
        let mut x = rect.left() + 10.0;

        let time_str = Self::format_timestamp(roster.timestamp);
        ui.painter().text(
            egui::pos2(x, center_y),
            egui::Align2::LEFT_CENTER,
            &time_str,
            egui::FontId::monospace(12.0),
            Color32::GRAY,
        );
        x += 70.0;

        let label = format!("/who - {} players (click to copy)", roster.names.len());
        ui.painter().text(
            egui::pos2(x, center_y),
            egui::Align2::LEFT_CENTER,
            &label,
            egui::FontId::proportional(14.0),
            text_color,
        );

        response.clone().hover_tip("Click to copy player names");
        ui.add_space(4.0);

        response.clicked()
    }

    /// Render a direct-message entry with cyan outline.
    /// Returns the user action: left-click copies `/t SenderName`, right-click
    /// opens a context menu with "View Chat with SenderName".
    fn render_dm_entry(
        &self,
        ui: &mut Ui,
        dm: &DmEntry,
        sprite_renderer: &mut SpriteRenderer,
    ) -> DmAction {
        let dm_color = ChatType::Whisper.color();
        let preview_color = Color32::from_rgb(160, 220, 255);
        let row_height = 32.0;
        let mut action = DmAction::None;

        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), row_height),
            egui::Sense::click(),
        );

        // Hover highlight
        if response.hovered() {
            ui.painter().rect_filled(
                rect,
                4.0,
                Color32::from_rgba_unmultiplied(100, 200, 255, 12),
            );
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        // Left click -> copy /t SenderName
        if response.clicked() {
            action = DmAction::CopyReply(dm.sender.clone());
        }

        // Right-click context menu -> View Chat
        response.context_menu(|ui| {
            let label = format!("View Chat with {}", dm.sender);
            if ui.button(label).clicked() {
                action = DmAction::ViewChat(dm.sender.clone());
                ui.close();
            }
        });

        let center_y = rect.center().y;
        let mut x = rect.left() + 10.0;

        // Timestamp
        let time_str = Self::format_timestamp(dm.timestamp);
        ui.painter().text(
            egui::pos2(x, center_y),
            egui::Align2::LEFT_CENTER,
            &time_str,
            egui::FontId::monospace(12.0),
            Color32::GRAY,
        );
        x += 70.0;

        // "sender: " prefix
        let prefix = format!("{}: ", dm.sender);
        let prefix_galley =
            ui.painter()
                .layout_no_wrap(prefix, egui::FontId::proportional(14.0), dm_color);
        ui.painter().galley(
            egui::pos2(x, center_y - prefix_galley.rect.height() / 2.0),
            prefix_galley.clone(),
            dm_color,
        );
        x += prefix_galley.rect.width();

        // Message body with inline emotes
        let max_x = rect.right() - 10.0;
        if emotes::has_emote_tags(&dm.text) {
            let segments = emotes::parse_segments(&dm.text);
            let asset_manager = realmhound_core::assets::get_asset_manager();
            let sprite_size = emotes::EMOTE_SPRITE_SIZE;

            for segment in segments {
                if x >= max_x {
                    break;
                }
                match segment {
                    EmoteSegment::Text(t) if !t.is_empty() => {
                        let available = max_x - x;
                        let galley = ui.painter().layout(
                            t.to_string(),
                            egui::FontId::proportional(14.0),
                            dm_color,
                            available,
                        );
                        ui.painter().galley(
                            egui::pos2(x, center_y - galley.rect.height() / 2.0),
                            galley.clone(),
                            dm_color,
                        );
                        x += galley.rect.width();
                    }
                    EmoteSegment::Emote(name) => {
                        let object_id = emotes::resolve_emote_id(asset_manager, name);

                        if let Some(id) = object_id {
                            x += EMOTE_PADDING;
                            let sprite_rect = egui::Rect::from_center_size(
                                egui::pos2(x + sprite_size / 2.0, center_y),
                                egui::vec2(sprite_size, sprite_size),
                            );
                            if !sprite_renderer.draw_sprite_in_rect(ui, id, sprite_rect) {
                                let fallback = emotes::fallback_text(name);
                                let available = max_x - x;
                                let galley = ui.painter().layout(
                                    fallback,
                                    egui::FontId::proportional(14.0),
                                    dm_color,
                                    available,
                                );
                                ui.painter().galley(
                                    egui::pos2(x, center_y - galley.rect.height() / 2.0),
                                    galley.clone(),
                                    dm_color,
                                );
                                x += galley.rect.width();
                            } else {
                                x += sprite_size + EMOTE_PADDING;
                            }
                        } else {
                            let fallback = emotes::fallback_text(name);
                            let available = max_x - x;
                            let galley = ui.painter().layout(
                                fallback,
                                egui::FontId::proportional(14.0),
                                dm_color,
                                available,
                            );
                            ui.painter().galley(
                                egui::pos2(x, center_y - galley.rect.height() / 2.0),
                                galley.clone(),
                                dm_color,
                            );
                            x += galley.rect.width();
                        }
                    }
                    _ => {}
                }
            }
        } else {
            let available = (max_x - x).max(0.0);
            if available > 0.0 {
                let text_galley = ui.painter().layout(
                    dm.text.clone(),
                    egui::FontId::proportional(14.0),
                    dm_color,
                    available,
                );
                ui.painter().galley(
                    egui::pos2(x, center_y - text_galley.rect.height() / 2.0),
                    text_galley,
                    dm_color,
                );
            }
        }

        // Tooltip: show what will be copied
        let copy_preview = format!("/t {} ", dm.sender);
        response.hover_tip(
            RichText::new(format!("Click to copy: {}", copy_preview)).color(preview_color),
        );

        ui.add_space(4.0);
        action
    }

    /// Render a public key-pop entry: the dungeon key sprite, dungeon name, and
    /// "Opened by <player>". Observed (not entered) events stay clickable and
    /// never expire; clicking copies a "Thanks <opener> for the key"
    /// callout. Returns true when clicked.
    fn render_key_pop_entry(
        &self,
        ui: &mut Ui,
        keypop: &KeyPopEntry,
        sprite_renderer: &mut SpriteRenderer,
    ) -> bool {
        let row_height = 32.0;
        // Self-opened keys have no one to thank, so the row is inert (no copy).
        let clickable = keypop.thankable;

        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), row_height),
            if clickable {
                egui::Sense::click()
            } else {
                egui::Sense::hover()
            },
        );

        // Subtle blue fill + outline, matching an active dungeon row.
        ui.painter()
            .rect_filled(rect, 4.0, Color32::from_rgb(28, 36, 48));
        ui.painter().rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(1.5_f32, Color32::from_rgb(70, 120, 190)),
            egui::StrokeKind::Outside,
        );
        if clickable && response.hovered() {
            ui.painter().rect_filled(
                rect,
                4.0,
                Color32::from_rgba_unmultiplied(100, 160, 230, 16),
            );
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        let name_color = Color32::WHITE;
        let opener_color = Color32::from_rgb(150, 230, 150);
        let cy = rect.center().y;

        // Timestamp.
        let time_str = Self::format_timestamp(keypop.timestamp);
        ui.painter().text(
            egui::pos2(rect.left() + 10.0, cy),
            egui::Align2::LEFT_CENTER,
            &time_str,
            egui::FontId::monospace(12.0),
            Color32::GRAY,
        );

        // Dungeon key sprite.
        let mut text_left = rect.left() + 80.0;
        let icon_size = 28.0;
        let icon_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 76.0, cy - icon_size / 2.0),
            egui::vec2(icon_size, icon_size),
        );
        if sprite_renderer.draw_outlined_sprite_in_rect_tinted(
            ui,
            keypop.key_id,
            icon_rect,
            Color32::WHITE,
        ) {
            text_left = icon_rect.right() + 8.0;
        }

        // Dungeon name.
        let name = if keypop.dungeon_name.is_empty() {
            "Dungeon".to_string()
        } else {
            keypop.dungeon_name.clone()
        };
        let name_font = egui::FontId::proportional(16.0);
        let name_galley = ui.painter().layout_no_wrap(name, name_font, name_color);
        let name_width = name_galley.size().x;
        ui.painter().galley(
            egui::pos2(text_left, cy - name_galley.size().y / 2.0),
            name_galley,
            name_color,
        );

        // "Opened by <player>".
        let opener_text = format!("Opened by {}", keypop.opener);
        ui.painter().text(
            egui::pos2(text_left + name_width + 12.0, cy),
            egui::Align2::LEFT_CENTER,
            &opener_text,
            egui::FontId::proportional(14.0),
            opener_color,
        );

        if let Some(callout) = keypop.callout() {
            response
                .clone()
                .hover_tip(format!("Click to copy: {}", callout));
        }
        ui.add_space(4.0);
        clickable && response.clicked()
    }

    /// Render a special area-unlock entry: for each contribution draw the popped
    /// item's sprite next to its popper, prefixed by the title. Stays clickable
    /// and copyable indefinitely when there is a thanks callout; entries with no
    /// callout (the local player was the opener) are inert. Returns true when a
    /// copyable row is clicked.
    fn render_area_unlock_entry(
        &self,
        ui: &mut Ui,
        unlock: &AreaUnlockEntry,
        sprite_renderer: &mut SpriteRenderer,
    ) -> bool {
        let clickable = unlock.has_callout();
        let row_height = 32.0;

        let sense = if clickable {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        };
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), row_height), sense);

        if clickable {
            ui.painter()
                .rect_filled(rect, 4.0, Color32::from_rgb(40, 34, 20));
            ui.painter().rect_stroke(
                rect,
                4.0,
                egui::Stroke::new(1.5_f32, Color32::from_rgb(210, 170, 70)),
                egui::StrokeKind::Outside,
            );
            if response.hovered() {
                ui.painter().rect_filled(
                    rect,
                    4.0,
                    Color32::from_rgba_unmultiplied(230, 190, 90, 16),
                );
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if ui.is_rect_visible(rect) {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(200));
            }
        }

        let inert_grey = Color32::from_gray(120);
        let title_color = if clickable {
            Color32::WHITE
        } else {
            inert_grey
        };
        let popper_color = if clickable {
            Color32::from_rgb(150, 230, 150)
        } else {
            inert_grey
        };
        let tint = if clickable {
            Color32::WHITE
        } else {
            Color32::from_gray(110)
        };
        let cy = rect.center().y;

        // Timestamp.
        let time_str = Self::format_timestamp(unlock.timestamp);
        ui.painter().text(
            egui::pos2(rect.left() + 10.0, cy),
            egui::Align2::LEFT_CENTER,
            &time_str,
            egui::FontId::monospace(12.0),
            Color32::GRAY,
        );

        // Title.
        let mut x = rect.left() + 80.0;
        let title_font = egui::FontId::proportional(15.0);
        let title_galley =
            ui.painter()
                .layout_no_wrap(unlock.title.clone(), title_font, title_color);
        let title_w = title_galley.size().x;
        ui.painter().galley(
            egui::pos2(x, cy - title_galley.size().y / 2.0),
            title_galley,
            title_color,
        );
        x += title_w + 12.0;

        // Each contribution: item sprite then popper name.
        let icon_size = 24.0;
        for (item_id, popper) in &unlock.contributions {
            let icon_rect = egui::Rect::from_min_size(
                egui::pos2(x, cy - icon_size / 2.0),
                egui::vec2(icon_size, icon_size),
            );
            if sprite_renderer.draw_outlined_sprite_in_rect_tinted(ui, *item_id, icon_rect, tint) {
                x = icon_rect.right() + 4.0;
            }
            let popper_font = egui::FontId::proportional(14.0);
            let popper_galley =
                ui.painter()
                    .layout_no_wrap(popper.clone(), popper_font, popper_color);
            let popper_w = popper_galley.size().x;
            ui.painter().galley(
                egui::pos2(x, cy - popper_galley.size().y / 2.0),
                popper_galley,
                popper_color,
            );
            x += popper_w + 14.0;
        }

        if clickable {
            response
                .clone()
                .hover_tip(format!("Click to copy: {}", unlock.callout()));
        }
        ui.add_space(4.0);
        clickable && response.clicked()
    }

    const WHITE_BAG_OBJECT_ID: i32 = 1292;

    /// Dimitus boss object id, used as a distinct icon for Dimitus dungeons.
    const DIMITUS_OBJECT_ID: i32 = 8813;

    /// XP Booster object id, used as the XP-bonus symbol.
    const XP_BOOSTER_OBJECT_ID: i32 = 3179;

    /// Text color for a modifier name by its danger category.
    fn danger_color(danger: ModDanger) -> Color32 {
        match danger {
            ModDanger::Red => Color32::from_rgb(235, 90, 90),
            ModDanger::Orange => Color32::from_rgb(240, 160, 70),
            ModDanger::Golden => Color32::from_rgb(240, 205, 90),
            ModDanger::Purple => Color32::from_rgb(195, 130, 240),
            ModDanger::Blue => Color32::from_rgb(110, 180, 255),
            ModDanger::White => Color32::from_rgb(225, 225, 225),
        }
    }

    /// Leading glyph for a modifier's in-game type: gift for Reward,
    /// diamond for Unique, spark for Special, none for Normal. Used as a fallback
    /// when the embedded type icon fails to load.
    fn mod_type_glyph(mod_type: ModType) -> Option<&'static str> {
        match mod_type {
            ModType::Reward => Some("🎁"),
            ModType::Unique => Some("💎"),
            ModType::Special => Some("✨"),
            ModType::Normal => None,
        }
    }

    /// Embedded type icon for a modifier's in-game type. `None` for
    /// Normal mods (no icon).
    fn mod_type_icon(mod_type: ModType) -> Option<EmbeddedIcon> {
        match mod_type {
            ModType::Reward => Some(EmbeddedIcon::ModTypeReward),
            ModType::Unique => Some(EmbeddedIcon::ModTypeUnique),
            ModType::Special => Some(EmbeddedIcon::ModTypeSpecial),
            ModType::Normal => None,
        }
    }

    /// Embedded grade-badge icon for a grade string. An absent or
    /// unrecognized grade maps to the "no grade" badge.
    fn grade_icon(grade: Option<&str>) -> EmbeddedIcon {
        match grade
            .and_then(|g| g.chars().next())
            .map(|c| c.to_ascii_uppercase())
        {
            Some('S') => EmbeddedIcon::GradeS,
            Some('A') => EmbeddedIcon::GradeA,
            Some('B') => EmbeddedIcon::GradeB,
            Some('C') => EmbeddedIcon::GradeC,
            Some('D') => EmbeddedIcon::GradeD,
            _ => EmbeddedIcon::GradeNo,
        }
    }

    /// Render a dungeon-entered entry.
    ///
    /// Returns the clipboard callout to copy when the row is clicked (and the
    /// dungeon has a callout), else `None`.
    fn render_dungeon_entry(
        &self,
        ui: &mut Ui,
        dungeon: &DungeonEntry,
        sprite_renderer: &mut SpriteRenderer,
        now: std::time::Instant,
    ) -> Option<String> {
        let remaining = dungeon.remaining(now);
        let expired = remaining.is_zero();
        // A callout can only be copied while the dungeon is still joinable.
        let callout_active = dungeon.callout.is_some() && !expired;

        // Background fill + outline stroke encode the callout's overall danger:
        // gold for Dimitus, red for dangerous mods, blue otherwise.
        // Once expired the row drops its outline and background entirely.
        let (bg, outline_stroke) = if expired {
            (Color32::TRANSPARENT, Color32::TRANSPARENT)
        } else {
            match dungeon.outline {
                OutlineKind::Golden => (
                    Color32::from_rgb(52, 44, 20),
                    Color32::from_rgb(225, 185, 70),
                ),
                OutlineKind::Red => (
                    Color32::from_rgb(52, 28, 32),
                    Color32::from_rgb(215, 75, 75),
                ),
                OutlineKind::Blue => (
                    Color32::from_rgb(28, 36, 48),
                    Color32::from_rgb(70, 120, 190),
                ),
            }
        };
        // Expired rows grey out every text element; active rows keep white name
        // and reward text.
        let expired_grey = Color32::from_gray(120);
        let name_color = if expired {
            expired_grey
        } else {
            Color32::WHITE
        };
        let loot_color = name_color;
        let dust_color = name_color;
        let xp_color = name_color;
        let callout_color = Color32::from_rgb(150, 230, 150);

        let has_mods = !dungeon.mods.is_empty();
        let row_height = if has_mods { 46.0 } else { 32.0 };
        // Clickable only while there is a callout still within its join window.
        let sense = if callout_active {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        };
        let (rect, mut response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), row_height), sense);
        ui.painter().rect_filled(rect, 4.0, bg);
        ui.painter().rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(1.5_f32, outline_stroke),
            egui::StrokeKind::Outside,
        );

        // Keep the countdown ticking even when not capturing, but only for rows
        // that are actually on screen so off-screen entries don't force repaints.
        // Also keep repainting while the run timer is still live, so it ticks up
        // even after the join window has expired.
        let timer_running = dungeon.frozen_elapsed_ms.is_none();
        if (!expired || timer_running) && ui.is_rect_visible(rect) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }

        // Click-to-copy affordance (same behavior as boss/realm events): a
        // subtle highlight + pointing-hand cursor on hover when there's a
        // callout still within its join window.
        if callout_active && response.hovered() {
            ui.painter().rect_filled(
                rect,
                4.0,
                Color32::from_rgba_unmultiplied(255, 255, 255, 15),
            );
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        // The left block (timestamp, portal, name, grade, timer) sits on the
        // row's vertical center. The right block stacks reward stats above the
        // modifier line.
        let cy = rect.center().y;
        let stats_y = if has_mods { rect.top() + 13.0 } else { cy };

        // Timestamp.
        let time_str = Self::format_timestamp(dungeon.timestamp);
        ui.painter().text(
            egui::pos2(rect.left() + 10.0, cy),
            egui::Align2::LEFT_CENTER,
            &time_str,
            egui::FontId::monospace(12.0),
            Color32::GRAY,
        );

        // Portal sprite. Falls back to the Dimitus sprite when the portal id is
        // unknown, so the entry still has a recognizable icon.
        let mut text_left = rect.left() + 80.0;
        let icon_id = dungeon.portal_id.or_else(|| {
            if dungeon.is_dimitus {
                Some(Self::DIMITUS_OBJECT_ID)
            } else {
                None
            }
        });
        if let Some(id) = icon_id {
            let icon_size = 28.0;
            let icon_rect = egui::Rect::from_min_size(
                egui::pos2(rect.left() + 76.0, cy - icon_size / 2.0),
                egui::vec2(icon_size, icon_size),
            );
            let tint = if expired {
                Color32::from_gray(110)
            } else {
                Color32::WHITE
            };
            if sprite_renderer.draw_outlined_sprite_in_rect_tinted(ui, id, icon_rect, tint) {
                text_left = icon_rect.right() + 8.0;
            }
        }

        // Dungeon name, measured so the grade icon and timer follow it.
        let name_text = format!("Entered {}", dungeon.dungeon_name);
        let name_font = egui::FontId::proportional(18.0);
        let name_galley = ui
            .painter()
            .layout_no_wrap(name_text, name_font, name_color);
        let name_width = name_galley.size().x;
        ui.painter().galley(
            egui::pos2(text_left, cy - name_galley.size().y / 2.0),
            name_galley,
            name_color,
        );

        let bonus_font = egui::FontId::proportional(13.0);
        let mut cursor = text_left + name_width + 10.0;

        // Grade badge with its letter drawn inside.
        cursor = self.draw_grade_icon(
            ui,
            sprite_renderer,
            cursor,
            cy,
            dungeon.grade.as_deref(),
            expired,
        ) + 8.0;

        // Join countdown, sized like the portal. Its slot is always reserved so
        // the right block keeps its position once the timer expires.
        let timer_size = 28.0;
        if !expired {
            let radius = timer_size / 2.0;
            let fraction =
                (remaining.as_secs_f32() / dungeon.window().as_secs_f32()).clamp(0.0, 1.0);
            let secs = remaining.as_secs_f32().ceil() as u64;
            let center = egui::pos2(cursor + radius, cy);
            Self::draw_countdown_circle(ui, center, radius, fraction, secs, dungeon.estimated);
        }
        cursor += timer_size + 12.0;

        let right_start = cursor;
        let content_right = rect.right() - 10.0;

        // Right block, row 1: reward bonuses ([icon] Label +X%).
        let mut stats_cursor = right_start;
        for (value, color, label, bonus_icon_id, embedded) in [
            (
                dungeon.loot_bonus,
                loot_color,
                "Loot",
                Self::WHITE_BAG_OBJECT_ID,
                None,
            ),
            (
                dungeon.dust_bonus,
                dust_color,
                "Dust",
                -1,
                Some(EmbeddedIcon::DustGrey),
            ),
            (
                dungeon.xp_bonus,
                xp_color,
                "XP",
                Self::XP_BOOSTER_OBJECT_ID,
                None,
            ),
        ] {
            if value <= 0 {
                continue;
            }
            let icon_size = 20.0;
            let icon_rect = egui::Rect::from_min_size(
                egui::pos2(stats_cursor, stats_y - icon_size / 2.0),
                egui::vec2(icon_size, icon_size),
            );
            let drew = if let Some(icon) = embedded {
                sprite_renderer.draw_embedded_icon(ui, icon, icon_rect)
            } else if bonus_icon_id >= 0 {
                sprite_renderer.draw_outlined_sprite_in_rect(ui, bonus_icon_id, icon_rect)
            } else {
                false
            };
            if drew {
                stats_cursor = icon_rect.right() + 3.0;
            }
            let galley = ui.painter().layout_no_wrap(
                format!("{} +{}%", label, value),
                bonus_font.clone(),
                color,
            );
            let w = galley.size().x;
            ui.painter().galley(
                egui::pos2(stats_cursor, stats_y - galley.size().y / 2.0),
                galley,
                color,
            );
            stats_cursor += w + 12.0;
        }

        // Right block, row 2: modifier chips, aligned under the reward stats.
        if has_mods {
            self.draw_mod_line(
                ui,
                dungeon,
                right_start,
                rect,
                content_right,
                expired,
                sprite_renderer,
            );
        }

        // Clipboard callout preview, right-aligned, and the live dungeon run
        // timer. While the dungeon is still joinable the timer sits just left of
        // the callout preview; once it can no longer be called (window expired or
        // non-callable) the timer takes over the far-right slot.
        let run_elapsed = dungeon.run_elapsed(now);
        let timer_text = format!("🕒 {}", format_run_timer(run_elapsed));
        // A live-ticking timer stays visible even after the join window expires;
        // only a settled (frozen) value dims to grey.
        let timer_color = if dungeon.frozen_elapsed_ms.is_some() {
            Color32::from_gray(150)
        } else {
            Color32::from_rgb(180, 205, 235)
        };
        let mut timer_right = content_right;
        if callout_active {
            if let Some(body) = &dungeon.callout {
                let callout_rect = ui.painter().text(
                    egui::pos2(content_right, rect.center().y),
                    egui::Align2::RIGHT_CENTER,
                    format!(
                        "📋 {}",
                        self.assemble_callout(
                            body,
                            dungeon.server_name.as_deref(),
                            dungeon.realm_name.as_deref(),
                            self.dungeon_join_position
                        )
                    ),
                    egui::FontId::monospace(12.0),
                    callout_color,
                );
                timer_right = callout_rect.left() - 12.0;
            }
        }
        ui.painter().text(
            egui::pos2(timer_right, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            timer_text,
            egui::FontId::monospace(12.0),
            timer_color,
        );

        // Hover tooltip: copy hint when the callout is still copyable (same as
        // boss/realm events), else a bonus summary.
        if callout_active {
            response = response.hover_tip("Click to copy callout");
        } else if let Some(tooltip) = Self::dungeon_bonus_tooltip(dungeon) {
            response = response.hover_tip(tooltip);
        }

        ui.add_space(4.0);

        // Copy the callout when the row is clicked (only while still joinable).
        if callout_active && response.clicked() {
            dungeon.callout.as_ref().map(|body| {
                self.assemble_callout(
                    body,
                    dungeon.server_name.as_deref(),
                    dungeon.realm_name.as_deref(),
                    self.dungeon_join_position,
                )
            })
        } else {
            None
        }
    }

    /// Draw the dungeon grade badge at `x`, vertically centered on `center_y`,
    /// with the grade letter drawn inside it. Returns the badge's right edge.
    /// Falls back to a colored letter badge if the grade icon asset isn't
    /// available.
    fn draw_grade_icon(
        &self,
        ui: &mut Ui,
        sprite_renderer: &mut SpriteRenderer,
        x: f32,
        center_y: f32,
        grade: Option<&str>,
        expired: bool,
    ) -> f32 {
        let size = 20.0;
        let icon_rect =
            egui::Rect::from_min_size(egui::pos2(x, center_y - size / 2.0), egui::vec2(size, size));
        let letter = grade.map(str::trim).filter(|g| !g.is_empty());
        if sprite_renderer.draw_embedded_icon(ui, Self::grade_icon(grade), icon_rect) {
            // White letter with a dark shadow keeps it readable on every gem color.
            if let Some(g) = letter {
                let font = egui::FontId::proportional(13.0);
                let fg = if expired {
                    Color32::from_gray(150)
                } else {
                    Color32::WHITE
                };
                ui.painter().text(
                    icon_rect.center() + egui::vec2(1.0, 1.0),
                    egui::Align2::CENTER_CENTER,
                    g,
                    font.clone(),
                    Color32::from_black_alpha(180),
                );
                ui.painter()
                    .text(icon_rect.center(), egui::Align2::CENTER_CENTER, g, font, fg);
            }
            return icon_rect.right();
        }
        // Fallback: colored letter badge (e.g. when assets failed to load).
        let Some(g) = letter else {
            return x;
        };
        let (bg, fg) = match g.chars().next().map(|c| c.to_ascii_uppercase()) {
            Some('S') => (Color32::from_rgb(200, 160, 40), Color32::BLACK),
            Some('A') => (Color32::from_rgb(70, 150, 220), Color32::WHITE),
            Some('B') => (Color32::from_rgb(70, 170, 150), Color32::WHITE),
            Some('C') => (Color32::from_rgb(120, 130, 140), Color32::WHITE),
            _ => (Color32::from_rgb(100, 110, 120), Color32::WHITE),
        };
        ui.painter().rect_filled(icon_rect, 4.0, bg);
        ui.painter().text(
            icon_rect.center(),
            egui::Align2::CENTER_CENTER,
            g,
            egui::FontId::proportional(12.0),
            fg,
        );
        icon_rect.right()
    }

    /// Draw the dungeon's modifier list as a single line of `[type icon] Name`
    /// chips, comma separated, color-coded by danger category.
    fn draw_mod_line(
        &self,
        ui: &mut Ui,
        dungeon: &DungeonEntry,
        text_left: f32,
        rect: egui::Rect,
        right_limit: f32,
        expired: bool,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        let center_y = rect.bottom() - 13.0;
        let font = egui::FontId::proportional(12.0);
        let expired_grey = Color32::from_gray(120);
        let sep_color = if expired {
            expired_grey
        } else {
            Color32::from_rgb(120, 120, 120)
        };
        let icon_size = 20.0;
        let mut cursor = text_left;

        for (idx, chip) in dungeon.mods.iter().enumerate() {
            if cursor >= right_limit {
                break;
            }
            // Leading icon: Dimitus shows its portrait; boss/minion mods show
            // their tier-specific icon; Reward/Unique/Special show their type
            // icon; Normal shows none. Falls back to an emoji glyph if an
            // embedded type icon isn't available.
            if chip.is_dimitus {
                let icon_rect = egui::Rect::from_min_size(
                    egui::pos2(cursor, center_y - icon_size / 2.0),
                    egui::vec2(icon_size, icon_size),
                );
                if sprite_renderer.draw_outlined_sprite_in_rect(
                    ui,
                    Self::DIMITUS_OBJECT_ID,
                    icon_rect,
                ) {
                    if expired {
                        ui.painter().rect_filled(
                            icon_rect,
                            0.0,
                            Color32::from_rgba_unmultiplied(40, 40, 40, 140),
                        );
                    }
                    cursor = icon_rect.right() + 3.0;
                }
            } else if let Some(mod_icon) = chip.modifier_icon {
                let icon_rect = egui::Rect::from_min_size(
                    egui::pos2(cursor, center_y - icon_size / 2.0),
                    egui::vec2(icon_size, icon_size),
                );
                if sprite_renderer.draw_modifier_icon(ui, mod_icon, icon_rect) {
                    if expired {
                        ui.painter().rect_filled(
                            icon_rect,
                            0.0,
                            Color32::from_rgba_unmultiplied(40, 40, 40, 140),
                        );
                    }
                    cursor = icon_rect.right() + 3.0;
                }
            } else if let Some(type_icon) = Self::mod_type_icon(chip.mod_type) {
                let icon_rect = egui::Rect::from_min_size(
                    egui::pos2(cursor, center_y - icon_size / 2.0),
                    egui::vec2(icon_size, icon_size),
                );
                if sprite_renderer.draw_embedded_icon(ui, type_icon, icon_rect) {
                    if expired {
                        ui.painter().rect_filled(
                            icon_rect,
                            0.0,
                            Color32::from_rgba_unmultiplied(40, 40, 40, 140),
                        );
                    }
                    cursor = icon_rect.right() + 3.0;
                } else if let Some(glyph) = Self::mod_type_glyph(chip.mod_type) {
                    let glyph_color = if expired {
                        expired_grey
                    } else {
                        Color32::WHITE
                    };
                    let g = ui.painter().layout_no_wrap(
                        glyph.to_string(),
                        egui::FontId::proportional(12.0),
                        glyph_color,
                    );
                    let w = g.size().x;
                    ui.painter().galley(
                        egui::pos2(cursor, center_y - g.size().y / 2.0),
                        g,
                        glyph_color,
                    );
                    cursor += w + 2.0;
                }
            }

            // Modifier name in its danger color (greyed once expired).
            let color = if expired {
                expired_grey
            } else {
                Self::danger_color(chip.danger)
            };
            let galley = ui
                .painter()
                .layout_no_wrap(chip.name.clone(), font.clone(), color);
            let w = galley.size().x;
            ui.painter().galley(
                egui::pos2(cursor, center_y - galley.size().y / 2.0),
                galley,
                color,
            );
            cursor += w;

            // Comma separator between chips.
            if idx + 1 < dungeon.mods.len() {
                let sep = ui
                    .painter()
                    .layout_no_wrap(", ".to_string(), font.clone(), sep_color);
                let sw = sep.size().x;
                ui.painter().galley(
                    egui::pos2(cursor, center_y - sep.size().y / 2.0),
                    sep,
                    sep_color,
                );
                cursor += sw;
            }
        }
    }

    /// Draw the join countdown as a depleting pie with the remaining whole
    /// seconds in the center. `fraction` is remaining/total in `0.0..=1.0`; the
    /// fill shrinks clockwise from the top and shifts green -> red as it runs
    /// down.
    fn draw_countdown_circle(
        ui: &mut Ui,
        center: egui::Pos2,
        radius: f32,
        fraction: f32,
        secs: u64,
        estimated: bool,
    ) {
        use std::f32::consts::{FRAC_PI_2, TAU};

        let painter = ui.painter();
        let fraction = fraction.clamp(0.0, 1.0);

        // Track behind the fill.
        painter.circle_filled(
            center,
            radius,
            Color32::from_rgba_unmultiplied(255, 255, 255, 25),
        );

        // Pie wedge for the remaining time, as a triangle fan from the center.
        // Darker green -> yellow -> red as time runs out, so the white number
        // stays readable.
        let fill_color = Color32::from_rgb(
            egui::lerp(40.0..=170.0, 1.0 - fraction) as u8,
            egui::lerp(140.0..=45.0, 1.0 - fraction) as u8,
            45,
        );
        // The empty wedge grows clockwise from 12 o'clock as time elapses, so the
        // remaining wedge starts past the empty part and sweeps clockwise back to
        // the top (egui y-down: increasing angle is clockwise).
        let segments = 48usize;
        let steps = ((segments as f32) * fraction).ceil() as usize;
        if steps > 0 {
            let span = fraction * TAU;
            let start = -FRAC_PI_2 + (1.0 - fraction) * TAU;
            let mut mesh = egui::epaint::Mesh::default();
            let uv = egui::epaint::WHITE_UV;
            mesh.vertices.push(egui::epaint::Vertex {
                pos: center,
                uv,
                color: fill_color,
            });
            for i in 0..=steps {
                let angle = start + (i as f32 / steps as f32) * span;
                let p = center + radius * egui::vec2(angle.cos(), angle.sin());
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: p,
                    uv,
                    color: fill_color,
                });
            }
            for i in 1..=(steps as u32) {
                mesh.indices.extend_from_slice(&[0, i, i + 1]);
            }
            painter.add(egui::Shape::mesh(mesh));
        }

        // Remaining seconds in the center. Estimated (party-call) windows append
        // a `?` to flag that the time isn't exact.
        let label = if estimated {
            format!("{}?", secs)
        } else {
            secs.to_string()
        };
        painter.text(
            center,
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(11.0),
            Color32::WHITE,
        );
    }

    /// Build a hover tooltip describing a dungeon's reward bonuses, in the order
    /// loot, dust, XP. Returns `None` when all bonuses are zero.
    fn dungeon_bonus_tooltip(dungeon: &DungeonEntry) -> Option<String> {
        let mut lines = Vec::new();
        if dungeon.loot_bonus > 0 {
            lines.push(format!("Loot bonus: +{}%", dungeon.loot_bonus));
        }
        if dungeon.dust_bonus > 0 {
            lines.push(format!("Dust bonus: +{}%", dungeon.dust_bonus));
        }
        if dungeon.xp_bonus > 0 {
            lines.push(format!("XP bonus: +{}%", dungeon.xp_bonus));
        }
        if lines.is_empty() {
            None
        } else {
            Some(lines.join("\n"))
        }
    }
}

impl Panel for LiveFeedPanel {
    fn show(&mut self, ui: &mut egui::Ui, ctx: &mut PanelContext) -> Vec<AppAction> {
        // A captured connection means the *verified* main loaded a map this
        // session. A mule tentatively accepted before the main (Connected but
        // not yet verified) must not clear the hint -- only the main's own map
        // change should.
        if ctx.account_verified {
            self.has_captured_connection = true;
        }
        // Show the mid-game capture hint only until the first connection is
        // captured; ignore the transient disconnects that portals cause.
        self.awaiting_first_map = !self.has_captured_connection;
        let labels = ctx.labels;
        let account_data = ctx.account_data;
        let get_label = |char_id: i32| -> Option<String> { labels.get(&char_id).cloned() };
        let get_appearance = |char_id: i32| -> Option<(i32, u32, u32)> {
            crate::panels::character_appearance(account_data, char_id)
        };
        self.render(
            ui,
            ctx.sprite_renderer,
            get_label,
            get_appearance,
            ctx.shadcn,
        );
        let mut actions: Vec<AppAction> = Vec::new();
        if self.pending_header_settings_save {
            self.pending_header_settings_save = false;
            actions.push(AppAction::SaveLiveFeedHeaderSettings {
                call_for_party: self.call_for_party,
                include_server_name: self.include_server_name,
                include_realm_name: self.include_realm_name,
            });
        }
        if self.pending_filter_settings_save {
            self.pending_filter_settings_save = false;
            actions.push(AppAction::SaveLiveFeedFilterSettings(
                self.current_filter_settings(),
            ));
        }
        if self.pending_clip_buttons_save {
            self.pending_clip_buttons_save = false;
            actions.push(AppAction::SaveQuickClipButtons(
                self.custom_clip_buttons.clone(),
            ));
        }
        actions.append(&mut self.pending_actions);
        actions
    }

    fn handle_event(
        &mut self,
        event: &realmhound_core::GameEvent,
        _session: &realmhound_core::GameSession,
    ) -> Vec<AppAction> {
        match event {
            realmhound_core::GameEvent::RealmScoreChanged { score } => {
                self.update_realm_score(*score);
            }
            realmhound_core::GameEvent::MapChanged {
                ref display_name,
                ref realm_name,
                fp,
                current_realm_score,
                max_realm_score,
                is_dungeon,
                ref dungeon_modifiers,
                ref dungeon_grade,
                ..
            } => {
                self.update_location(
                    display_name,
                    realm_name,
                    *current_realm_score,
                    *max_realm_score,
                );
                self.clear_encounters();
                self.currently_in_dungeon = *is_dungeon;

                if *is_dungeon {
                    self.push_dungeon(*fp, display_name, dungeon_modifiers, dungeon_grade.clone());
                } else {
                    // Left the dungeon (hub/realm): you can no longer call it, so
                    // expire any still-active dungeon entries and allow a fresh
                    // entry on re-entry. A non-dungeon transition also voids any
                    // pending portal anchor (e.g. a realm/nexus portal we used).
                    self.deactivate_active_dungeons();
                    self.clear_dungeon_signature();
                    self.pending_portal_spawn = None;
                }
                // Portal spawn times are map-local; drop them when the map changes.
                self.portal_spawns.clear();
            }
            realmhound_core::GameEvent::UpdateReceived(update, _) => {
                let am = realmhound_core::assets::get_asset_manager();
                for obj in &update.new_objects {
                    if am.is_portal_object(obj.object_type as i32) {
                        // Keep the first-seen spawn time; re-seeing the same
                        // portal must not reset its countdown. Drops are not
                        // pruned (map change clears the whole map).
                        self.portal_spawns
                            .entry(obj.object_id())
                            .or_insert_with(std::time::Instant::now);
                    }
                }
            }
            realmhound_core::GameEvent::PortalUsed { object_id } => {
                self.pending_portal_spawn = self.portal_spawns.get(object_id).copied();
            }
            realmhound_core::GameEvent::DustUpdated {
                amounts,
                is_seasonal,
            } => {
                self.update_dust(amounts.clone(), *is_seasonal);
            }
            realmhound_core::GameEvent::SeasonalStatusReceived { is_seasonal, .. } => {
                self.set_seasonal(*is_seasonal);
            }
            realmhound_core::GameEvent::TextReceived(ref text) => {
                if self.show_who_roster {
                    if let Some(names) = parse_who_roster(&text.text) {
                        self.push_who_roster(names);
                    }
                }
                // `/server` reply: authoritatively override the header server +
                // realm. Gated to system messages so player chat can't spoof it.
                if let Some((server, realm)) = parse_server_command_packet(text) {
                    self.apply_server_command(server, realm);
                }
            }
            _ => {}
        }
        vec![]
    }
}

/// Render the Live Feed Taskbar: a full-width, wrapping row of progress pills
/// for the tracked missions and Daily Quest Chests, followed by a separator.
pub(crate) fn render_taskbar_row(
    ui: &mut Ui,
    sr: &mut SpriteRenderer,
    shadcn: &Shadcn,
    items: &[crate::panels::taskbar::TaskbarItem],
    quests_stale: bool,
) {
    // Eligible items first (bright), dimmed items (ineligible tasks and chains
    // still waiting on a cooldown partner) pushed to the right regardless of
    // their progress. Within each group, more-progressed items come first; on
    // equal progress, missions sit ahead of (left of) standalone mark quests.
    // Stable, so remaining ties keep their build order.
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&a, &b| {
        items[a]
            .dim_right()
            .cmp(&items[b].dim_right())
            .then_with(|| {
                items[b]
                    .fraction()
                    .partial_cmp(&items[a].fraction())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| items[a].is_mark_quest().cmp(&items[b].is_mark_quest()))
    });

    // The Taskbar shares the darker Widget-Bar band; a full-width separator sets
    // it off from the Widget Bar above (mirroring how the Widget Bar is bracketed
    // from the tabs). The band's bottom inner margin and closing divider provide
    // the space below, so no trailing separator is drawn here.
    shadcn.full_width_separator(ui);

    ui.horizontal_wrapped(|ui| {
        ui.set_min_height(26.0);
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);

        // Leading compass icon marks the row as the Live Feed Taskbar. Stamp a
        // 1px black silhouette in the four cardinal offsets first so the icon
        // gets the same crisp outline the pill sprites carry.
        let (crect, cresp) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover());
        let icon_rect = crect.shrink(1.0);
        for off in [
            egui::vec2(-1.0, 0.0),
            egui::vec2(1.0, 0.0),
            egui::vec2(0.0, -1.0),
            egui::vec2(0.0, 1.0),
        ] {
            sr.draw_embedded_icon_tinted(
                ui,
                EmbeddedIcon::Compass,
                icon_rect.translate(off),
                Color32::BLACK,
            );
        }
        sr.draw_embedded_icon(ui, EmbeddedIcon::Compass, icon_rect);
        cresp.hover_tip("Taskbar - tracked mission and quest progress");

        for &i in &order {
            render_taskbar_item(ui, sr, shadcn, &items[i], quests_stale);
        }
    });
}

/// One drawn segment of a Taskbar chip: an optional text prefix (the grave
/// difficulty range), an icon, and a trailing count string.
struct ChipSeg {
    prefix: Option<String>,
    icon: crate::panels::missions::ObjIcon,
    text: String,
}

/// Draw one white-outlined chip (matching the Widget Bar) from grouped
/// segments. Segments within a group use plain spacing; groups are separated by
/// a `|` when `choice` (mutually-exclusive varieties). Returns the hover
/// response so the caller can attach a tooltip.
fn paint_chip(
    ui: &mut Ui,
    sr: &mut SpriteRenderer,
    shadcn: &Shadcn,
    groups: &[Vec<ChipSeg>],
    choice: bool,
    dimmed: bool,
    stale: bool,
) -> egui::Response {
    use crate::panels::missions::{paint_obj_icon, ObjIcon};
    // Stale (expired, un-refetched) quest pills read as dimmed too, but carry a
    // red outline instead of the normal border.
    let faded = dimmed || stale;
    let text_col = if faded {
        Color32::from_gray(150)
    } else {
        Color32::from_gray(225)
    };
    let font = egui::FontId::proportional(12.5);
    let layout = |ui: &Ui, s: &str, col: Color32| {
        ui.painter()
            .layout_no_wrap(s.to_string(), font.clone(), col)
    };

    let icon_sz = 20.0;
    let pad = 6.0;
    let gap = 4.0;
    let prefix_gap = 1.0;
    let seg_gap = 8.0;

    struct Prepared {
        prefix: Option<std::sync::Arc<egui::Galley>>,
        icon: ObjIcon,
        text: std::sync::Arc<egui::Galley>,
    }
    let prepared: Vec<Vec<Prepared>> = groups
        .iter()
        .map(|g| {
            g.iter()
                .map(|s| Prepared {
                    prefix: s
                        .prefix
                        .as_ref()
                        .filter(|p| !p.is_empty())
                        .map(|p| layout(ui, p, text_col)),
                    icon: s.icon.clone(),
                    text: layout(ui, &s.text, text_col),
                })
                .collect()
        })
        .collect();

    // Choice varieties separate with a `|`; everything else uses plain spacing.
    let sep_gal = choice.then(|| layout(ui, "|", Color32::from_gray(140)));

    let seg_w = |p: &Prepared| {
        let pre = p.prefix.as_ref().map_or(0.0, |g| g.size().x + prefix_gap);
        pre + icon_sz + gap + p.text.size().x
    };
    let mut content_w = 0.0_f32;
    for (gi, g) in prepared.iter().enumerate() {
        if gi > 0 {
            content_w += match &sep_gal {
                Some(sg) => seg_gap + sg.size().x + seg_gap,
                None => seg_gap,
            };
        }
        for (si, p) in g.iter().enumerate() {
            if si > 0 {
                content_w += seg_gap;
            }
            content_w += seg_w(p);
        }
    }

    let h = 26.0;
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(content_w + pad * 2.0, h), egui::Sense::hover());

    let bg = shadcn.colors().secondary;
    let border = shadcn.colors().border;
    let (bg, border) = if stale {
        (
            Color32::from_rgba_unmultiplied(bg.r(), bg.g(), bg.b(), 150),
            Color32::from_rgb(210, 55, 55),
        )
    } else if dimmed {
        (
            Color32::from_rgba_unmultiplied(bg.r(), bg.g(), bg.b(), 150),
            Color32::from_rgba_unmultiplied(border.r(), border.g(), border.b(), 120),
        )
    } else {
        (bg, border)
    };
    ui.painter().rect_filled(rect, 4.0, bg);
    ui.painter().rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(1.0_f32, border),
        egui::StrokeKind::Inside,
    );

    let tint = if faded {
        Color32::from_gray(150)
    } else {
        Color32::WHITE
    };
    let mut x = rect.left() + pad;
    for (gi, g) in prepared.iter().enumerate() {
        if gi > 0 {
            if let Some(sg) = &sep_gal {
                let sx = x + seg_gap;
                ui.painter().galley(
                    egui::pos2(sx, rect.center().y - sg.size().y * 0.5),
                    sg.clone(),
                    Color32::from_gray(140),
                );
                x = sx + sg.size().x + seg_gap;
            } else {
                x += seg_gap;
            }
        }
        for (si, p) in g.iter().enumerate() {
            if si > 0 {
                x += seg_gap;
            }
            if let Some(pg) = &p.prefix {
                ui.painter().galley(
                    egui::pos2(x, rect.center().y - pg.size().y * 0.5),
                    pg.clone(),
                    text_col,
                );
                x += pg.size().x + prefix_gap;
            }
            let icon_rect = egui::Rect::from_min_size(
                egui::pos2(x, rect.center().y - icon_sz * 0.5),
                egui::vec2(icon_sz, icon_sz),
            );
            paint_obj_icon(ui, sr, &p.icon, icon_rect, tint);
            let text_x = icon_rect.right() + gap;
            ui.painter().galley(
                egui::pos2(text_x, rect.center().y - p.text.size().y * 0.5),
                p.text.clone(),
                text_col,
            );
            x = text_x + p.text.size().x;
        }
    }
    resp
}

/// Chip segment-groups for a mission pill. A choice (OR) mission yields one
/// group per variety (rendered `|`-separated); everything else is one group.
fn mission_chip_groups(m: &crate::panels::taskbar::MissionTask) -> (Vec<Vec<ChipSeg>>, bool) {
    let segs: Vec<ChipSeg> = m
        .objs
        .iter()
        .map(|o| ChipSeg {
            prefix: (!o.prefix.is_empty()).then(|| o.prefix.clone()),
            icon: o.icon.clone(),
            text: if o.qualifier.is_empty() {
                format!("x {}", o.remaining)
            } else {
                format!("{} x {}", o.qualifier, o.remaining)
            },
        })
        .collect();
    if m.choice {
        (segs.into_iter().map(|s| vec![s]).collect(), true)
    } else {
        (vec![segs], false)
    }
}

/// Chip segment-groups for a standalone mark-quest pill: one `[mark] x N` group
/// per still-needed mark type.
fn quest_chip_groups(q: &crate::panels::taskbar::QuestTask) -> Vec<Vec<ChipSeg>> {
    use crate::panels::missions::ObjIcon;
    let segs: Vec<ChipSeg> = q
        .marks
        .iter()
        .map(|mk| ChipSeg {
            prefix: None,
            icon: ObjIcon::Object(mk.mark_id),
            text: format!("x {}", mk.remaining),
        })
        .collect();
    vec![segs]
}

/// Chip segment-groups for a combined mission+quest pill. Per dungeon variety
/// the pill shows the mark pickups first, then any remaining portal-only runs:
/// each of the first `mark_count` dungeon runs also drops a mark, so only
/// `dungeon_runs - mark_count` runs are portal-only. When the marks already
/// cover every run (`mark_count >= dungeon_runs`) just the marks show; with no
/// marks left just the portals show. A choice/tie yields one `|`-separated
/// group per variety.
fn combined_chip_groups(variants: &[crate::panels::taskbar::CombinedVariant]) -> Vec<Vec<ChipSeg>> {
    use crate::panels::missions::ObjIcon;
    variants
        .iter()
        .filter_map(|v| {
            let mut segs: Vec<ChipSeg> = Vec::new();
            if v.mark_id > 0 && v.mark_count > 0 {
                segs.push(ChipSeg {
                    prefix: None,
                    icon: ObjIcon::Object(v.mark_id),
                    text: format!("x {}", v.mark_count),
                });
                let extra_portals = v.dungeon_runs - v.mark_count;
                if extra_portals > 0 {
                    segs.push(ChipSeg {
                        prefix: None,
                        icon: v.portal_icon.clone(),
                        text: format!("x {extra_portals}"),
                    });
                }
            } else if v.dungeon_runs > 0 {
                segs.push(ChipSeg {
                    prefix: None,
                    icon: v.portal_icon.clone(),
                    text: format!("x {}", v.dungeon_runs),
                });
            }
            (!segs.is_empty()).then_some(segs)
        })
        .collect()
}

/// Render one processed Taskbar item: a plain pill, a fused mission+quest pill,
/// or a chain of linked missions.
fn render_taskbar_item(
    ui: &mut Ui,
    sr: &mut SpriteRenderer,
    shadcn: &Shadcn,
    item: &crate::panels::taskbar::TaskbarItem,
    quests_stale: bool,
) {
    use crate::panels::taskbar::TaskbarItem;
    match item {
        TaskbarItem::Single(t) => render_single_pill(ui, sr, shadcn, t, quests_stale),
        TaskbarItem::Combined(c) => render_combined_pill(ui, sr, shadcn, c, quests_stale),
        TaskbarItem::Chain(g) => render_chain(ui, sr, shadcn, g, quests_stale),
    }
}

/// Render a plain mission or quest pill. Hovering shows the full card.
fn render_single_pill(
    ui: &mut Ui,
    sr: &mut SpriteRenderer,
    shadcn: &Shadcn,
    task: &crate::panels::taskbar::TaskbarTask,
    quests_stale: bool,
) {
    use crate::panels::taskbar::TaskbarTask;
    let dimmed = !task.eligible();
    let stale = quests_stale && matches!(task, TaskbarTask::Quest(_));
    let (groups, choice) = match task {
        TaskbarTask::Mission(m) => mission_chip_groups(m),
        TaskbarTask::Quest(q) => (quest_chip_groups(q), false),
    };
    if groups.iter().all(|g| g.is_empty()) {
        return;
    }
    let resp = paint_chip(ui, sr, shadcn, &groups, choice, dimmed, stale);
    if stale {
        resp.hover_tip_ui(|ui| {
            ui.label(
                RichText::new(crate::panels::quest::STALE_QUEST_TOOLTIP)
                    .italics()
                    .color(Color32::from_rgb(210, 55, 55)),
            );
        });
        return;
    }
    resp.hover_tip_ui(|ui| match task {
        TaskbarTask::Mission(m) => {
            crate::panels::missions::render_mission_tooltip(
                ui,
                sr,
                &m.entry,
                m.current_char,
                &m.tooltip_objs,
                m.choice,
            );
        }
        TaskbarTask::Quest(q) => {
            crate::panels::quest::render_quest_tooltip(
                ui,
                sr,
                &q.quest,
                &q.item_counts,
                q.seasonal,
                &std::collections::HashSet::new(),
            );
        }
    });
}

/// Render a fused mission+quest pill. The hover shows the Mission and the
/// Quest(s) side by side in two columns, separated by a vertical divider.
fn render_combined_pill(
    ui: &mut Ui,
    sr: &mut SpriteRenderer,
    shadcn: &Shadcn,
    c: &crate::panels::taskbar::CombinedTask,
    quests_stale: bool,
) {
    let dimmed = !c.eligible;
    let groups = combined_chip_groups(&c.variants);
    if groups.is_empty() {
        return;
    }
    let resp = paint_chip(ui, sr, shadcn, &groups, c.choice, dimmed, quests_stale);
    // Only reproduce the objectives matching the pill's locked variant(s). A
    // choice (OR) mission that locked onto one dungeon must not list the drop
    // locations of the alternatives it didn't pick.
    let variant_dungeons: std::collections::HashSet<String> = c
        .variants
        .iter()
        .map(|v| crate::panels::taskbar::canonical_match_dungeon(&v.dungeon_name))
        .collect();
    let mut tooltip_objs: Vec<usize> = c
        .entry
        .objectives
        .iter()
        .enumerate()
        .filter(|(_, o)| {
            o.dungeon_name
                .as_deref()
                .map(crate::panels::taskbar::canonical_match_dungeon)
                .is_some_and(|d| variant_dungeons.contains(&d))
        })
        .map(|(i, _)| i)
        .collect();
    // Fallback (e.g. a grave-difficulty reframe whose objective has no dungeon
    // name to match): reproduce every objective as before.
    if tooltip_objs.is_empty() {
        tooltip_objs = (0..c.entry.objectives.len()).collect();
    }
    // The mission column already shows the drop locations for the dungeons it
    // renders; suppress those same dungeons in the quest column so an Advanced
    // Kogbold mark doesn't render a second (base-dungeon) "Drops from" section.
    let mission_dungeons: std::collections::HashSet<String> = tooltip_objs
        .iter()
        .filter_map(|&i| c.entry.objectives[i].dungeon_name.as_deref())
        .map(crate::panels::taskbar::canonical_match_dungeon)
        .collect();
    resp.hover_tip_ui(|ui| {
        // A grave-difficulty reframe clarifies which dungeon it now stands in for.
        if c.partial {
            if let Some(v) = c.variants.first() {
                ui.label(
                    egui::RichText::new(format!("Run {} to advance this mission", v.dungeon_name))
                        .italics()
                        .weak(),
                );
            }
        }
        ui.horizontal_top(|ui| {
            let divider = ui.visuals().widgets.noninteractive.bg_stroke;
            let mission_rect = ui
                .vertical(|ui| {
                    ui.set_max_width(300.0);
                    ui.label(
                        egui::RichText::new("Mission")
                            .size(11.0)
                            .strong()
                            .color(Color32::from_gray(150)),
                    );
                    crate::panels::missions::render_mission_tooltip(
                        ui,
                        sr,
                        &c.entry,
                        c.current_char,
                        &tooltip_objs,
                        c.choice,
                    );
                })
                .response
                .rect;

            ui.add_space(13.0);

            let quest_rect = ui
                .vertical(|ui| {
                    ui.set_max_width(300.0);
                    ui.label(
                        egui::RichText::new("Quest")
                            .size(11.0)
                            .strong()
                            .color(Color32::from_gray(150)),
                    );
                    for (i, q) in c.quests.iter().enumerate() {
                        if i > 0 {
                            ui.separator();
                        }
                        if quests_stale {
                            ui.label(
                                RichText::new(crate::panels::quest::STALE_QUEST_TOOLTIP)
                                    .italics()
                                    .color(Color32::from_rgb(210, 55, 55)),
                            );
                            continue;
                        }
                        crate::panels::quest::render_quest_tooltip(
                            ui,
                            sr,
                            q,
                            &c.item_counts,
                            c.seasonal,
                            &mission_dungeons,
                        );
                    }
                })
                .response
                .rect;

            // Vertical divider spanning only the taller of the two columns.
            let x = (mission_rect.right() + quest_rect.left()) * 0.5;
            let top = mission_rect.top().min(quest_rect.top());
            let bottom = mission_rect.bottom().max(quest_rect.bottom());
            ui.painter().vline(x, top..=bottom, divider);
        });
    });
}

/// Render a chain of linked missions as adjacent pills joined by a chain-link
/// glyph. A chain waiting on a cooldown partner is drawn dimmed. Pills are
/// allocated directly into the parent Taskbar row (not a nested layout) so they
/// share the same vertical centering as every other pill.
fn render_chain(
    ui: &mut Ui,
    sr: &mut SpriteRenderer,
    shadcn: &Shadcn,
    g: &crate::panels::taskbar::ChainGroup,
    quests_stale: bool,
) {
    use crate::panels::taskbar::TaskbarTask;
    let group_dim = g.dimmed();
    let prev_gap = ui.spacing().item_spacing.x;
    ui.spacing_mut().item_spacing.x = 4.0;
    let mut drawn = false;
    for member in g.members.iter() {
        let (groups, choice) = match &member.task {
            TaskbarTask::Mission(m) => mission_chip_groups(m),
            TaskbarTask::Quest(q) => (quest_chip_groups(q), false),
        };
        if groups.iter().all(|gr| gr.is_empty()) {
            continue;
        }
        // Only link members that actually render, so an empty member can't leave
        // an orphaned chain glyph.
        if drawn {
            draw_chain_link(ui, group_dim);
        }
        drawn = true;
        let stale = quests_stale && matches!(&member.task, TaskbarTask::Quest(_));
        let dimmed =
            member.dimmed || matches!(&member.task, TaskbarTask::Mission(m) if !m.eligible);
        let resp = paint_chip(ui, sr, shadcn, &groups, choice, dimmed, stale);
        if stale {
            resp.hover_tip_ui(|ui| {
                ui.label(
                    RichText::new(crate::panels::quest::STALE_QUEST_TOOLTIP)
                        .italics()
                        .color(Color32::from_rgb(210, 55, 55)),
                );
            });
            continue;
        }
        match &member.task {
            TaskbarTask::Mission(m) => {
                resp.hover_tip_ui(|ui| {
                    crate::panels::missions::render_mission_tooltip(
                        ui,
                        sr,
                        &m.entry,
                        m.current_char,
                        &m.tooltip_objs,
                        m.choice,
                    );
                });
            }
            TaskbarTask::Quest(q) => {
                resp.hover_tip_ui(|ui| {
                    crate::panels::quest::render_quest_tooltip(
                        ui,
                        sr,
                        &q.quest,
                        &q.item_counts,
                        q.seasonal,
                        &std::collections::HashSet::new(),
                    );
                });
            }
        }
    }
    ui.spacing_mut().item_spacing.x = prev_gap;
}

/// Draw the chain-link glyph (two interlocking rings) between chained pills.
fn draw_chain_link(ui: &mut Ui, dim: bool) {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(16.0, 26.0), egui::Sense::hover());
    let col = if dim {
        Color32::from_gray(120)
    } else {
        Color32::from_gray(200)
    };
    let c = rect.center();
    for dx in [-3.0_f32, 3.0] {
        let r = egui::Rect::from_center_size(egui::pos2(c.x + dx, c.y), egui::vec2(9.0, 6.0));
        ui.painter().rect_stroke(
            r,
            3.0,
            egui::Stroke::new(1.5_f32, col),
            egui::StrokeKind::Inside,
        );
    }
    resp.hover_tip("Run these together - progressing one advances the other");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combined_variant(
        mark_id: i32,
        mark_count: i32,
        dungeon_runs: i32,
    ) -> crate::panels::taskbar::CombinedVariant {
        crate::panels::taskbar::CombinedVariant {
            dungeon_name: "Ocean Trench".to_string(),
            portal_icon: crate::panels::missions::ObjIcon::None,
            mark_id,
            dungeon_runs,
            mark_count,
        }
    }

    #[test]
    fn combined_split_shows_marks_then_leftover_portals() {
        // 4 dungeon runs, 2 marks left: collect 2 marks (which also clear 2
        // runs), then 2 portal-only runs.
        let groups = combined_chip_groups(&[combined_variant(50, 2, 4)]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 2);
        assert_eq!(groups[0][0].text, "x 2");
        assert_eq!(groups[0][1].text, "x 2");
    }

    #[test]
    fn combined_split_marks_only_when_marks_cover_runs() {
        // Marks >= runs: every run drops a mark, so only the marks show.
        let groups = combined_chip_groups(&[combined_variant(50, 4, 4)]);
        assert_eq!(groups[0].len(), 1);
        assert_eq!(groups[0][0].text, "x 4");

        let groups = combined_chip_groups(&[combined_variant(50, 5, 4)]);
        assert_eq!(groups[0].len(), 1);
        assert_eq!(groups[0][0].text, "x 5");
    }

    #[test]
    fn combined_split_portals_only_when_no_marks() {
        // No marks left (or a portal-only variant): just the dungeon runs.
        let groups = combined_chip_groups(&[combined_variant(50, 0, 4)]);
        assert_eq!(groups[0].len(), 1);
        assert_eq!(groups[0][0].text, "x 4");

        let groups = combined_chip_groups(&[combined_variant(0, 0, 3)]);
        assert_eq!(groups[0].len(), 1);
        assert_eq!(groups[0][0].text, "x 3");
    }

    /// Owned default callout inputs for tests. Returned as a tuple so the
    /// borrowed [`DungeonCalloutParams`] can reference stable locals.
    fn test_callout_inputs() -> (
        Vec<realmhound_core::settings::RewardModEntry>,
        std::collections::BTreeMap<String, String>,
    ) {
        (
            realmhound_core::settings::default_reward_mods(),
            std::collections::BTreeMap::new(),
        )
    }

    /// Build default [`DungeonCalloutParams`] borrowing the provided owned inputs.
    fn test_params<'a>(
        mods: &'a [realmhound_core::settings::RewardModEntry],
        overrides: &'a std::collections::BTreeMap<String, String>,
    ) -> crate::panels::dungeon_callout::DungeonCalloutParams<'a> {
        crate::panels::dungeon_callout::DungeonCalloutParams {
            name_style: realmhound_core::settings::DungeonNameStyle::Short,
            name_overrides: overrides,
            loot_label: realmhound_core::settings::LootLabel::Lb,
            dust_label: realmhound_core::settings::DustLabel::Db,
            xp_label: realmhound_core::settings::XpLabel::None,
            percent: false,
            reward_mods: mods,
        }
    }

    fn boss_call_count(panel: &LiveFeedPanel) -> usize {
        panel
            .entries
            .iter()
            .filter(|e| matches!(e, FeedEntry::BossCall(_)))
            .count()
    }

    fn dust_full_count(panel: &LiveFeedPanel) -> usize {
        panel
            .entries
            .iter()
            .filter(|e| matches!(e, FeedEntry::DustFull(_)))
            .count()
    }

    fn sample_dungeon_entry() -> FeedEntry {
        let (mods, ov) = test_callout_inputs();
        let params = test_params(&mods, &ov);
        FeedEntry::Dungeon(DungeonEntry::new(
            "Snake Pit".to_string(),
            Some(1234),
            &[],
            None,
            &params,
            None,
            None,
            std::time::Instant::now(),
            false,
        ))
    }

    #[test]
    fn notification_and_warning_toggles_gate_entry_visibility() {
        let mut panel = LiveFeedPanel::new();

        let dungeon = sample_dungeon_entry();
        let boss =
            FeedEntry::BossCall(BossCallEntry::new(2471, "Cube God".to_string(), None, None));
        let warning = FeedEntry::Warning(WarningEntry::new(
            "Realm closed!".to_string(),
            WarningSeverity::Red,
            false,
        ));
        let dust = FeedEntry::DustFull(DustFullEntry::new(false, DustType::Red));

        // Defaults: everything passes.
        assert!(panel.entry_passes_filter(&dungeon));
        assert!(panel.entry_passes_filter(&boss));
        assert!(panel.entry_passes_filter(&warning));
        assert!(panel.entry_passes_filter(&dust));

        panel.show_dungeon_entries = false;
        panel.show_boss_calls = false;
        panel.show_realm_warnings = false;
        panel.show_dust_full = false;

        assert!(!panel.entry_passes_filter(&dungeon));
        assert!(!panel.entry_passes_filter(&boss));
        assert!(!panel.entry_passes_filter(&warning));
        assert!(!panel.entry_passes_filter(&dust));
    }

    #[test]
    fn loot_drops_toggle_hides_loot_entries() {
        let mut panel = LiveFeedPanel::new();
        let boss =
            FeedEntry::BossCall(BossCallEntry::new(2471, "Cube God".to_string(), None, None));

        // Turning off loot drops must not affect non-loot entries.
        panel.show_loot_drops = false;
        assert!(panel.entry_passes_filter(&boss));
    }

    #[test]
    fn push_key_pop_adds_entry_without_expiry() {
        let mut panel = LiveFeedPanel::new();
        panel.push_key_pop(1234, "Snake Pit".to_string(), "Alice".to_string(), true);
        let entry = panel
            .entries
            .iter()
            .find_map(|e| match e {
                FeedEntry::KeyPop(k) => Some(k),
                _ => None,
            })
            .expect("key pop entry present");
        assert_eq!(entry.opener, "Alice");
        assert_eq!(entry.dungeon_name, "Snake Pit");
        assert_eq!(entry.key_id, 1234);
        assert_eq!(
            entry.callout(),
            Some("Thanks Alice for the key".to_string())
        );
    }

    #[test]
    fn dust_full_notifies_once_on_transition() {
        let mut panel = LiveFeedPanel::new();

        // Green below cap: no notification.
        let mut amounts = DustAmounts::new();
        amounts.green = (1900, 2000);
        amounts.red = (500, 2000);
        amounts.purple = (500, 2000);
        panel.update_dust(amounts.clone(), false);
        assert_eq!(dust_full_count(&panel), 0);

        // Green reaches cap: one notification.
        amounts.green = (2000, 2000);
        panel.update_dust(amounts.clone(), false);
        assert_eq!(dust_full_count(&panel), 1);

        // Still full on the next update: no new notification.
        panel.update_dust(amounts.clone(), false);
        assert_eq!(dust_full_count(&panel), 1);

        // A different type fills: a second notification.
        amounts.red = (2000, 2000);
        panel.update_dust(amounts, false);
        assert_eq!(dust_full_count(&panel), 2);
    }

    #[test]
    fn dust_full_separate_per_account() {
        let mut panel = LiveFeedPanel::new();

        // Seed both accounts below cap (first observation does not notify).
        let mut below = DustAmounts::new();
        below.green = (0, 2000);
        below.red = (0, 2000);
        below.purple = (0, 2000);
        panel.update_dust(below.clone(), false);
        panel.update_dust(below, true);
        assert_eq!(dust_full_count(&panel), 0);

        // Fill green on each account: one notification per account.
        let mut full_green = DustAmounts::new();
        full_green.green = (2000, 2000);
        full_green.red = (0, 2000);
        full_green.purple = (0, 2000);
        panel.update_dust(full_green.clone(), false);
        panel.update_dust(full_green, true);
        assert_eq!(dust_full_count(&panel), 2);
    }

    #[test]
    fn dust_full_first_observation_already_full_does_not_notify() {
        let mut panel = LiveFeedPanel::new();

        let mut full_green = DustAmounts::new();
        full_green.green = (2000, 2000);
        full_green.red = (0, 2000);
        full_green.purple = (0, 2000);

        // First-ever update for the account already at cap: seed only.
        panel.update_dust(full_green, false);
        assert_eq!(dust_full_count(&panel), 0);
    }

    #[test]
    fn crystal_first_callout_sounds_then_pins_without_feed_entry() {
        let mut panel = LiveFeedPanel::new();
        panel.realm_name = Some("Mesa".to_string());
        // First Sweet Treasure callout: sounds and creates the pin.
        assert!(panel.push_boss_call(2471, "Mysterious Crystal".to_string()));
        assert!(panel.crystal_pin.is_some());
        // Repeat announcements refresh the timer but must not re-sound.
        for _ in 0..5 {
            assert!(!panel.push_boss_call(2471, "Mysterious Crystal".to_string()));
        }
        // The Crystal is pinned, never added to the scrolling feed.
        assert_eq!(boss_call_count(&panel), 0);
    }

    #[test]
    fn crystal_re_sounds_in_a_new_realm() {
        let mut panel = LiveFeedPanel::new();
        panel.realm_name = Some("Mesa".to_string());
        assert!(panel.push_boss_call(2471, "Mysterious Crystal".to_string()));
        // A different realm is a fresh sighting -> sound again.
        panel.realm_name = Some("Ocean".to_string());
        assert!(panel.push_boss_call(2471, "Mysterious Crystal".to_string()));
    }

    #[test]
    fn crystal_pin_survives_leaving_the_realm() {
        let mut panel = LiveFeedPanel::new();
        panel.realm_name = Some("Mesa".to_string());
        assert!(panel.push_boss_call(2471, "Mysterious Crystal".to_string()));
        assert!(panel.crystal_pin.is_some());
        // Leaving to the Nexus must NOT drop the pin -- the player keeps the
        // knowledge that a Crystal was heard.
        panel.update_location("Nexus", "Nexus", 0, 0);
        assert!(panel.crystal_pin.is_some());
        // Diving into a dungeon from a different realm also keeps it.
        panel.update_location("Spider Den", "NexusPortal.Ocean", 0, 0);
        assert!(panel.crystal_pin.is_some());
    }

    #[test]
    fn crystal_pin_cleared_on_disconnect() {
        let mut panel = LiveFeedPanel::new();
        panel.realm_name = Some("Mesa".to_string());
        assert!(panel.push_boss_call(2471, "Mysterious Crystal".to_string()));
        panel.clear_realm_state();
        assert!(panel.crystal_pin.is_none());
    }

    #[test]
    fn unpinning_folds_crystal_into_the_feed_with_its_real_time() {
        let mut panel = LiveFeedPanel::new();
        panel.realm_name = Some("Mesa".to_string());
        assert!(panel.push_boss_call(2471, "Mysterious Crystal".to_string()));
        let pin = panel.crystal_pin.take().expect("pin exists");
        let at = pin.last_spoke_at;
        panel.fold_crystal_pin_into_feed(&pin);
        assert_eq!(boss_call_count(&panel), 1);
        let entry = panel
            .entries
            .iter()
            .find_map(|e| match e {
                FeedEntry::BossCall(b) => Some(b),
                _ => None,
            })
            .unwrap();
        assert_eq!(entry.boss_name, "Mysterious Crystal");
        assert_eq!(entry.timestamp, at);
    }

    #[test]
    fn distinct_boss_callouts_are_kept() {
        let mut panel = LiveFeedPanel::new();
        panel.push_boss_call(22128, "Beer God".to_string());
        panel.push_boss_call(22129, "Grand Sphinx".to_string());
        // Different bosses must each produce their own callout.
        assert_eq!(boss_call_count(&panel), 2);
    }

    #[test]
    fn new_alien_wave_expires_previous_waves_in_same_realm() {
        let mut panel = LiveFeedPanel::new();
        panel.realm_name = Some("Mesa".to_string());
        assert!(panel.push_boss_call(1, "Alien Invasion Adept - Wave 1".to_string()));
        assert!(panel.push_boss_call(2, "Alien Invasion Adept - Wave 2".to_string()));
        let waves: Vec<&BossCallEntry> = panel
            .entries
            .iter()
            .filter_map(|e| match e {
                FeedEntry::BossCall(b) => Some(b),
                _ => None,
            })
            .collect();
        assert_eq!(waves.len(), 2);
        // Newest-first: Wave 2 active, Wave 1 expired.
        assert_eq!(waves[0].boss_name, "Alien Invasion Adept - Wave 2");
        assert!(!waves[0].expired);
        assert_eq!(waves[1].boss_name, "Alien Invasion Adept - Wave 1");
        assert!(waves[1].expired);
    }

    #[test]
    fn alien_wave_in_a_different_realm_does_not_expire_prior_realm() {
        let mut panel = LiveFeedPanel::new();
        panel.realm_name = Some("Mesa".to_string());
        assert!(panel.push_boss_call(1, "Alien Invasion Adept - Wave 1".to_string()));
        // A wave in a different realm must not expire Mesa's Wave 1.
        panel.realm_name = Some("Ocean".to_string());
        assert!(panel.push_boss_call(2, "Alien Invasion Adept - Wave 2".to_string()));
        let wave1 = panel
            .entries
            .iter()
            .find_map(|e| match e {
                FeedEntry::BossCall(b) if b.boss_name.ends_with("Wave 1") => Some(b),
                _ => None,
            })
            .unwrap();
        assert!(!wave1.expired);
    }

    #[test]
    fn repeated_boss_callout_deduped_within_realm() {
        let mut panel = LiveFeedPanel::new();
        panel.realm_name = Some("Mesa".to_string());
        assert!(panel.push_boss_call(22128, "Beer God".to_string()));
        assert!(!panel.push_boss_call(22128, "Beer God".to_string()));
        assert_eq!(boss_call_count(&panel), 1);
        // Same boss in a different realm is a new sighting.
        panel.realm_name = Some("Ocean".to_string());
        assert!(panel.push_boss_call(22128, "Beer God".to_string()));
        assert_eq!(boss_call_count(&panel), 2);
    }

    #[test]
    fn repeatable_encounter_respawn_after_window_re_notifies() {
        // A fast, repeatable encounter (Skull Shrine) that respawns after the
        // short dedup window must notify again -- the previous long window
        // silently swallowed genuine new spawns.
        let mut panel = LiveFeedPanel::new();
        panel.realm_name = Some("Mesa".to_string());
        assert!(panel.push_boss_call(22003, "Skull Shrine".to_string()));
        assert_eq!(boss_call_count(&panel), 1);
        // Age the stored callout just past the dedup window.
        let aged = Local::now() - BOSS_CALL_DEDUP_WINDOW - chrono::Duration::seconds(1);
        for entry in &mut panel.entries {
            if let FeedEntry::BossCall(call) = entry {
                call.timestamp = aged;
            }
        }
        // The next spawn of the same shrine in the same realm is a fresh sighting.
        assert!(panel.push_boss_call(22003, "Skull Shrine".to_string()));
        assert_eq!(boss_call_count(&panel), 2);
    }

    #[test]
    fn build_event_body_crystal_uses_short_name() {
        // Mysterious Crystal's short name is "cry".
        let panel = LiveFeedPanel::new();
        let boss = BossCallEntry::new(2471, "Mysterious Crystal".to_string(), None, None);
        assert_eq!(panel.build_event_body(&boss), "cry");
        // With default settings (call for party, event join = none), the
        // assembled callout is "/p cry" regardless of realm info.
        assert_eq!(
            panel.assemble_callout(
                &panel.build_event_body(&boss),
                boss.server_name.as_deref(),
                boss.realm_name.as_deref(),
                panel.event_join_position
            ),
            "/p cry"
        );
    }

    #[test]
    fn build_event_body_unknown_boss_falls_back_to_full_name() {
        let panel = LiveFeedPanel::new();
        let boss = BossCallEntry::new(22128, "Some Unknown Boss".to_string(), None, None);
        assert_eq!(panel.build_event_body(&boss), "Some Unknown Boss");
        assert_eq!(
            panel.assemble_callout(
                &panel.build_event_body(&boss),
                boss.server_name.as_deref(),
                boss.realm_name.as_deref(),
                panel.event_join_position
            ),
            "/p Some Unknown Boss"
        );
    }

    #[test]
    fn assemble_callout_honors_party_server_and_join_settings() {
        use realmhound_core::settings::{JoinPosition, LiveFeedSettings};
        let mut panel = LiveFeedPanel::new();
        panel.set_server_name("USMidWest2".to_string());

        // Dungeon join defaults to end: /p + end join, server off.
        assert_eq!(
            panel.assemble_callout(
                "rav rot",
                panel.server_name.as_deref(),
                panel.realm_name.as_deref(),
                panel.dungeon_join_position
            ),
            "/p rav rot j"
        );

        // Server name on: short code is inserted after /p (and after any
        // beginning join).
        panel.apply_live_feed_settings(&LiveFeedSettings {
            include_server_name: true,
            ..Default::default()
        });
        assert_eq!(
            panel.assemble_callout(
                "rav rot",
                panel.server_name.as_deref(),
                panel.realm_name.as_deref(),
                panel.dungeon_join_position
            ),
            "/p USMW2 rav rot j"
        );

        // Join at beginning, right after /p.
        panel.apply_live_feed_settings(&LiveFeedSettings {
            include_server_name: true,
            dungeon_join_position: JoinPosition::Beginning,
            ..Default::default()
        });
        assert_eq!(
            panel.assemble_callout(
                "rav rot",
                panel.server_name.as_deref(),
                panel.realm_name.as_deref(),
                panel.dungeon_join_position
            ),
            "/p j USMW2 rav rot"
        );

        // No party prefix, no join marker.
        panel.apply_live_feed_settings(&LiveFeedSettings {
            call_for_party: false,
            dungeon_join_position: JoinPosition::None,
            ..Default::default()
        });
        assert_eq!(
            panel.assemble_callout(
                "rav rot",
                panel.server_name.as_deref(),
                panel.realm_name.as_deref(),
                panel.dungeon_join_position
            ),
            "rav rot"
        );

        // Realm name on: inserted after server name.
        panel.realm_name = Some("Medusa".to_string());
        panel.apply_live_feed_settings(&LiveFeedSettings {
            include_server_name: true,
            include_realm_name: true,
            ..Default::default()
        });
        assert_eq!(
            panel.assemble_callout(
                "rav rot",
                panel.server_name.as_deref(),
                panel.realm_name.as_deref(),
                panel.dungeon_join_position
            ),
            "/p USMW2 Medusa rav rot j"
        );

        // Realm name on without server name.
        panel.apply_live_feed_settings(&LiveFeedSettings {
            include_realm_name: true,
            ..Default::default()
        });
        assert_eq!(
            panel.assemble_callout(
                "rav rot",
                panel.server_name.as_deref(),
                panel.realm_name.as_deref(),
                panel.dungeon_join_position
            ),
            "/p Medusa rav rot j"
        );
    }

    fn dungeon_count(panel: &LiveFeedPanel) -> usize {
        panel
            .entries
            .iter()
            .filter(|e| matches!(e, FeedEntry::Dungeon(_)))
            .count()
    }

    #[test]
    fn prettify_modifier_formats_tokens() {
        assert_eq!(prettify_modifier("CHEF"), "Chef");
        assert_eq!(prettify_modifier("SOUVENIR_1"), "Souvenir 1");
        assert_eq!(prettify_modifier("PETCOLLECTOR"), "Petcollector");
    }

    #[test]
    fn format_dungeon_name_resolves_and_falls_back() {
        // Known localization key resolved via portal map.
        assert_eq!(format_dungeon_name("{s.wine_cellar}"), "Wine Cellar");
        // Unknown localization key falls back to stripped + title-cased form.
        assert_eq!(format_dungeon_name("{s.spider_den}"), "Spider Den");
        // Already-readable names pass through unchanged.
        assert_eq!(format_dungeon_name("The Shatters"), "The Shatters");
    }

    #[test]
    fn push_dungeon_computes_loot_bonus_and_display_names() {
        let _assets = crate::test_support::modifier_assets();
        let mut panel = LiveFeedPanel::new();
        panel.push_dungeon(
            1,
            "Snake Pit",
            &["LOOTING".to_string(), "REWARDING".to_string()],
            Some("S".to_string()),
        );
        match panel.entries.front().unwrap() {
            FeedEntry::Dungeon(d) => {
                assert_eq!(
                    d.modifiers,
                    vec!["Looting".to_string(), "Rewarding".to_string()]
                );
                assert_eq!(d.loot_bonus, 75);
            }
            other => panic!("Expected Dungeon entry, got {:?}", other),
        }
    }

    #[test]
    fn push_dungeon_sets_map_seed_and_starts_run_timer_unfrozen() {
        let mut panel = LiveFeedPanel::new();
        panel.push_dungeon(7, "Snake Pit", &[], None);
        match panel.entries.front().unwrap() {
            FeedEntry::Dungeon(d) => {
                assert_eq!(d.map_seed, 7);
                assert!(d.frozen_elapsed_ms.is_none(), "run timer starts live");
            }
            other => panic!("Expected Dungeon entry, got {:?}", other),
        }
    }

    #[test]
    fn apply_dungeon_freeze_settles_matching_seed_only() {
        let mut panel = LiveFeedPanel::new();
        panel.push_dungeon(11, "Snake Pit", &[], None);
        // A non-matching seed leaves the row ticking.
        panel.apply_dungeon_freeze(999, 4_000);
        match panel.entries.front().unwrap() {
            FeedEntry::Dungeon(d) => assert!(d.frozen_elapsed_ms.is_none()),
            other => panic!("Expected Dungeon entry, got {:?}", other),
        }
        // The matching seed settles the timer to the committed value.
        panel.apply_dungeon_freeze(11, 4_000);
        match panel.entries.front().unwrap() {
            FeedEntry::Dungeon(d) => assert_eq!(d.frozen_elapsed_ms, Some(4_000)),
            other => panic!("Expected Dungeon entry, got {:?}", other),
        }
    }

    #[test]
    fn format_run_timer_uses_mmss_then_hhmmss() {
        use std::time::Duration;
        assert_eq!(format_run_timer(Duration::from_secs(5)), "0:05");
        assert_eq!(format_run_timer(Duration::from_secs(92)), "1:32");
        assert_eq!(format_run_timer(Duration::from_secs(3_661)), "1:01:01");
    }

    #[test]
    fn dungeon_bonus_tooltip_orders_loot_dust_xp() {
        let _assets = crate::test_support::modifier_assets();
        let (mods, ov) = test_callout_inputs();
        let params = test_params(&mods, &ov);
        let entry = DungeonEntry::new(
            "Snake Pit".to_string(),
            None,
            &["REWARDING".to_string(), "WEAKBOSS_3".to_string()],
            None,
            &params,
            None,
            None,
            std::time::Instant::now(),
            false,
        );
        // Rewarding (25/25/25) + Weak Boss III (xp 15, loot 1, dust 3).
        let tooltip = LiveFeedPanel::dungeon_bonus_tooltip(&entry).expect("has bonuses");
        assert_eq!(
            tooltip,
            "Loot bonus: +26%\nDust bonus: +28%\nXP bonus: +40%"
        );
    }

    #[test]
    fn dungeon_bonus_tooltip_none_when_all_zero() {
        let (mods, ov) = test_callout_inputs();
        let params = test_params(&mods, &ov);
        let entry = DungeonEntry::new(
            "Snake Pit".to_string(),
            None,
            &["UNKNOWN_MOD".to_string()],
            None,
            &params,
            None,
            None,
            std::time::Instant::now(),
            false,
        );
        assert!(LiveFeedPanel::dungeon_bonus_tooltip(&entry).is_none());
    }

    #[test]
    fn push_dungeon_resolves_reworked_token_display_name() {
        let _assets = crate::test_support::modifier_assets();
        let mut panel = LiveFeedPanel::new();
        panel.push_dungeon(
            1,
            "Snake Pit",
            &["WEAKBOSS_3".to_string()],
            Some("C".to_string()),
        );
        match panel.entries.front().unwrap() {
            FeedEntry::Dungeon(d) => {
                assert_eq!(d.modifiers, vec!["Weak Boss III".to_string()]);
                assert_eq!(d.loot_bonus, 1);
            }
            other => panic!("Expected Dungeon entry, got {:?}", other),
        }
    }

    #[test]
    fn push_dungeon_records_modifiers_and_grade() {
        let _assets = crate::test_support::modifier_assets();
        let mut panel = LiveFeedPanel::new();
        panel.push_dungeon(
            1,
            "Spider Den",
            &["CHEF".to_string(), "SOUVENIR_1".to_string()],
            Some("S".to_string()),
        );
        assert_eq!(dungeon_count(&panel), 1);
        match panel.entries.front().unwrap() {
            FeedEntry::Dungeon(d) => {
                assert_eq!(d.dungeon_name, "Spider Den");
                assert_eq!(
                    d.modifiers,
                    vec!["Chef".to_string(), "Souvenir I (Legacy)".to_string()]
                );
                assert_eq!(d.grade, Some("S".to_string()));
                assert_eq!(d.loot_bonus, 0);
            }
            other => panic!("Expected Dungeon entry, got {:?}", other),
        }
    }

    #[test]
    fn push_dungeon_populates_mod_chips_and_outline() {
        let _assets = crate::test_support::modifier_assets();
        let mut panel = LiveFeedPanel::new();
        // Dimitus drives a golden outline; chips carry per-mod type + danger.
        panel.push_dungeon(
            1,
            "Snake Pit",
            &["DIMITUS".to_string(), "EXPOSED_1".to_string()],
            None,
        );
        match panel.entries.front().unwrap() {
            FeedEntry::Dungeon(d) => {
                assert_eq!(d.outline, OutlineKind::Golden);
                assert_eq!(d.mods.len(), 2);
                let dimitus = &d.mods[0];
                assert!(dimitus.is_dimitus);
                assert_eq!(dimitus.mod_type, ModType::Unique);
                assert_eq!(dimitus.danger, ModDanger::Golden);
                let exposed = &d.mods[1];
                assert!(!exposed.is_dimitus);
                assert_eq!(exposed.danger, ModDanger::Red);
            }
            other => panic!("Expected Dungeon entry, got {:?}", other),
        }
    }

    #[test]
    fn push_dungeon_dedups_consecutive_identical() {
        let mut panel = LiveFeedPanel::new();
        panel.push_dungeon(1, "Spider Den", &["CHEF".to_string()], None);
        panel.push_dungeon(1, "Spider Den", &["CHEF".to_string()], None);
        assert_eq!(dungeon_count(&panel), 1);

        // A different dungeon (or different modifiers) is kept.
        panel.push_dungeon(1, "Spider Den", &["GENEROUS".to_string()], None);
        assert_eq!(dungeon_count(&panel), 2);
    }

    #[test]
    fn push_dungeon_dedup_survives_intervening_entries() {
        let mut panel = LiveFeedPanel::new();
        panel.push_dungeon(1, "Spider Den", &["CHEF".to_string()], None);
        // A non-dungeon entry arrives at the front before a duplicate MapInfo.
        panel.push_kick_alert("Cheater".to_string());
        panel.push_dungeon(1, "Spider Den", &["CHEF".to_string()], None);
        // Still deduped because we track a signature, not just the front entry.
        assert_eq!(dungeon_count(&panel), 1);
    }

    #[test]
    fn clearing_signature_allows_reentry_entry() {
        let mut panel = LiveFeedPanel::new();
        panel.push_dungeon(1, "Spider Den", &["CHEF".to_string()], None);
        // Player leaves the dungeon (enters a hub/realm).
        panel.clear_dungeon_signature();
        // Re-entering the same dungeon produces a fresh entry.
        panel.push_dungeon(1, "Spider Den", &["CHEF".to_string()], None);
        assert_eq!(dungeon_count(&panel), 2);
    }

    #[test]
    fn entering_new_dungeon_expires_prior_active_entry() {
        let mut panel = LiveFeedPanel::new();
        panel.push_dungeon(1, "Spider Den", &["CHEF".to_string()], None);
        panel.push_dungeon(2, "Snake Pit", &["CHEF".to_string()], None);
        let now = std::time::Instant::now();
        let dungeons: Vec<&DungeonEntry> = panel
            .entries
            .iter()
            .filter_map(|e| match e {
                FeedEntry::Dungeon(d) => Some(d),
                _ => None,
            })
            .collect();
        assert_eq!(dungeons.len(), 2);
        // Newest at front: Snake Pit still joinable, Spider Den now expired.
        assert!(!dungeons[0].remaining(now).is_zero());
        assert!(dungeons[1].remaining(now).is_zero());
    }

    #[test]
    fn deactivate_active_dungeons_expires_entries() {
        let mut panel = LiveFeedPanel::new();
        panel.push_dungeon(1, "Spider Den", &["CHEF".to_string()], None);
        panel.deactivate_active_dungeons();
        let now = std::time::Instant::now();
        match panel.entries.front().unwrap() {
            FeedEntry::Dungeon(d) => assert!(d.remaining(now).is_zero()),
            other => panic!("Expected Dungeon entry, got {:?}", other),
        }
    }

    #[test]
    fn dungeon_at_full_score_before_realm_close_is_deactivated() {
        // A dungeon entered while the realm reads 100% but before the
        // RealmClosed packet arrives can't be joined, so it starts expired.
        let mut panel = LiveFeedPanel::new();
        panel.update_location("Realm of the Mad God", "NexusPortal.Medusa", 1000, 1000);
        assert_eq!(panel.get_score_percent(), Some(100));
        assert!(!panel.realm_closed);
        panel.push_dungeon(1, "Snake Pit", &[], None);
        let now = std::time::Instant::now();
        match panel.entries.front().unwrap() {
            FeedEntry::Dungeon(d) => assert!(d.remaining(now).is_zero()),
            other => panic!("Expected Dungeon entry, got {:?}", other),
        }
    }

    #[test]
    fn key_dungeon_after_realm_close_stays_callable() {
        // Once the realm has closed and the Oryx endgame begins, a
        // dungeon opened with a key inside the castle areas must stay callable
        // even though the stale realm score still reads 100% (CLOSED).
        let mut panel = LiveFeedPanel::new();
        panel.update_location("Realm of the Mad God", "NexusPortal.Medusa", 500, 1000);
        panel.force_realm_closed();
        assert_eq!(
            panel.get_score_percent(),
            Some(100),
            "realm should read CLOSED"
        );
        assert!(panel.realm_closed);
        panel.push_dungeon(1, "Snake Pit", &[], None);
        let now = std::time::Instant::now();
        match panel.entries.front().unwrap() {
            FeedEntry::Dungeon(d) => {
                assert!(
                    !d.remaining(now).is_zero(),
                    "key dungeon after close must be callable"
                )
            }
            other => panic!("Expected Dungeon entry, got {:?}", other),
        }
    }

    #[test]
    fn court_dungeon_after_realm_close_stays_callable() {
        // The Court of Oryx dungeons (Encore, Reef, Thicket,
        // Shaitan, High Tech Terror) are joinable and must stay callable after
        // the realm closes, even though they aren't opened with a key.
        let mut panel = LiveFeedPanel::new();
        panel.update_location("Realm of the Mad God", "NexusPortal.Medusa", 500, 1000);
        panel.force_realm_closed();
        let now = std::time::Instant::now();
        for (i, name) in ["Encore", "Reef", "Thicket", "Shaitan", "High Tech Terror"]
            .iter()
            .enumerate()
        {
            panel.push_dungeon(i as i32, name, &[], None);
            match panel.entries.front().unwrap() {
                FeedEntry::Dungeon(d) => {
                    assert!(
                        !d.remaining(now).is_zero(),
                        "{name} must be callable after close"
                    )
                }
                other => panic!("Expected Dungeon entry, got {:?}", other),
            }
        }
    }

    #[test]
    fn entering_new_realm_clears_realm_closed_latch() {
        // Diving into a fresh realm after a close re-arms the pre-close
        // deactivation behavior.
        let mut panel = LiveFeedPanel::new();
        panel.update_location("Realm of the Mad God", "NexusPortal.Medusa", 500, 1000);
        panel.force_realm_closed();
        assert!(panel.realm_closed);
        panel.update_location("Realm of the Mad God", "NexusPortal.Svalinn", 200, 1000);
        assert!(!panel.realm_closed);
    }

    #[test]
    fn nexus_clears_realm_closed_latch() {
        let mut panel = LiveFeedPanel::new();
        panel.update_location("Realm of the Mad God", "NexusPortal.Medusa", 500, 1000);
        panel.force_realm_closed();
        assert!(panel.realm_closed);
        panel.update_location("Nexus", "Nexus", -1, -1);
        assert!(!panel.realm_closed);
    }

    #[test]
    fn push_dungeon_anchors_countdown_to_portal_spawn() {
        let mut panel = LiveFeedPanel::new();
        panel.pending_portal_spawn =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(25));
        panel.push_dungeon(1, "Spider Den", &["CHEF".to_string()], None);
        let now = std::time::Instant::now();
        match panel.entries.front().unwrap() {
            FeedEntry::Dungeon(d) => {
                let rem = d.remaining(now);
                assert!(!d.estimated);
                assert!(rem <= std::time::Duration::from_secs(5));
                assert!(rem > std::time::Duration::from_secs(3));
            }
            other => panic!("Expected Dungeon entry, got {:?}", other),
        }
        // The spawn anchor is consumed by the push.
        assert!(panel.pending_portal_spawn.is_none());
    }

    #[test]
    fn push_dungeon_ignores_stale_portal_anchor() {
        let mut panel = LiveFeedPanel::new();
        // A spawn older than the 30s join window can't be the portal we entered.
        panel.pending_portal_spawn =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(40));
        panel.push_dungeon(1, "Spider Den", &["CHEF".to_string()], None);
        let now = std::time::Instant::now();
        match panel.entries.front().unwrap() {
            FeedEntry::Dungeon(d) => {
                // Falls back to entry time: an estimated, near-full 25s window.
                assert!(d.estimated);
                let rem = d.remaining(now);
                assert!(rem > std::time::Duration::from_secs(23));
                assert!(rem <= std::time::Duration::from_secs(25));
            }
            other => panic!("Expected Dungeon entry, got {:?}", other),
        }
    }

    #[test]
    fn different_fp_produces_new_entry_for_same_dungeon() {
        // Entering a new instance of the same dungeon (different fp/
        // map seed) must produce a fresh feed entry even without visiting a hub.
        let mut panel = LiveFeedPanel::new();
        panel.push_dungeon(100, "Spider Den", &[], None);
        panel.push_dungeon(200, "Spider Den", &[], None);
        assert_eq!(dungeon_count(&panel), 2);
    }

    #[test]
    fn push_dungeon_normalizes_localization_keys() {
        let mut panel = LiveFeedPanel::new();
        panel.push_dungeon(1, "{s.wine_cellar}", &[], None);
        match panel.entries.front().unwrap() {
            FeedEntry::Dungeon(d) => assert_eq!(d.dungeon_name, "Wine Cellar"),
            other => panic!("Expected Dungeon entry, got {:?}", other),
        }
    }

    #[test]
    fn push_dungeon_stores_callout_for_known_dungeon() {
        let _assets = crate::test_support::modifier_assets();
        let mut panel = LiveFeedPanel::new();
        // Snake Pit -> "snake"; Looting (+50 loot) -> "50 lb" (percent off by default).
        panel.push_dungeon(1, "Snake Pit", &["LOOTING".to_string()], None);
        match panel.entries.front().unwrap() {
            FeedEntry::Dungeon(d) => {
                assert_eq!(d.callout.as_deref(), Some("snake 50 lb"));
                assert!(!d.is_dimitus);
            }
            other => panic!("Expected Dungeon entry, got {:?}", other),
        }
    }

    #[test]
    fn push_dungeon_dimitus_suppressed_by_default_and_can_be_enabled() {
        // Default: Dimitus reward-mod entry is disabled, so a Dimitus dungeon
        // produces no callout (but is still flagged).
        let mut panel = LiveFeedPanel::new();
        panel.push_dungeon(1, "Snake Pit", &["DIMITUS".to_string()], None);
        match panel.entries.front().unwrap() {
            FeedEntry::Dungeon(d) => {
                assert_eq!(d.callout, None);
                assert!(d.is_dimitus);
            }
            other => panic!("Expected Dungeon entry, got {:?}", other),
        }

        // When the Dimitus entry is enabled, the dungeon gets a callout with a
        // trailing `dimitus` tag.
        let mut mods = realmhound_core::settings::default_reward_mods();
        if let Some(entry) = mods.iter_mut().find(|e| e.id == "DIMITUS") {
            entry.enabled = true;
        }
        let mut panel = LiveFeedPanel::new();
        panel.apply_live_feed_settings(&realmhound_core::settings::LiveFeedSettings {
            reward_mods: mods,
            ..Default::default()
        });
        panel.push_dungeon(1, "Snake Pit", &["DIMITUS".to_string()], None);
        match panel.entries.front().unwrap() {
            FeedEntry::Dungeon(d) => {
                let callout = d.callout.as_deref().expect("enabled dimitus has callout");
                assert!(callout.starts_with("snake "), "{callout}");
                assert!(callout.ends_with(" dimitus"), "{callout}");
                assert!(d.is_dimitus);
            }
            other => panic!("Expected Dungeon entry, got {:?}", other),
        }
    }

    #[test]
    fn push_dungeon_unknown_nickname_falls_back_to_full_name() {
        let _assets = crate::test_support::modifier_assets();
        let mut panel = LiveFeedPanel::new();
        panel.push_dungeon(1, "Some Unknown Place", &["LOOTING".to_string()], None);
        match panel.entries.front().unwrap() {
            FeedEntry::Dungeon(d) => {
                assert_eq!(d.callout.as_deref(), Some("some unknown place 50 lb"))
            }
            other => panic!("Expected Dungeon entry, got {:?}", other),
        }
    }

    #[test]
    fn push_dungeon_non_callable_stays_in_feed_without_callout() {
        // The Void is non-callable: it appears in the feed but has no callout,
        // even with the Dimitus modifier present.
        let mut panel = LiveFeedPanel::new();
        panel.push_dungeon(1, "The Void", &["DIMITUS".to_string()], None);
        match panel.entries.front().unwrap() {
            FeedEntry::Dungeon(d) => {
                assert_eq!(d.dungeon_name, "The Void");
                assert_eq!(d.callout, None);
                assert!(d.is_dimitus);
                // Non-callable dungeons start expired (no join timer).
                assert!(d.remaining(std::time::Instant::now()).is_zero());
            }
            other => panic!("Expected Dungeon entry, got {:?}", other),
        }
    }

    #[test]
    fn settings_change_recomputes_existing_callouts() {
        let _assets = crate::test_support::modifier_assets();
        use realmhound_core::settings::LiveFeedSettings;
        let mut panel = LiveFeedPanel::new();
        panel.push_dungeon(1, "Snake Pit", &["LOOTING".to_string()], None);

        // Initially: percent sign is omitted (default off).
        match panel.entries.front().unwrap() {
            FeedEntry::Dungeon(d) => assert_eq!(d.callout.as_deref(), Some("snake 50 lb")),
            other => panic!("Expected Dungeon entry, got {:?}", other),
        }

        // Toggle percent sign on -> existing entry updates retroactively.
        panel.apply_live_feed_settings(&LiveFeedSettings {
            callout_percent: true,
            ..Default::default()
        });
        match panel.entries.front().unwrap() {
            FeedEntry::Dungeon(d) => assert_eq!(d.callout.as_deref(), Some("snake 50% lb")),
            other => panic!("Expected Dungeon entry, got {:?}", other),
        }

        // Toggle back off -> percent disappears.
        panel.apply_live_feed_settings(&LiveFeedSettings {
            callout_percent: false,
            ..Default::default()
        });
        match panel.entries.front().unwrap() {
            FeedEntry::Dungeon(d) => assert_eq!(d.callout.as_deref(), Some("snake 50 lb")),
            other => panic!("Expected Dungeon entry, got {:?}", other),
        }
    }

    #[test]
    fn push_dungeon_resolves_nickname_from_localization_key() {
        // Unresolved localization keys are title-cased by format_dungeon_name,
        // so "The..." nicknames still match.
        let mut panel = LiveFeedPanel::new();
        panel.push_dungeon(1, "{s.the_third_dimension}", &[], None);
        match panel.entries.front().unwrap() {
            FeedEntry::Dungeon(d) => {
                assert_eq!(d.dungeon_name, "The Third Dimension");
                assert_eq!(d.callout.as_deref(), Some("3D"));
            }
            other => panic!("Expected Dungeon entry, got {:?}", other),
        }
    }

    // === DM Entry Tests ===

    #[test]
    fn push_dm_creates_direct_message_entry() {
        let mut panel = LiveFeedPanel::new();
        panel.push_dm("Player1".into(), "Hello!".into());
        assert_eq!(panel.entries.len(), 1);
        match panel.entries.front().unwrap() {
            FeedEntry::DirectMessage(dm) => {
                assert_eq!(dm.sender, "Player1");
                assert_eq!(dm.text, "Hello!");
            }
            other => panic!("Expected DirectMessage entry, got {:?}", other),
        }
    }

    #[test]
    fn dm_entry_hidden_by_filter() {
        let mut panel = LiveFeedPanel::new();
        panel.push_dm("Player1".into(), "test".into());
        assert!(panel.entry_passes_filter(panel.entries.front().unwrap()));

        // Filter bar toggle hides DMs
        panel.show_dms_filter = false;
        assert!(!panel.entry_passes_filter(panel.entries.front().unwrap()));

        // Global setting also hides DMs
        panel.show_dms_filter = true;
        panel.show_dms = false;
        assert!(!panel.entry_passes_filter(panel.entries.front().unwrap()));
    }

    #[test]
    fn dm_display_enabled_mirrors_setting() {
        let mut panel = LiveFeedPanel::new();
        assert!(panel.dm_display_enabled());

        panel.show_dms = false;
        assert!(!panel.dm_display_enabled());
    }

    // === /who Roster Tests ===

    #[test]
    fn parse_who_roster_sorts_case_insensitively() {
        let names = parse_who_roster("Players online (3): charlie, Alice, bob").unwrap();
        assert_eq!(names, vec!["Alice", "bob", "charlie"]);
    }

    #[test]
    fn parse_who_roster_single_player() {
        let names = parse_who_roster("Players online (1): Solo").unwrap();
        assert_eq!(names, vec!["Solo"]);
    }

    #[test]
    fn parse_who_roster_empty_roster() {
        let names = parse_who_roster("Players online (0): ").unwrap();
        assert!(names.is_empty());
    }

    #[test]
    fn parse_who_roster_ignores_non_who_text() {
        assert!(parse_who_roster("Hello there").is_none());
        assert!(parse_who_roster("Player has left the party").is_none());
    }

    #[test]
    fn parse_who_roster_tolerates_extra_whitespace_and_dupes() {
        let names = parse_who_roster("Players online (4):  bob ,,alice, bob ,").unwrap();
        assert_eq!(names, vec!["alice", "bob"]);
    }

    #[test]
    fn push_who_roster_creates_entry_and_respects_toggle() {
        let mut panel = LiveFeedPanel::new();
        panel.push_who_roster(vec!["a".into(), "b".into()]);
        assert_eq!(panel.entries.len(), 1);
        let entry = panel.entries.front().unwrap();
        match entry {
            FeedEntry::WhoRoster(r) => assert_eq!(r.names.len(), 2),
            other => panic!("Expected WhoRoster entry, got {:?}", other),
        }
        assert!(panel.entry_passes_filter(entry));

        panel.show_who_roster = false;
        assert!(!panel.entry_passes_filter(panel.entries.front().unwrap()));
    }

    // === /server Command Tests ===

    #[test]
    fn parse_server_command_realm() {
        let (server, realm) = parse_server_command("EUWest NexusPortal.Meridian").unwrap();
        assert_eq!(server, "EUWest");
        assert_eq!(realm.as_deref(), Some("Meridian"));
    }

    #[test]
    fn parse_server_command_nexus_has_no_realm() {
        let (server, realm) = parse_server_command("EUWest Nexus").unwrap();
        assert_eq!(server, "EUWest");
        assert_eq!(realm, None);
    }

    #[test]
    fn parse_server_command_tolerates_surrounding_whitespace() {
        let (server, realm) = parse_server_command("  USMidWest2 NexusPortal.Djinn  ").unwrap();
        assert_eq!(server, "USMidWest2");
        assert_eq!(realm.as_deref(), Some("Djinn"));
    }

    #[test]
    fn parse_server_command_ignores_ordinary_chat() {
        assert!(parse_server_command("EUWest is popping right now").is_none());
        assert!(parse_server_command("come to NexusPortal.Meridian").is_none());
        assert!(parse_server_command("Hello there").is_none());
        assert!(parse_server_command("EUWest NexusPortal.").is_none());
        assert!(parse_server_command("EU-West Nexus").is_none());
    }

    #[test]
    fn apply_server_command_overrides_header() {
        let mut panel = LiveFeedPanel::new();
        // Entering a realm sets the name.
        panel.apply_server_command("EUWest".into(), Some("Meridian".into()));
        assert_eq!(panel.server_name.as_deref(), Some("EUWest"));
        assert_eq!(panel.realm_name.as_deref(), Some("Meridian"));

        // A hub reply clears realm state but keeps the server.
        panel.apply_server_command("EUWest".into(), None);
        assert_eq!(panel.server_name.as_deref(), Some("EUWest"));
        assert_eq!(panel.realm_name, None);
    }

    #[test]
    fn apply_server_command_different_realm_clears_stale_score() {
        let mut panel = LiveFeedPanel::new();
        panel.realm_name = Some("Medusa".into());
        panel.realm_score = Some((10, 100));
        panel.is_score_stale = true;
        // Same realm keeps the score.
        panel.apply_server_command("EUWest".into(), Some("Medusa".into()));
        assert_eq!(panel.realm_score, Some((10, 100)));
        // A different realm invalidates it.
        panel.apply_server_command("EUWest".into(), Some("Meridian".into()));
        assert_eq!(panel.realm_score, None);
        assert!(!panel.is_score_stale);
    }

    #[test]
    fn apply_server_command_preserves_dungeon_flag() {
        let mut panel = LiveFeedPanel::new();
        panel.realm_name = Some("Lumen".into());
        panel.currently_in_dungeon = true;
        // A `/server` reply while inside a dungeon reports the parent realm and
        // must not clear the dungeon flag (keeps portal icon + Oryx-lag gate).
        panel.apply_server_command("EUWest".into(), Some("Lumen".into()));
        assert!(panel.currently_in_dungeon);
        // A hub reply ("<Server> Nexus") is ambiguous: it also arrives inside a
        // dungeon opened from the Nexus, so it must NOT clear the dungeon flag
        // either. MapChanged clears it on the real exit to the Nexus.
        panel.apply_server_command("EUWest".into(), None);
        assert!(panel.currently_in_dungeon);
        assert!(panel.realm_name.is_none());
    }

    fn text_packet(
        name: &str,
        object_id: i32,
        recipient: &str,
        text: &str,
    ) -> realmhound_core::protocol::TextPacket {
        realmhound_core::protocol::TextPacket {
            name: name.to_string(),
            object_id,
            num_stars: 0,
            bubble_time: 0,
            recipient: recipient.to_string(),
            text: text.to_string(),
            clean_text: text.to_string(),
            is_supporter: false,
            star_background: 0,
        }
    }

    #[test]
    fn server_command_packet_accepts_system_message() {
        // System announcement: negative object id, no recipient.
        let p = text_packet("", -1, "", "EUWest NexusPortal.Meridian");
        let (server, realm) = parse_server_command_packet(&p).unwrap();
        assert_eq!(server, "EUWest");
        assert_eq!(realm.as_deref(), Some("Meridian"));
    }

    #[test]
    fn server_command_packet_rejects_player_chat_spoof() {
        // Public chat from a player (object id >= 0) cannot spoof the header.
        let public = text_packet("Spoofer", 42, "", "EUWest Nexus");
        assert!(parse_server_command_packet(&public).is_none());
        // Whisper/party/guild directed messages carry a recipient.
        let whisper = text_packet("", -1, "Me", "EUWest Nexus");
        assert!(parse_server_command_packet(&whisper).is_none());
    }
}

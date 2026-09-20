//! Quest panel - displays daily quests from the Tinkerer.
//!
//! Shows quest requirements (items needed) and rewards (items received)
//! when the player enters the Daily Quest Room.

use chrono::{DateTime, Utc};
use eframe::egui::{self, Color32, RichText, ScrollArea};
use realmhound_core::{
    assets::get_asset_manager,
    protocol::{data::QuestData, QuestFetchResponsePacket},
    vault::AccountData,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::SystemTime;

use crate::memo::Memo;
use crate::panels::{AppAction, Panel, PanelContext};
use crate::rendering::SpriteRenderer;
use crate::shadcn_ui::Shadcn;
use crate::ui_colors::{REGULAR_COLOR, SEASONAL_COLOR};
use crate::ui_ext::HoverTooltipExt;

/// Categorized quest sections, derived from a quest's live `category` int plus
/// name-based refinements. Each section has a stable `id()` (persisted in
/// settings), a display title, and a header color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum QuestSection {
    DailyChests = 0,
    DailyMisc = 1,
    PermanentRewards = 2,
    WeeklySchematic = 3,
    RepeatableExchange = 4,
    SkinExchange = 5,
    Event = 6,
    Other = 7,
    /// Quests the player explicitly hid. Always rendered last, no enable toggle.
    Hidden = 8,
}

impl QuestSection {
    /// Every section, in display order.
    const ALL: [QuestSection; 9] = [
        QuestSection::DailyChests,
        QuestSection::DailyMisc,
        QuestSection::PermanentRewards,
        QuestSection::WeeklySchematic,
        QuestSection::RepeatableExchange,
        QuestSection::SkinExchange,
        QuestSection::Event,
        QuestSection::Other,
        QuestSection::Hidden,
    ];

    /// Stable id used in persisted settings.
    fn id(self) -> u8 {
        self as u8
    }

    /// Base section for a live quest `category` int, before name refinements.
    fn base_for_category(category: i32) -> QuestSection {
        match category {
            0 => QuestSection::DailyChests,
            999..=1001 | 6999..=7000 | 7162..=7163 => QuestSection::PermanentRewards,
            7010..=7012 | 11000 => QuestSection::RepeatableExchange,
            9080 | 9095..=9097 => QuestSection::SkinExchange,
            _ => QuestSection::Other,
        }
    }

    /// Resolve a quest's section from its reward structure, category, and name.
    ///
    /// A quest that awards a Quest Chest is always a Daily Quest Chest, whatever
    /// its server `category` (seasonal/event chests use unpredictable category
    /// ids, e.g. The Cursed Heart's 100). Only non-chest quests fall back to the
    /// category buckets, where a few named quests are split out: daily misc
    /// chores, weekly schematic exchanges, and repeatable ore refinements.
    fn for_quest(quest: &QuestData) -> QuestSection {
        if awards_quest_chest(quest) {
            return QuestSection::DailyChests;
        }
        let name = quest.name.to_lowercase();
        match Self::base_for_category(quest.category) {
            QuestSection::DailyChests => {
                if name.contains("eggs")
                    || name.contains("scout")
                    || name.contains("potion fusion")
                    || name.contains("laundry")
                {
                    QuestSection::DailyMisc
                } else {
                    QuestSection::DailyChests
                }
            }
            QuestSection::PermanentRewards => {
                if name.contains("craftsmanship") {
                    QuestSection::WeeklySchematic
                } else if name.contains("ore refin") {
                    QuestSection::RepeatableExchange
                } else {
                    QuestSection::PermanentRewards
                }
            }
            other => other,
        }
    }

    fn title(self) -> &'static str {
        match self {
            QuestSection::DailyChests => "Daily Quest Chests",
            QuestSection::DailyMisc => "Daily Misc",
            QuestSection::PermanentRewards => "Permanent Account Rewards",
            QuestSection::WeeklySchematic => "Weekly Schematic Exchange",
            QuestSection::RepeatableExchange => "Exchange",
            QuestSection::SkinExchange => "Skin Exchange",
            QuestSection::Event => "Event Tinkerer",
            QuestSection::Other => "Other",
            QuestSection::Hidden => "Hidden by player",
        }
    }

    fn color(self) -> Color32 {
        match self {
            QuestSection::SkinExchange => Color32::from_rgb(0x8e, 0xf3, 0xff),
            QuestSection::RepeatableExchange => Color32::from_rgb(235, 205, 70),
            QuestSection::WeeklySchematic => Color32::from_rgb(200, 160, 255),
            QuestSection::Other => Color32::from_gray(150),
            QuestSection::Hidden => Color32::from_gray(140),
            _ => Color32::WHITE,
        }
    }
}

/// True when a quest can be completed only once per account: any permanent
/// account-reward quest (non-repeatable), plus the one-per-account Pet Trial.
fn is_unique_quest(quest: &QuestData) -> bool {
    let name = quest.name.to_lowercase();
    (QuestSection::for_quest(quest) == QuestSection::PermanentRewards && !quest.repeatable)
        || name.contains("pet trial")
}

/// Longest reset countdown we surface for an explicit `expiration`; anything
/// further out (or unparseable / empty) is treated as "no timer" and hidden.
/// Quests without a real reset carry sentinel expirations years out (2030,
/// 2120), so this filters those while keeping weekly/monthly resets visible.
const MAX_TIMER_DAYS: i64 = 366;

/// Parse a quest's `expiration` into a UTC instant. The live server sends a
/// Unix epoch-seconds value (e.g. "1787644800.0"); we also accept RFC3339 for
/// robustness. Empty / zero / unparseable values yield `None`.
fn parse_expiration(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(secs) = s.parse::<f64>() {
        if secs > 0.0 {
            return DateTime::<Utc>::from_timestamp(secs as i64, 0);
        }
        return None;
    }
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// A quest's in-range explicit expiration (from the packet's `expiration`
/// field), or `None` when it has no usable one (empty, unparseable, or further
/// out than `MAX_TIMER_DAYS`). In practice the server currently leaves this
/// empty, so only the daily categories (which synthesize a reset) show a timer.
fn quest_timer(q: &QuestData, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    parse_expiration(&q.expiration)
        .filter(|&exp| exp - now <= chrono::Duration::days(MAX_TIMER_DAYS))
}

/// The daily quest categories that reset together at 00:00 UTC each day.
fn is_daily_reset_section(s: QuestSection) -> bool {
    matches!(s, QuestSection::DailyChests | QuestSection::DailyMisc)
}

/// The first 00:00 UTC strictly after `from`.
fn next_utc_midnight(from: DateTime<Utc>) -> DateTime<Utc> {
    let next_day = from
        .date_naive()
        .succ_opt()
        .unwrap_or_else(|| from.date_naive());
    let ndt = next_day.and_hms_opt(0, 0, 0).expect("valid midnight");
    DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc)
}

/// When the currently-held daily quests reset: the first 00:00 UTC after they
/// were fetched. The client only refreshes them on restart, so once this passes
/// `now` the timer is expired (goes red) until the game is relaunched.
fn daily_reset(fetched_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> DateTime<Utc> {
    next_utc_midnight(fetched_at.unwrap_or(now))
}

/// Tooltip shown for daily quests that are stale after a post-reset game
/// restart: the cached quests expired at the daily reset and the client has not
/// re-fetched them yet (the Tinkerer Cave has not been re-entered this run).
pub const STALE_QUEST_TOOLTIP: &str =
    "This quest has expired. Enter Tinkerer Cave portal in game to see new quests";

/// Header/tab variant of `STALE_QUEST_TOOLTIP`, phrased for the whole daily set
/// rather than a single card.
pub const STALE_QUESTS_TAB_TOOLTIP: &str =
    "Daily quests have expired. Enter Tinkerer Cave portal in game to see new quests";

/// True when the currently-held daily quests are stale AND could not have been
/// refreshed by the running game: the game was (re)started at or after the daily
/// reset boundary, yet we still hold the pre-reset quest set. In that case the
/// timer sits at 0:00 but restarting would not help -- only re-entering the
/// Tinkerer Cave re-fetches quests. An unknown or non-positive game start time
/// fails safe to `false` (keep the normal "restart to refresh" behavior).
fn daily_stale_unrefetched(
    fetched_at: Option<DateTime<Utc>>,
    game_start_unix: Option<i64>,
    now: DateTime<Utc>,
) -> bool {
    let boundary = daily_reset(fetched_at, now);
    now >= boundary && game_start_unix.is_some_and(|s| s > 0 && s >= boundary.timestamp())
}

/// True when `incoming` introduces a quest id not already held in `current` --
/// i.e. a genuine daily re-roll rather than a repeated fetch of the same set (or
/// a completion-shrunk subset). Used to decide whether to re-anchor the daily
/// reset countdown (`cached_at`).
fn introduces_new_quest(current: &[QuestData], incoming: &[QuestData]) -> bool {
    let old: std::collections::HashSet<&str> = current.iter().map(|q| q.id.as_str()).collect();
    incoming.iter().any(|q| !old.contains(q.id.as_str()))
}

/// A quest's effective reset instant: a synthesized daily 00:00 UTC reset for
/// daily quests (keyed off `fetched_at`), otherwise its explicit expiration.
/// Uses the quest's natural section so hidden daily quests still show a card
/// timer.
fn effective_quest_timer(
    q: &QuestData,
    fetched_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    if is_daily_reset_section(QuestSection::for_quest(q)) {
        Some(daily_reset(fetched_at, now))
    } else {
        quest_timer(q, now)
    }
}

/// Format the remaining time as a compact "Dd Hh" (>= 24h) or "Hh Mm" (< 24h)
/// label, matching the Missions Status column. Clamped at zero.
fn format_remaining_short(exp: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (exp - now).num_seconds().max(0);
    let d = secs / 86_400;
    let h = (secs % 86_400) / 3_600;
    let m = (secs % 3_600) / 60;
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        "<1m".to_string()
    }
}

/// A quest's Status-column timer: a periodic reset (circle-arrow glyph) or a
/// one-time expiry (hourglass glyph). `text` is the short remaining time.
#[derive(Clone)]
enum QuestStatusTimer {
    Resets { text: String, expired: bool },
    Ends { text: String, expired: bool },
}

/// Space reserved on the right of a card for the Hide button, the (always
/// reserved) compass slot, and a gap -- the right boundary the card grid must
/// end before, so cards with and without a compass still align.
const TIMER_PILL_RIGHT_RESERVE: f32 = 77.0;

/// Whether a chest quest can be turned in right now: every required mark is
/// fully collected on the Seasonal side, or fully on the Regular side.
fn chest_completable(quest: &QuestData, item_counts: &HashMap<i32, (u32, u32)>) -> bool {
    let mut need: HashMap<i32, u32> = HashMap::new();
    for &id in &quest.requirements {
        *need.entry(id).or_insert(0) += 1;
    }
    if need.is_empty() {
        return false;
    }
    let seasonal_ok = need
        .iter()
        .all(|(id, c)| item_counts.get(id).is_some_and(|&(s, _)| s >= *c));
    let regular_ok = need
        .iter()
        .all(|(id, c)| item_counts.get(id).is_some_and(|&(_, r)| r >= *c));
    seasonal_ok || regular_ok
}

/// True when a resolved item name denotes a Quest Chest reward. Shared by the
/// Daily Quest Chests classifier and the chest-tier icon resolver.
fn name_is_quest_chest(name: &str) -> bool {
    name.to_ascii_lowercase().contains("quest chest")
}

/// True when any of the quest's reward items is a Quest Chest, resolved from the
/// reward item name in the asset catalog. This is the structural signal for the
/// Daily Quest Chests bucket, independent of the server `category` int.
fn awards_quest_chest(quest: &QuestData) -> bool {
    let am = get_asset_manager();
    quest
        .rewards
        .iter()
        .any(|&id| name_is_quest_chest(&am.object_name(id).unwrap_or_default()))
}

/// Chest-tier priority (0 = highest, Epic) and the chest reward's sprite id for
/// a quest, resolved from the reward item name. `None` when no reward looks like
/// a Quest Chest item.
fn chest_tier_icon(quest: &QuestData) -> Option<(u8, i32)> {
    let am = get_asset_manager();
    let mut best: Option<(u8, i32)> = None;
    for &id in &quest.rewards {
        let name = am.object_name(id).unwrap_or_default().to_ascii_lowercase();
        if !name_is_quest_chest(&name) {
            continue;
        }
        let tier = if name.contains("epic") {
            0
        } else if name.contains("mighty") {
            1
        } else if name.contains("standard") {
            2
        } else if name.contains("beginner") {
            3
        } else {
            2
        };
        if best.is_none_or(|(bt, _)| tier < bt) {
            best = Some((tier, id));
        }
    }
    best
}

/// Reward sprite of the highest-priority Daily Quest Chest that can be claimed
/// now (Epic > Mighty > Standard > Beginner), or `None` when none are ready.
/// Only currently-shown, non-hidden chest quests are considered.
fn claimable_chest_icon(
    quests: &[QuestData],
    hidden: &HashSet<String>,
    item_counts: &HashMap<i32, (u32, u32)>,
) -> Option<i32> {
    let mut best: Option<(u8, i32)> = None;
    for q in quests {
        if !q.should_display() || hidden.contains(&q.id) {
            continue;
        }
        if QuestSection::for_quest(q) != QuestSection::DailyChests {
            continue;
        }
        if !chest_completable(q, item_counts) {
            continue;
        }
        if let Some((tier, icon)) = chest_tier_icon(q) {
            if best.is_none_or(|(bt, _)| tier < bt) {
                best = Some((tier, icon));
            }
        }
    }
    best.map(|(_, icon)| icon)
}

/// Cached quest data structure for serialization.
#[derive(Serialize, Deserialize)]
struct CachedQuests {
    quests: Vec<QuestData>,
    #[serde(with = "system_time_serde")]
    cached_at: SystemTime,
}

/// Serde helper for SystemTime.
mod system_time_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    pub fn serialize<S>(time: &SystemTime, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
        duration.as_secs().serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<SystemTime, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secs = u64::deserialize(deserializer)?;
        Ok(UNIX_EPOCH + Duration::from_secs(secs))
    }
}

/// Quest panel state.
pub struct QuestPanel {
    /// Current quests (from QuestFetchResponse packet)
    quests: Vec<QuestData>,
    /// Whether we've received any quest data
    has_data: bool,
    /// Hide the quest-name column to save horizontal space.
    hide_names: bool,
    /// Whether the data was loaded from cache
    from_cache: bool,
    /// Experimental: shrink icons/fonts by half to fit more on small screens
    compact_view: bool,
    /// When the data was last updated
    cached_at: Option<SystemTime>,
    /// Id of the quest the player most recently tried to redeem, tracked so
    /// the subsequent `QuestRedeemResult` can mark it completed immediately
    /// instead of waiting for a fresh QuestFetchResponse.
    pending_redeem_quest_id: Option<String>,
    /// Bumped on any `quests` change; part of the item-count memo key.
    quests_revision: u64,
    /// Owned-item counts per required item, keyed on `(account_epoch, quests_revision)`.
    item_counts_memo: Memo<(u64, u64), HashMap<i32, (u32, u32)>>,
    /// Section ids the user has collapsed (empty = all expanded).
    collapsed: HashSet<u8>,
    /// Section ids toggled off (hidden entirely).
    disabled: HashSet<u8>,
    /// Quest ids the player hid (moved to the "Hidden by player" bucket).
    hidden: HashSet<String>,
    /// Set when persisted state changed this frame (triggers a settings save).
    dirty: bool,
    /// Live Feed Taskbar tracking settings (mirrors `LiveFeedSettings.taskbar`).
    /// Drives the per-card compass toggle and its persistence.
    taskbar: realmhound_core::settings::TaskbarSettings,
    /// Explicit quest data document path (cached packet definitions + progress).
    /// Injected so the panel resolves no path itself; empty disables persistence.
    quest_cache_path: std::path::PathBuf,
}

impl QuestPanel {
    /// Create a new quest panel bound to an explicit quest data path.
    pub fn new(quest_cache_path: std::path::PathBuf) -> Self {
        // Try to load cached quests
        let (quests, has_data, from_cache, cached_at) = Self::load_cached_quests(&quest_cache_path)
            .map(|c| (c.quests, true, true, Some(c.cached_at)))
            .unwrap_or_else(|| (Vec::new(), false, false, None));

        Self {
            quests,
            has_data,
            hide_names: false,
            from_cache,
            compact_view: false,
            cached_at,
            pending_redeem_quest_id: None,
            quests_revision: 0,
            item_counts_memo: Memo::new(),
            collapsed: HashSet::new(),
            disabled: HashSet::new(),
            hidden: HashSet::new(),
            dirty: false,
            taskbar: realmhound_core::settings::TaskbarSettings::default(),
            quest_cache_path,
        }
    }

    /// Apply persisted settings after construction.
    pub fn apply_settings(&mut self, s: &realmhound_core::settings::QuestsSettings) {
        self.collapsed = s.collapsed.iter().copied().collect();
        self.disabled = s.disabled.iter().copied().collect();
        self.hidden = s.hidden.iter().cloned().collect();
        self.hide_names = s.hide_names;
    }

    /// Apply the Live Feed Taskbar settings. Call on startup and whenever the
    /// Taskbar settings change so the compass toggles stay in sync.
    pub fn apply_taskbar_settings(&mut self, t: &realmhound_core::settings::TaskbarSettings) {
        self.taskbar = t.clone();
    }

    /// Monotonic revision bumped whenever the quest list or its display state
    /// (including hidden quests) changes. Used as a Taskbar-rebuild cache key so
    /// quest pills are not reprojected every frame.
    pub fn quests_revision(&self) -> u64 {
        self.quests_revision
    }

    /// Build the Live Feed Taskbar tasks for the tracked Daily Quest Chests.
    /// Returns an empty list when the Taskbar is disabled. Mark progress follows
    /// the "Prioritize seasonal mark progress" setting.
    pub fn taskbar_quest_tasks(
        &self,
        account_data: &AccountData,
    ) -> Vec<crate::panels::taskbar::QuestTask> {
        if !self.taskbar.enabled {
            return Vec::new();
        }
        let counts = Self::build_item_counts_cache(&self.quests, account_data);
        self.quests
            .iter()
            .filter(|q| q.should_display())
            .filter(|q| !self.hidden.contains(&q.id))
            .filter(|q| QuestSection::for_quest(q) == QuestSection::DailyChests)
            .filter(|q| self.taskbar.quest_tracked(&q.id))
            .filter_map(|q| {
                crate::panels::taskbar::build_quest_task(
                    q,
                    |id| counts.get(&id).copied().unwrap_or((0, 0)),
                    self.taskbar.prioritize_seasonal_mark_progress,
                )
            })
            .filter(|t| t.remaining > 0)
            .collect()
    }

    /// Snapshot the persisted settings for saving.
    fn to_settings(&self) -> realmhound_core::settings::QuestsSettings {
        realmhound_core::settings::QuestsSettings {
            collapsed: self.collapsed.iter().copied().collect(),
            disabled: self.disabled.iter().copied().collect(),
            hidden: self.hidden.iter().cloned().collect(),
            hide_names: self.hide_names,
        }
    }

    /// Load cached quest data (definitions + progress) from the injected path.
    fn load_cached_quests(path: &std::path::Path) -> Option<CachedQuests> {
        if path.as_os_str().is_empty() {
            return None;
        }
        match realmhound_core::storage::load_json_at::<CachedQuests>(path) {
            Ok(realmhound_core::storage::LoadOutcome::Primary(cached))
            | Ok(realmhound_core::storage::LoadOutcome::RecoveredBackup(cached)) => {
                tracing::info!("[QUEST_PANEL] Loaded cached quests");
                Some(cached)
            }
            Ok(realmhound_core::storage::LoadOutcome::Missing) => None,
            Err(e) => {
                tracing::debug!("[QUEST_PANEL] Failed to read quest cache: {}", e);
                None
            }
        }
    }

    /// Save quest data (definitions + progress) atomically with a bounded backup.
    fn save_cache(&self) {
        if self.quest_cache_path.as_os_str().is_empty() {
            return;
        }
        let cached = CachedQuests {
            quests: self.quests.clone(),
            cached_at: self.cached_at.unwrap_or_else(SystemTime::now),
        };
        match realmhound_core::storage::write_json_atomic_at(
            &self.quest_cache_path,
            &cached,
            realmhound_core::storage::BackupPolicy::Single,
        ) {
            Ok(()) => tracing::debug!("[QUEST_PANEL] Saved quests to cache"),
            Err(e) => tracing::warn!("[QUEST_PANEL] Failed to save cache: {}", e),
        }
    }

    /// Format a SystemTime as "X ago" string.
    fn format_time_ago(time: SystemTime) -> String {
        let elapsed = time.elapsed().unwrap_or_default();
        let secs = elapsed.as_secs();

        if secs < 60 {
            "just now".to_string()
        } else if secs < 3600 {
            let mins = secs / 60;
            format!("{}m ago", mins)
        } else if secs < 86400 {
            let hours = secs / 3600;
            format!("{}h ago", hours)
        } else {
            let days = secs / 86400;
            format!("{}d ago", days)
        }
    }

    /// Update quests from a QuestFetchResponse packet.
    pub fn update_quests(&mut self, packet: &QuestFetchResponsePacket) {
        // Clone and sort quests by category
        let mut quests: Vec<QuestData> = packet.quests.clone();
        quests.sort_by_key(|q| q.category);

        // Only re-anchor the daily reset countdown when this fetch actually
        // introduces a *new* quest (a genuine daily re-roll). Re-entering the
        // Tinkerer returns the same quest set until the server rolls it over, so
        // bumping `cached_at` on every packet pushed the synthesized 00:00 UTC
        // reset forward a full day -- masking the expired state and showing a
        // fresh ~24h timer for stale, pre-reset quests. Completing a quest may
        // shrink the set (a subset of held ids); that is not a new day either.
        let has_new_quest = introduces_new_quest(&self.quests, &quests);

        self.quests = quests;
        self.has_data = true;
        self.from_cache = false;
        if has_new_quest || self.cached_at.is_none() {
            self.cached_at = Some(SystemTime::now());
        }
        self.quests_revision = self.quests_revision.wrapping_add(1);

        // Save to cache
        self.save_cache();

        tracing::info!("[QUEST_PANEL] Updated with {} quests", self.quests.len());
    }

    /// Mark a quest completed locally by id, so it disappears immediately
    /// (subject to `should_display`) without waiting for a fresh
    /// QuestFetchResponse from re-entering the Tinkerer's portal.
    fn mark_quest_completed(&mut self, quest_id: &str) {
        if let Some(quest) = self.quests.iter_mut().find(|q| q.id == quest_id) {
            quest.completed = true;
            self.quests_revision = self.quests_revision.wrapping_add(1);
            self.save_cache();
        }
    }

    /// Get quests that should be displayed (filters based on settings).
    fn displayable_quests(&self) -> Vec<&QuestData> {
        self.quests.iter().filter(|q| q.should_display()).collect()
    }

    /// Count items across all storage locations for a given item type.
    /// Returns (seasonal_count, regular_count).
    ///
    /// Storage locations checked:
    /// - Seasonal: characters, vault, gifts, materials, potions, pet inventories
    /// - Regular: characters, vault, gifts, materials, potions, pet inventories, seasonal spoils
    fn count_items(item_id: i32, account_data: &AccountData) -> (u32, u32) {
        let mut seasonal: u32 = 0;
        let mut regular: u32 = 0;
        let manager = get_asset_manager();

        // Helper to count items in character slots
        let count_char_items = |items: &[realmhound_core::vault::CharacterItem]| -> u32 {
            items
                .iter()
                .filter(|i| manager.items_match(i.item_id, item_id))
                .map(|i| {
                    if i.stack_count > 1 {
                        i.stack_count as u32
                    } else {
                        1
                    }
                })
                .sum()
        };

        // Helper to count items in vault storage
        let count_vault_items = |items: &[realmhound_core::vault::LiveVaultItem]| -> u32 {
            items
                .iter()
                .filter(|i| manager.items_match(i.item_id, item_id))
                .count() as u32
        };

        // Helper to count items in pet inventory
        let count_pet_items = |items: &[realmhound_core::vault::CachedPetItem]| -> u32 {
            items
                .iter()
                .filter(|i| manager.items_match(i.item_id, item_id))
                .map(|i| {
                    if i.stack_count > 1 {
                        i.stack_count as u32
                    } else {
                        1
                    }
                })
                .sum()
        };

        // Count from characters
        for char in &account_data.characters.characters {
            let char_count = count_char_items(&char.equipment)
                + count_char_items(&char.inventory)
                + count_char_items(&char.backpack)
                + count_char_items(&char.backpack_ext)
                + count_char_items(&char.belt);

            if char.seasonal {
                seasonal += char_count;
            } else {
                regular += char_count;
            }
        }

        // Count from seasonal vault (vault + gifts + materials + potions)
        seasonal += count_vault_items(&account_data.seasonal_vault.vault_items);
        seasonal += count_vault_items(&account_data.seasonal_vault.gift_items);
        seasonal += count_vault_items(&account_data.seasonal_vault.material_items);
        seasonal += count_vault_items(&account_data.seasonal_vault.potion_items);

        // Count from regular vault (vault + gifts + materials + potions + spoils)
        regular += count_vault_items(&account_data.regular_vault.vault_items);
        regular += count_vault_items(&account_data.regular_vault.gift_items);
        regular += count_vault_items(&account_data.regular_vault.material_items);
        regular += count_vault_items(&account_data.regular_vault.potion_items);
        regular += count_vault_items(&account_data.regular_vault.spoils_items);

        // Count from pet inventories
        for pet in account_data.characters.seasonal_pets.values() {
            seasonal += count_pet_items(&pet.inventory);
        }
        for pet in account_data.characters.regular_pets.values() {
            regular += count_pet_items(&pet.inventory);
        }

        (seasonal, regular)
    }

    /// Build a cache of item counts for all required items in current quests.
    fn build_item_counts_cache(
        quests: &[QuestData],
        account_data: &AccountData,
    ) -> HashMap<i32, (u32, u32)> {
        let mut cache = HashMap::new();
        for quest in quests {
            for &item_id in &quest.requirements {
                if !cache.contains_key(&item_id) {
                    cache.insert(item_id, Self::count_items(item_id, account_data));
                }
            }
        }
        cache
    }

    /// Reward sprite of the highest-priority Daily Quest Chest that can be
    /// claimed right now (enough marks collected on Seasonal or Regular), or
    /// `None`. Drives the Quests tab-button claimable cue; memoized on
    /// `(account_epoch, quests_revision)` so it only rebuilds when data changes.
    pub fn claimable_chest_icon(
        &mut self,
        account_data: &AccountData,
        account_epoch: u64,
    ) -> Option<i32> {
        if !self.has_data {
            return None;
        }
        let key = (account_epoch, self.quests_revision);
        self.item_counts_memo.update(key, || {
            Self::build_item_counts_cache(&self.quests, account_data)
        });
        let counts = self.item_counts_memo.get()?;
        claimable_chest_icon(&self.quests, &self.hidden, counts)
    }

    /// Whether the currently-held daily quests are stale after a post-reset
    /// game restart (see `daily_stale_unrefetched`). Used by the tab header and
    /// taskbar to flag that quests need a Tinkerer Cave re-visit before they are
    /// accurate again. `game_start_unix` is the running client's launch time
    /// (Unix seconds), if known.
    pub fn daily_quests_stale(&self, game_start_unix: Option<i64>, now: DateTime<Utc>) -> bool {
        if !self.has_data {
            return false;
        }
        let has_daily = self
            .quests
            .iter()
            .any(|q| is_daily_reset_section(QuestSection::for_quest(q)));
        has_daily
            && daily_stale_unrefetched(
                self.cached_at.map(DateTime::<Utc>::from),
                game_start_unix,
                now,
            )
    }

    /// Render the quest panel.
    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        account_data: &AccountData,
        account_epoch: u64,
        shadcn: &Shadcn,
        game_start_unix: Option<i64>,
    ) -> Vec<AppAction> {
        if !self.has_data {
            // Show placeholder message
            ui.vertical_centered(|ui| {
                ui.add_space(100.0);
                ui.heading("Daily Quests");
                ui.add_space(20.0);
                ui.label("Enter the Daily Quest Room to see your quests.");
                ui.add_space(10.0);
                ui.label(
                    RichText::new("(Visit the Tinkerer in the Nexus)")
                        .weak()
                        .italics(),
                );
            });
            return Vec::new();
        }

        // Per-section counts. A player-hidden quest counts under the special
        // Hidden bucket rather than its natural section. `present` lists the
        // normal (non-Hidden) sections that have at least one quest; a
        // toggled-off section stays present so it can be re-enabled from the
        // "Show categories" menu.
        let (present, hidden_count, total_displayable): (Vec<(QuestSection, usize)>, usize, usize) = {
            let mut counts = [0usize; 9];
            for q in self.displayable_quests() {
                let sec = if self.hidden.contains(&q.id) {
                    QuestSection::Hidden
                } else {
                    QuestSection::for_quest(q)
                };
                counts[sec.id() as usize] += 1;
            }
            let present: Vec<(QuestSection, usize)> = QuestSection::ALL
                .iter()
                .copied()
                .filter(|s| *s != QuestSection::Hidden && counts[s.id() as usize] > 0)
                .map(|s| (s, counts[s.id() as usize]))
                .collect();
            let hidden = counts[QuestSection::Hidden.id() as usize];
            let total: usize = counts.iter().sum();
            (present, hidden, total)
        };

        let enabled_total: usize = present
            .iter()
            .filter(|(s, _)| !self.disabled.contains(&s.id()))
            .map(|(_, c)| c)
            .sum();
        let visible_total = enabled_total + hidden_count;
        let count_text = if visible_total != total_displayable {
            format!("{}/{} quests", visible_total, total_displayable)
        } else {
            format!("{} quests", total_displayable)
        };

        // Resolve item ownership once (memoized) up front so the list below
        // reuses the counts.
        let counts_key = (account_epoch, self.quests_revision);
        self.item_counts_memo.update(counts_key, || {
            Self::build_item_counts_cache(&self.quests, account_data)
        });

        // --- Responsive layout decision, computed before the toolbar so the
        // view toggles can reflect a forced state. On a narrow window we force
        // "Hide names" first, then "Compact", so the grid never overflows.
        // Kept in a scoped block so the immutable `self` borrow is released
        // before the toolbar mutates `self`.
        let now = Utc::now();
        let fetched_at = self.cached_at.map(DateTime::<Utc>::from);
        // Daily quests that expired at the reset boundary but the running game
        // (started at/after that boundary) has not re-fetched: show them as
        // stale rather than as still-available-but-0:00.
        let daily_stale = daily_stale_unrefetched(fetched_at, game_start_unix, now);
        let (max_req, max_reward) = {
            let quests = self.displayable_quests();
            let mut mr = 1usize;
            let mut mw = 1usize;
            for q in &quests {
                mr = mr.max(q.requirements.len());
                mw = mw.max(q.rewards.len());
            }
            (mr, mw)
        };

        // Right margin the compass / Hide button occupy; the grid must end
        // before it. `- 16` leaves room for the scrollbar.
        let avail_width = ui.available_width() - 16.0;
        let right_reserve = TIMER_PILL_RIGHT_RESERVE;
        let fits = |compact: bool, names: bool| {
            Cols::content_width(compact, names, max_req, max_reward) + right_reserve <= avail_width
        };
        let user_show_names = !self.hide_names;
        let (eff_compact, eff_show_names) = if fits(self.compact_view, user_show_names) {
            (self.compact_view, user_show_names)
        } else if fits(self.compact_view, false) {
            (self.compact_view, false)
        } else {
            (true, false)
        };
        let forced_hide_names = user_show_names && !eff_show_names;
        let forced_compact = eff_compact && !self.compact_view;

        // Header toolbar: quest count, view toggles, collapse/expand-all and the
        // "Show categories" visibility menu.
        shadcn.header_band(ui, shadcn.secondary_header_fill(), |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading(count_text);
                ui.add_space(20.0);

                if forced_compact {
                    let mut on = true;
                    ui.add_enabled(false, egui::Checkbox::new(&mut on, "Compact view"))
                        .on_hover_text(
                            "Auto-enabled: the window is too narrow to fit the full layout",
                        );
                } else {
                    ui.checkbox(&mut self.compact_view, "Compact view");
                }
                ui.add_space(12.0);
                if forced_hide_names {
                    let mut on = true;
                    ui.add_enabled(false, egui::Checkbox::new(&mut on, "Hide quest names"))
                        .on_hover_text("Auto-enabled: the window is too narrow to fit quest names");
                } else if ui
                    .checkbox(&mut self.hide_names, "Hide quest names")
                    .on_hover_text("Hides the quest-name column to save space")
                    .changed()
                {
                    self.dirty = true;
                }
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(10.0);

                // Collapse / expand every visible section at once (Hidden included).
                let mut visible: Vec<u8> = present
                    .iter()
                    .filter(|(s, _)| !self.disabled.contains(&s.id()))
                    .map(|(s, _)| s.id())
                    .collect();
                if hidden_count > 0 {
                    visible.push(QuestSection::Hidden.id());
                }
                let any_expanded = visible.iter().any(|id| !self.collapsed.contains(id));
                let label = if any_expanded {
                    "⏶ Collapse all"
                } else {
                    "⏷ Expand all"
                };
                if ui.button(label).clicked() {
                    if any_expanded {
                        self.collapsed.extend(visible.iter().copied());
                    } else {
                        for id in &visible {
                            self.collapsed.remove(id);
                        }
                    }
                    self.dirty = true;
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(10.0);

                // Per-category visibility. Unchecking a bucket hides it (header and
                // cards) from the page. `Hidden by player` is always shown and has
                // no toggle, so it is excluded here.
                ui.menu_button("Show categories ▼", |ui| {
                    for section in QuestSection::ALL
                        .iter()
                        .filter(|s| **s != QuestSection::Hidden)
                    {
                        // Color each entry with its section header color so the menu
                        // matches the on-page headers.
                        ui.visuals_mut().override_text_color = Some(section.color());
                        let mut on = !self.disabled.contains(&section.id());
                        if ui.checkbox(&mut on, section.title()).changed() {
                            if on {
                                self.disabled.remove(&section.id());
                            } else {
                                self.disabled.insert(section.id());
                            }
                            self.dirty = true;
                        }
                    }
                });

                // Data source indicator (right-aligned)
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(time) = self.cached_at {
                        let age = Self::format_time_ago(time);
                        ui.label(
                            RichText::new(format!("• {}", age))
                                .small()
                                .color(Color32::GRAY),
                        );
                    }
                    let (icon, label, color) = if self.from_cache {
                        ("📁", "Cached", Color32::from_rgb(200, 200, 100))
                    } else {
                        ("📡", "Live", Color32::from_rgb(100, 255, 100))
                    };
                    ui.label(
                        RichText::new(format!("{} {}", icon, label))
                            .small()
                            .color(color),
                    );
                });
            });
        });

        // Empty state: nothing to show at all. Sections merely unchecked in the
        // "Show categories" menu are skipped in the loop below.
        if total_displayable == 0 {
            crate::panels::empty_state(
                ui,
                "All quests completed!",
                Some("Check back tomorrow for new quests."),
            );
            return self.take_dirty();
        }

        // Built before `displayable_quests()` borrows `self`, so the memo can
        // still take `&mut self`.
        let item_counts = self
            .item_counts_memo
            .get()
            .expect("item counts built above");

        let cols = Cols::build(eff_compact, eff_show_names, max_req, max_reward);

        // Group displayable quests into buckets, routing hidden quests to the
        // Hidden bucket. Holds an immutable borrow of `self.quests`.
        let mut buckets: Vec<(QuestSection, Vec<&QuestData>)> =
            QuestSection::ALL.iter().map(|&s| (s, Vec::new())).collect();
        for q in self.displayable_quests() {
            let sec = if self.hidden.contains(&q.id) {
                QuestSection::Hidden
            } else {
                QuestSection::for_quest(q)
            };
            buckets[sec.id() as usize].1.push(q);
        }

        // Column header, pinned above the scroll area so it stays visible.
        ui.add_space(6.0);
        quest_header_row(ui, cols);
        shadcn.full_width_separator(ui);
        ui.add_space(4.0);

        let card_fill = shadcn.secondary_header_fill();
        let mut collapse_toggles: Vec<u8> = Vec::new();
        let mut to_hide: Vec<String> = Vec::new();
        let mut to_unhide: Vec<String> = Vec::new();
        let mut toggle_track: Option<String> = None;
        let collapsed = &self.collapsed;
        let disabled = &self.disabled;
        let taskbar = &self.taskbar;

        let mut any_timer = false;

        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (section, quests) in &buckets {
                    if quests.is_empty() {
                        continue;
                    }
                    let is_hidden_sec = *section == QuestSection::Hidden;
                    // A category unchecked in "Show categories" is hidden
                    // entirely (header included). Hidden-by-player is always
                    // shown and has no visibility toggle.
                    if !is_hidden_sec && disabled.contains(&section.id()) {
                        continue;
                    }
                    let expanded = !collapsed.contains(&section.id());

                    let acts = section_header(ui, expanded, *section, quests.len());
                    if acts.toggle_collapse {
                        collapse_toggles.push(section.id());
                    }
                    // A collapsed section shows only its header.
                    if !expanded {
                        ui.add_space(6.0);
                        continue;
                    }
                    ui.add_space(2.0);
                    for quest in quests {
                        let card_timer = effective_quest_timer(quest, fetched_at, now).map(|exp| {
                            let expired = exp <= now;
                            // An expired reset reads as "0h 0m" (paired with
                            // the red colour + "restart to refresh" tooltip),
                            // not "<1m" which looks like time still remains.
                            let text = if expired {
                                "0h 0m".to_string()
                            } else {
                                format_remaining_short(exp, now)
                            };
                            if is_daily_reset_section(QuestSection::for_quest(quest)) {
                                QuestStatusTimer::Resets { text, expired }
                            } else {
                                QuestStatusTimer::Ends { text, expired }
                            }
                        });
                        // A live per-card timer keeps the panel repainting so
                        // the countdown ticks.
                        if card_timer.is_some() {
                            any_timer = true;
                        }
                        quest_card(
                            ui,
                            quest,
                            item_counts,
                            sprite_renderer,
                            cols,
                            card_fill,
                            is_hidden_sec,
                            &mut to_hide,
                            &mut to_unhide,
                            taskbar.enabled && *section == QuestSection::DailyChests,
                            taskbar.quest_tracked(&quest.id),
                            &mut toggle_track,
                            card_timer,
                            daily_stale && is_daily_reset_section(QuestSection::for_quest(quest)),
                        );
                    }
                    ui.add_space(10.0);
                }
            });

        if any_timer {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs(1));
        }

        for id in collapse_toggles {
            if !self.collapsed.remove(&id) {
                self.collapsed.insert(id);
            }
            self.dirty = true;
        }
        let mut taskbar_changed = false;
        for id in to_hide {
            // A hidden quest stops being tracked in the Taskbar so it no longer
            // shows there and its compass reads off.
            if self.taskbar.quest_tracked(&id) {
                self.taskbar.set_quest_tracked(id.clone(), false);
                taskbar_changed = true;
            }
            self.hidden.insert(id);
            self.dirty = true;
            self.quests_revision = self.quests_revision.wrapping_add(1);
        }
        for id in to_unhide {
            self.hidden.remove(&id);
            self.dirty = true;
            self.quests_revision = self.quests_revision.wrapping_add(1);
        }

        let mut actions = self.take_dirty();
        if let Some(id) = toggle_track {
            let now = self.taskbar.quest_tracked(&id);
            self.taskbar.set_quest_tracked(id, !now);
            taskbar_changed = true;
        }
        if taskbar_changed {
            actions.push(AppAction::SaveTaskbarSettings(self.taskbar.clone()));
        }
        actions
    }

    /// Emit a save action if persisted state changed this frame.
    fn take_dirty(&mut self) -> Vec<AppAction> {
        if self.dirty {
            self.dirty = false;
            vec![AppAction::SaveQuestsSettings(self.to_settings())]
        } else {
            Vec::new()
        }
    }
}

impl Default for QuestPanel {
    fn default() -> Self {
        Self::new(std::path::PathBuf::new())
    }
}

// Layout metrics for the aligned quest table (base, unscaled px).
const PAD_X: f32 = 10.0;
const PAD_Y: f32 = 6.0;
const COL_GAP: f32 = 8.0;
const CARD_GAP: f32 = 4.0;

/// Type-pill colors. Blue matches the Skin Exchange header shade.
/// Fixed width reserved for the "Seasonal" / "Regular" labels on the collected
/// mark rows beneath the Required tiles.
const COLLECTED_LABEL_W: f32 = 52.0;
/// Gap between the small collected-mark tiles.
const SMALL_TILE_GAP: f32 = 2.0;
/// Approximate width an "or" / "+" reward separator adds between reward tiles.
const REWARD_SEP_W: f32 = 18.0;

/// Fixed column widths for the quest table, shared by the pinned header and
/// every card so all buckets line up. Tile columns are sized to the global max
/// tile count; text columns keep a minimum so headers never clip.
#[derive(Clone, Copy)]
struct Cols {
    tile: f32,
    small_tile: f32,
    tile_gap: f32,
    show_quest: bool,
    quest: f32,
    status: f32,
    req: f32,
    arrow: f32,
    reward: f32,
    font: f32,
}

impl Cols {
    fn build(compact: bool, show_quest: bool, max_req: usize, max_reward: usize) -> Cols {
        let scale = if compact { 0.8 } else { 1.0 };
        let tile = 38.0 * scale;
        let small_tile = 22.0 * scale;
        let tile_gap = if compact { 2.0 } else { 4.0 };
        let tiles_w = |n: usize| {
            let n = n.max(1) as f32;
            n * tile + (n - 1.0) * tile_gap
        };
        // Width of a collected-marks row: the "Seasonal"/"Regular" label plus a
        // smaller tile per requirement.
        let small_row_w = |n: usize| {
            let n = n.max(1) as f32;
            COLLECTED_LABEL_W + 6.0 + n * small_tile + (n - 1.0) * SMALL_TILE_GAP
        };
        // Reward tiles are joined by "or"/"+" separators; reserve room for them.
        let reward_w = {
            let n = max_reward.max(1) as f32;
            n * tile + (n - 1.0) * (tile_gap + REWARD_SEP_W)
        };
        Cols {
            tile,
            small_tile,
            tile_gap,
            show_quest,
            quest: 160.0,
            status: 128.0,
            // The Required column also stacks the smaller Seasonal / Regular
            // collected rows beneath the requirement tiles.
            req: tiles_w(max_req).max(small_row_w(max_req)).max(150.0),
            arrow: 22.0,
            reward: reward_w.max(64.0),
            font: 14.0 * scale,
        }
    }

    /// Total horizontal space the card grid needs for the given options
    /// (left+right padding, every column, and the inter-column gaps). Used to
    /// decide when a narrow window must drop quest names / go compact.
    fn content_width(compact: bool, show_quest: bool, max_req: usize, max_reward: usize) -> f32 {
        let c = Cols::build(compact, show_quest, max_req, max_reward);
        let mut widths: Vec<f32> = Vec::new();
        if show_quest {
            widths.push(c.quest);
        }
        widths.push(c.status);
        widths.push(c.req);
        widths.push(c.arrow);
        widths.push(c.reward);
        let sum: f32 = widths.iter().sum();
        let gaps = (widths.len() as f32 - 1.0) * COL_GAP;
        2.0 * PAD_X + sum + gaps
    }
}

/// Fixed-size column cell (`w` x `h`) whose content is vertically centred; the
/// width is always reserved so columns stay aligned across rows.
fn cell(ui: &mut egui::Ui, w: f32, h: f32, add: impl FnOnce(&mut egui::Ui)) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::hover());
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    add(&mut child);
}

/// The pinned column-header row: Quest | Status | Required | Reward. Sits above
/// the scroll area so it stays put.
fn quest_header_row(ui: &mut egui::Ui, cols: Cols) {
    let grey = Color32::from_gray(170);
    let lbl = |s: &str, c: Color32| RichText::new(s).strong().size(12.0).color(c);
    let h = 16.0;
    ui.horizontal(|ui| {
        ui.add_space(PAD_X);
        ui.spacing_mut().item_spacing.x = COL_GAP;
        if cols.show_quest {
            cell(ui, cols.quest, h, |ui| {
                ui.label(lbl("Quest", grey));
            });
        }
        cell(ui, cols.status, h, |ui| {
            ui.label(lbl("Status", grey));
        });
        cell(ui, cols.req, h, |ui| {
            ui.label(lbl("Required", grey));
        });
        cell(ui, cols.arrow, h, |_| {});
        cell(ui, cols.reward, h, |ui| {
            ui.label(lbl("Reward", grey));
        });
    });
    ui.add_space(2.0);
}

/// Render a run of plain item tiles left-aligned. When `overlay_blueprints` is
/// set (reward column), blueprint tiles get the unlocked item's icon stamped in
/// the top-left corner so the reward is identifiable at a glance without hovering.
/// Render a run of plain item tiles left-aligned. Blueprint tiles get the
/// unlocked item's icon stamped in the top-left corner automatically by the
/// shared slot renderer.
fn render_tiles(ui: &mut egui::Ui, sr: &mut SpriteRenderer, ids: &[i32], cols: Cols) {
    ui.spacing_mut().item_spacing.x = cols.tile_gap;
    for &id in ids {
        sr.render_item_tile_sized(ui, id, None, &[], cols.tile);
    }
}

/// Render the reward tiles joined by a connector, mirroring the Missions reward
/// line: "or" between mutually-exclusive choices, "+" for a grouped bundle.
/// Rewards draw as outlined sprites without inventory slots (unlike the Required
/// marks, whose slots signal you must physically carry them to the Tinkerer).
fn render_quest_rewards(ui: &mut egui::Ui, sr: &mut SpriteRenderer, quest: &QuestData, tile: f32) {
    let sep = if quest.item_of_choice { "or" } else { "+" };
    ui.spacing_mut().item_spacing.x = 5.0;
    // Inset the art inside the layout cell so a slot-less reward still renders
    // at the same ~32px art size as the slotted Required marks (the raw sprite
    // would otherwise fill the whole 38px cell and look oversized).
    let inset = tile * 3.0 / 38.0;
    for (i, &id) in quest.rewards.iter().enumerate() {
        if i > 0 {
            ui.label(RichText::new(sep).size(11.5).color(Color32::from_gray(160)));
        }
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(tile, tile), egui::Sense::hover());
        if id > 0 {
            let art = rect.shrink(inset);
            sr.draw_outlined_sprite_in_rect_native(ui, id, art);
            // Forge blueprints stamp the unlocked item's icon in the top-left
            // corner so the reward is identifiable without hovering.
            sr.draw_blueprint_unlock_overlay(ui, id, art);
        }
        if let Some(name) = sr.item_name(id) {
            resp.on_hover_text(name);
        }
    }
}

/// One collected-marks row (Seasonal or Regular): a small coloured label, then a
/// smaller mark tile per requirement, greyed when the player lacks that copy.
fn render_small_collected(
    ui: &mut egui::Ui,
    sr: &mut SpriteRenderer,
    quest: &QuestData,
    item_counts: &HashMap<i32, (u32, u32)>,
    cols: Cols,
    seasonal: bool,
) {
    let (label, color) = if seasonal {
        ("Seasonal", SEASONAL_COLOR)
    } else {
        ("Regular", REGULAR_COLOR)
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = SMALL_TILE_GAP;
        let (r, _) = ui.allocate_exact_size(
            egui::vec2(COLLECTED_LABEL_W, cols.small_tile),
            egui::Sense::hover(),
        );
        ui.painter().text(
            egui::pos2(r.left(), r.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(10.5),
            color,
        );
        let mut used: HashMap<i32, u32> = HashMap::new();
        for &id in &quest.requirements {
            let slot = *used.entry(id).or_insert(0);
            used.insert(id, slot + 1);
            let owned = item_counts
                .get(&id)
                .map(|&(s, r)| if seasonal { s } else { r })
                .unwrap_or(0);
            render_item_slot(ui, id, slot < owned, sr, cols.small_tile);
        }
    });
}

/// Render the Status column stack (mission-style): the reset / expiry timer and
/// a "Repeatable" line. A daily reset already implies repetition, so the
/// Repeatable line is suppressed when a reset timer is showing.
fn render_quest_status(
    ui: &mut egui::Ui,
    quest: &QuestData,
    timer: &Option<QuestStatusTimer>,
    stale: bool,
) {
    let yellow = Color32::from_rgb(235, 205, 70);
    let grey = Color32::from_gray(120);
    let red = Color32::from_rgb(200, 90, 90);

    // A daily reset already implies repetition, so the Repeatable line is
    // suppressed when a reset timer is showing.
    let show_repeatable =
        quest.repeatable && !matches!(timer, Some(QuestStatusTimer::Resets { .. }));
    let line_count = timer.is_some() as u32 + show_repeatable as u32;
    if line_count == 0 {
        return;
    }

    // Stack the lines in a manually-built top_down child (vertically centred on
    // the cell's middle axis). This mirrors the Missions status column: a bare
    // `ui.vertical(..)` scope would register a container hover-response *after*
    // the timer row, becoming the top-most hovered widget and stealing hover
    // from the row -- so its tooltip would never fire.
    let cell = ui.max_rect();
    let block_h = line_count as f32 * 16.0;
    let inner = egui::Rect::from_min_size(
        egui::pos2(cell.left(), cell.center().y - block_h / 2.0),
        egui::vec2(cell.width(), block_h),
    );
    let mut sui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    let ui = &mut sui;
    ui.spacing_mut().item_spacing.y = 2.0;

    if let Some(t) = timer {
        let mut resets = false;
        let (glyph, verb, text, expired) = match t {
            QuestStatusTimer::Resets { text, expired } => {
                resets = true;
                ("\u{27f3}", "Resets", text, *expired)
            }
            QuestStatusTimer::Ends { text, expired } => ("\u{29d7}", "Ends", text, *expired),
        };
        let col = if expired { red } else { grey };
        let row = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.label(RichText::new(glyph).size(13.0).color(col));
            ui.label(
                RichText::new(format!("{verb} {text}"))
                    .size(11.0)
                    .color(col),
            );
        });
        // A daily reset that has hit 0h 0m needs an in-game restart to refresh;
        // otherwise explain when the countdown lands. When the quests are stale
        // after a post-reset restart the card shows a whole-card "re-enter the
        // cave" tooltip instead, so the timer row skips its own tip here.
        if !stale {
            let tip = match (resets, expired) {
                (true, true) => "Daily quests expired. Restarting your game will reset them.",
                (true, false) => "Time left until these quests reset at daily reset",
                (false, _) => "Time left until this quest expires",
            };
            show_status_tip(ui, row.response.rect, &quest.id, "timer", tip);
        }
    }
    if show_repeatable {
        let row = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.label(RichText::new("\u{27f3}").size(13.0).color(yellow));
            ui.label(RichText::new("Repeatable").size(11.0).color(yellow));
        });
        show_status_tip(
            ui,
            row.response.rect,
            &quest.id,
            "repeat",
            "This quest can be repeated several times",
        );
    }
}

/// Force a tooltip open when the pointer is geometrically over `rect`, bypassing
/// egui's `hovered()`/`contains_pointer()` occlusion checks. The Status column's
/// timer/repeatable rows sit under the card's and cell's hover-sensing rects, so
/// a normal `on_hover_text`/`hover_tip` (both gated on those flags) never fires.
fn show_status_tip(ui: &egui::Ui, rect: egui::Rect, quest_id: &str, salt: &str, text: &str) {
    if !ui.rect_contains_pointer(rect) {
        return;
    }
    egui::Tooltip::always_open(
        ui.ctx().clone(),
        ui.layer_id(),
        ui.id().with(("quest_status_tip", quest_id, salt)),
        rect,
    )
    .show(|ui| {
        ui.label(text);
    });
}

/// Text of the one-per-account note, shared by the card renderer (so it can
/// measure the wrapped height) and the tooltip.
const UNIQUE_NOTE_TEXT: &str = "This quest can be only completed once on your account";

/// Yellow italic note shown above the Required tiles for a one-per-account quest.
fn render_unique_note(ui: &mut egui::Ui) {
    let yellow = Color32::from_rgb(235, 205, 70);
    ui.add(
        egui::Label::new(
            RichText::new(UNIQUE_NOTE_TEXT)
                .size(11.0)
                .italics()
                .color(yellow),
        )
        .wrap(),
    )
    .on_hover_text("Unique: one completion per account");
}

/// Render the collected-availability column for one storage side (seasonal or
/// regular): one slot per requirement, greyed when the player lacks that copy.
fn render_collected(
    ui: &mut egui::Ui,
    sr: &mut SpriteRenderer,
    quest: &QuestData,
    item_counts: &HashMap<i32, (u32, u32)>,
    cols: Cols,
    seasonal: bool,
) {
    ui.spacing_mut().item_spacing.x = cols.tile_gap;
    let mut used: HashMap<i32, u32> = HashMap::new();
    for &id in &quest.requirements {
        let slot = *used.entry(id).or_insert(0);
        used.insert(id, slot + 1);
        let owned = item_counts
            .get(&id)
            .map(|&(s, r)| if seasonal { s } else { r })
            .unwrap_or(0);
        render_item_slot(ui, id, slot < owned, sr, cols.tile);
    }
}

/// Render an item slot - full color if available, greyed out if not.
fn render_item_slot(
    ui: &mut egui::Ui,
    item_id: i32,
    is_available: bool,
    sr: &mut SpriteRenderer,
    tile_size: f32,
) {
    let response = sr.render_item_tile_sized(ui, item_id, None, &[], tile_size);
    if !is_available {
        ui.painter().rect_filled(
            response.rect,
            2.0,
            Color32::from_rgba_unmultiplied(0, 0, 0, 160),
        );
    }
}

/// Render a Daily Quest Chest as a vertical hover card for the Live Feed
/// Taskbar, reproducing the Quests tab card visuals: the status line, the quest
/// name, the requirement -> reward tiles, and the collected-on-Seasonal /
/// Regular availability rows.
pub(crate) fn render_quest_tooltip(
    ui: &mut egui::Ui,
    sr: &mut SpriteRenderer,
    quest: &QuestData,
    item_counts: &HashMap<i32, (u32, u32)>,
    seasonal: bool,
    exclude_dungeons: &HashSet<String>,
) {
    ui.spacing_mut().item_spacing.y = 6.0;
    let cols = Cols::build(false, false, quest.requirements.len(), quest.rewards.len());

    if quest.repeatable {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            let yellow = Color32::from_rgb(235, 205, 70);
            ui.label(RichText::new("\u{27f3}").size(13.0).color(yellow));
            ui.label(RichText::new("Repeatable").size(11.0).color(yellow));
        });
    }

    ui.label(
        RichText::new(&quest.name)
            .strong()
            .size(15.0)
            .color(Color32::WHITE),
    );

    if is_unique_quest(quest) {
        render_unique_note(ui);
    }

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        render_tiles(ui, sr, &quest.requirements, cols);
        ui.label(
            RichText::new("→")
                .strong()
                .size(15.0)
                .color(Color32::from_gray(200)),
        );
        render_quest_rewards(ui, sr, quest, cols.tile);
    });

    ui.label(
        RichText::new("Collected")
            .size(12.0)
            .strong()
            .color(Color32::from_gray(160)),
    );
    // Reserve one fixed label width for both rows so the Seasonal and Regular
    // mark columns line up vertically under each other.
    let lbl_font = egui::FontId::proportional(11.0);
    let label_w = ui
        .painter()
        .layout_no_wrap("Seasonal".to_string(), lbl_font.clone(), Color32::WHITE)
        .size()
        .x;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (r, _) = ui.allocate_exact_size(egui::vec2(label_w, cols.tile), egui::Sense::hover());
        ui.painter().text(
            egui::pos2(r.left(), r.center().y),
            egui::Align2::LEFT_CENTER,
            "Seasonal",
            lbl_font.clone(),
            SEASONAL_COLOR,
        );
        render_collected(ui, sr, quest, item_counts, cols, true);
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (r, _) = ui.allocate_exact_size(egui::vec2(label_w, cols.tile), egui::Sense::hover());
        ui.painter().text(
            egui::pos2(r.left(), r.center().y),
            egui::Align2::LEFT_CENTER,
            "Regular",
            lbl_font.clone(),
            REGULAR_COLOR,
        );
        render_collected(ui, sr, quest, item_counts, cols, false);
    });

    // Where the required marks come from. A mark that's already fully collected
    // on the active storage side drops out (its dungeon disappears). Low-grave
    // dungeons that share a biome are folded into "efficient areas"; grave-7+
    // dungeons (and any low-grave ones with no shared biome) keep an individual
    // per-dungeon column.
    let am = get_asset_manager();
    let mut need: HashMap<i32, i32> = HashMap::new();
    let mut order: Vec<i32> = Vec::new();
    for &id in &quest.requirements {
        if !need.contains_key(&id) {
            order.push(id);
        }
        *need.entry(id).or_insert(0) += 1;
    }
    let mut dungeons: Vec<String> = Vec::new();
    for id in order {
        let (s, r) = item_counts.get(&id).copied().unwrap_or((0, 0));
        let collected = if seasonal { s } else { r } as i32;
        if collected >= need[&id] {
            continue;
        }
        let Some(name) = am.object_name(id) else {
            continue;
        };
        if let Some(dn) = realmhound_core::assets::dungeon_for_mark_name(&name) {
            // Skip dungeons the paired mission already shows (e.g. an Advanced
            // Kogbold mark resolves to base "Kogbold Steamworks" but the mission
            // shows the "Advanced Kogbold Steamworks" variant -- one section is
            // enough).
            if exclude_dungeons.contains(&crate::panels::taskbar::canonical_match_dungeon(dn)) {
                continue;
            }
            if crate::panels::missions::dungeon_has_tip_info(dn)
                && !dungeons.iter().any(|n| n == dn)
            {
                dungeons.push(dn.to_string());
            }
        }
    }
    let plan = crate::panels::missions::plan_drop_tip(&dungeons);
    if !plan.efficient.is_empty() || !plan.individual.is_empty() {
        ui.separator();
    }
    if !plan.efficient.is_empty() {
        crate::panels::missions::render_efficient_areas(ui, sr, &plan.efficient);
    }
    if !plan.individual.is_empty() {
        if !plan.efficient.is_empty() {
            ui.separator();
        }
        ui.horizontal_top(|ui| {
            let divider = ui.visuals().widgets.noninteractive.bg_stroke;
            let mut rects: Vec<egui::Rect> = Vec::new();
            for (k, dn) in plan.individual.iter().enumerate() {
                if k > 0 {
                    ui.add_space(13.0);
                }
                let r = ui
                    .vertical(|ui| {
                        ui.set_max_width(240.0);
                        crate::panels::missions::render_dungeon_tip_body(ui, sr, dn, dn);
                    })
                    .response
                    .rect;
                rects.push(r);
            }
            // Vertical dividers painted only as tall as the columns. A
            // `ui.separator()` here would grow to the tooltip's available
            // height and leave a large empty gap below the shorter column.
            let top = rects.iter().map(|r| r.top()).fold(f32::INFINITY, f32::min);
            let bottom = rects
                .iter()
                .map(|r| r.bottom())
                .fold(f32::NEG_INFINITY, f32::max);
            for pair in rects.windows(2) {
                let x = (pair[0].right() + pair[1].left()) * 0.5;
                ui.painter().vline(x, top..=bottom, divider);
            }
        });
    }
}

/// Render a single quest as an aligned dark card: hover outline, a quest-name
/// tooltip, and a Hide/Unhide button. Pushes the quest id to `to_hide` /
/// `to_unhide` when its button is clicked.
#[allow(clippy::too_many_arguments)]
fn quest_card(
    ui: &mut egui::Ui,
    quest: &QuestData,
    item_counts: &HashMap<i32, (u32, u32)>,
    sr: &mut SpriteRenderer,
    cols: Cols,
    card_fill: Color32,
    in_hidden: bool,
    to_hide: &mut Vec<String>,
    to_unhide: &mut Vec<String>,
    show_compass: bool,
    tracked: bool,
    toggle_track: &mut Option<String>,
    timer: Option<QuestStatusTimer>,
    stale: bool,
) {
    let unique = is_unique_quest(quest);
    // The Required column stacks (optional unique note) + requirement tiles +
    // the smaller Seasonal and Regular collected rows, so the card height must
    // grow to fit that block. Measure the note's wrapped height at the column
    // width so a two-line note (narrow columns) never clips.
    let vgap = 3.0;
    let unique_h = if unique {
        ui.painter()
            .layout(
                UNIQUE_NOTE_TEXT.to_string(),
                egui::FontId::proportional(11.0),
                Color32::WHITE,
                cols.req,
            )
            .size()
            .y
    } else {
        0.0
    };
    let req_rows = if unique { 4.0 } else { 3.0 };
    let req_stack_h = unique_h + cols.tile + 2.0 * cols.small_tile + (req_rows - 1.0) * vgap;
    // Status column may show up to two short lines.
    let status_lines = (timer.is_some() as u32
        + (quest.repeatable && !matches!(timer, Some(QuestStatusTimer::Resets { .. }))) as u32)
        as f32;
    let status_h = status_lines * 16.0;
    let content_h = req_stack_h.max(status_h).max(cols.tile + 4.0);
    let card_h = content_h + PAD_Y * 2.0;
    let avail = ui.available_width();
    let (card_rect, _) = ui.allocate_exact_size(egui::vec2(avail, card_h), egui::Sense::hover());
    ui.painter().rect_filled(card_rect, 6.0, card_fill);

    // A Daily Quest Chest with every required mark collected is claimable now:
    // draw the same soft glow + crisp gold outline the Missions cards use. A
    // stale (expired, un-refetched) card is never claimable -- its red styling
    // takes precedence since the chest is no longer available in game.
    let claimable = !stale
        && QuestSection::for_quest(quest) == QuestSection::DailyChests
        && chest_completable(quest, item_counts);
    if claimable {
        let gold = Color32::from_rgb(235, 205, 70);
        let p = ui.painter();
        p.rect_stroke(
            card_rect.expand(1.0),
            7.0,
            egui::Stroke::new(
                3.0_f32,
                Color32::from_rgba_unmultiplied(gold.r(), gold.g(), gold.b(), 26),
            ),
            egui::StrokeKind::Outside,
        );
        p.rect_stroke(
            card_rect,
            6.0,
            egui::Stroke::new(2.0_f32, gold),
            egui::StrokeKind::Inside,
        );
    }

    let content_rect = egui::Rect::from_min_max(
        card_rect.min + egui::vec2(PAD_X, PAD_Y),
        card_rect.max - egui::vec2(PAD_X, PAD_Y),
    );
    let mut cui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(content_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    cui.spacing_mut().item_spacing.x = COL_GAP;

    if cols.show_quest {
        cell(&mut cui, cols.quest, content_h, |ui| {
            let t = RichText::new(&quest.name).strong().size(cols.font);
            ui.add(egui::Label::new(t).truncate());
        });
    }

    // Status column: reset/expiry timer + repeatable indicator (mission-style).
    cell(&mut cui, cols.status, content_h, |ui| {
        render_quest_status(ui, quest, &timer, stale);
    });

    // Required column: an optional "unique" note, the requirement tiles, then
    // the smaller Seasonal / Regular collected rows stacked beneath.
    cell(&mut cui, cols.req, content_h, |ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = vgap;
            if unique {
                render_unique_note(ui);
            }
            ui.horizontal(|ui| {
                render_tiles(ui, sr, &quest.requirements, cols);
            });
            render_small_collected(ui, sr, quest, item_counts, cols, true);
            render_small_collected(ui, sr, quest, item_counts, cols, false);
        });
    });
    cell(&mut cui, cols.arrow, content_h, |ui| {
        ui.label(RichText::new("→").strong().size(cols.font));
    });
    cell(&mut cui, cols.reward, content_h, |ui| {
        render_quest_rewards(ui, sr, quest, cols.tile);
    });

    // Hide / Unhide button, top-right corner of the card. In the Hidden bucket
    // it becomes "Unhide" with a return-arrow glyph marking the undo action.
    let label = if in_hidden { "\u{27f2} Unhide" } else { "Hide" };
    let font = egui::FontId::proportional(11.0);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), font.clone(), Color32::WHITE);
    let size = galley.size() + egui::vec2(12.0, 6.0);
    let btn_rect = egui::Rect::from_min_size(
        egui::pos2(card_rect.right() - 8.0 - size.x, card_rect.top() + 6.0),
        size,
    );
    let btn = ui.interact(
        btn_rect,
        ui.id().with(("qhide", quest.id.as_str())),
        egui::Sense::click(),
    );
    let (bg, fg) = if btn.hovered() {
        (Color32::from_gray(64), Color32::from_gray(235))
    } else {
        (Color32::from_gray(40), Color32::from_gray(155))
    };
    ui.painter().rect_filled(btn_rect, 4.0, bg);
    ui.painter().text(
        btn_rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        font,
        fg,
    );
    let tip = if in_hidden {
        "Unhide this quest"
    } else {
        "Hide this quest (find it under \"Hidden by player\")"
    };
    let btn = btn
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .hover_tip(tip);
    if btn.clicked() {
        if in_hidden {
            to_unhide.push(quest.id.clone());
        } else {
            to_hide.push(quest.id.clone());
        }
    }

    // Compass toggle (left of Hide) to add/remove this daily-quest chest from
    // the Live Feed Taskbar.
    if show_compass {
        let cs = 20.0;
        let comp_rect = egui::Rect::from_min_size(
            egui::pos2(btn_rect.left() - 5.0 - cs, btn_rect.center().y - cs * 0.5),
            egui::vec2(cs, cs),
        );
        let r = crate::panels::missions::taskbar_compass_button(
            ui,
            sr,
            comp_rect,
            ui.id().with(("taskbar_compass_q", quest.id.as_str())),
            tracked,
        );
        let tip = if tracked {
            "Tracked in the Taskbar. Click to remove."
        } else {
            "Click to add this quest to your Taskbar"
        };
        if r.hover_tip(tip).clicked() {
            *toggle_track = Some(quest.id.clone());
        }
    }

    // Blue outline on hover, matching Missions/Loot cards. Skipped for claimable
    // cards so their gold outline stays dominant, and for stale cards so their
    // red outline stays dominant. No card-level name tooltip -- the Quest column
    // already labels each row (avoids a double tip).
    if !claimable && !stale && ui.rect_contains_pointer(card_rect) {
        ui.painter().rect_stroke(
            card_rect,
            6.0,
            egui::Stroke::new(1.5_f32, Color32::from_rgb(90, 130, 170)),
            egui::StrokeKind::Inside,
        );
    }

    // Stale daily quests (expired at reset, not yet re-fetched): dim the card
    // and draw a red outline, matching the Missions cooldown-card treatment, to
    // flag that these quests are no longer available until the Tinkerer Cave is
    // re-entered in game. Hovering anywhere on the card surfaces the same
    // "re-enter the cave" note in red italic.
    if stale {
        ui.painter()
            .rect_filled(card_rect, 6.0, Color32::from_black_alpha(96));
        ui.painter().rect_stroke(
            card_rect,
            6.0,
            egui::Stroke::new(1.5_f32, Color32::from_rgb(210, 55, 55)),
            egui::StrokeKind::Inside,
        );
        if ui.rect_contains_pointer(card_rect) {
            egui::Tooltip::always_open(
                ui.ctx().clone(),
                ui.layer_id(),
                ui.id().with(("quest_stale_tip", quest.id.as_str())),
                card_rect,
            )
            .show(|ui| {
                ui.label(
                    RichText::new(STALE_QUEST_TOOLTIP)
                        .italics()
                        .color(Color32::from_rgb(210, 55, 55)),
                );
            });
        }
    }

    ui.add_space(CARD_GAP);
}

/// Outcome of a section header interaction this frame.
struct HeaderActions {
    toggle_collapse: bool,
}

/// A collapsible quest-section header: a collapse arrow, colored title and
/// count. Category visibility is controlled from the toolbar's "Show
/// categories" menu, not from the header.
fn section_header(
    ui: &mut egui::Ui,
    expanded: bool,
    section: QuestSection,
    count: usize,
) -> HeaderActions {
    let mut acts = HeaderActions {
        toggle_collapse: false,
    };
    ui.horizontal(|ui| {
        ui.add_space(2.0);

        let arrow = if expanded { "▼" } else { "▶" };
        if ui
            .selectable_label(
                false,
                RichText::new(format!("{arrow}  {}", section.title()))
                    .strong()
                    .size(14.0)
                    .color(section.color()),
            )
            .clicked()
        {
            acts.toggle_collapse = true;
        }
        ui.label(
            RichText::new(format!("({count})"))
                .size(12.0)
                .color(Color32::from_gray(130)),
        );
    });
    acts
}

impl Panel for QuestPanel {
    fn show(&mut self, ui: &mut egui::Ui, ctx: &mut PanelContext) -> Vec<AppAction> {
        self.render(
            ui,
            ctx.sprite_renderer,
            ctx.account_data,
            ctx.account_epoch,
            ctx.shadcn,
            (ctx.client_launch_unix > 0).then_some(ctx.client_launch_unix),
        )
    }

    fn handle_event(
        &mut self,
        event: &realmhound_core::GameEvent,
        _session: &realmhound_core::GameSession,
    ) -> Vec<AppAction> {
        match event {
            realmhound_core::GameEvent::QuestsReceived(response) => {
                self.update_quests(response);
                self.pending_redeem_quest_id = None;
            }
            realmhound_core::GameEvent::QuestRedeemAttempted { quest_id } => {
                self.pending_redeem_quest_id = Some(quest_id.clone());
            }
            realmhound_core::GameEvent::QuestRedeemResult { ok } => {
                if let Some(quest_id) = self.pending_redeem_quest_id.take() {
                    if *ok {
                        self.mark_quest_completed(&quest_id);
                    }
                }
            }
            _ => {}
        }
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(name: &str, cat: i32) -> QuestData {
        QuestData {
            id: "x".into(),
            name: name.into(),
            description: String::new(),
            expiration: String::new(),
            category: cat,
            unknown_int: 0,
            requirements: vec![],
            rewards: vec![],
            completed: false,
            item_of_choice: false,
            repeatable: false,
        }
    }

    #[test]
    fn quest_cache_round_trips_through_injected_path() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("quests").join("quests.json");
        let mut panel = QuestPanel::new(path.clone());
        panel.quests = vec![mk("Daily", 0)];
        panel.cached_at = Some(SystemTime::now());
        panel.save_cache();
        assert!(path.exists(), "quest cache written to the injected path");

        // A second panel bound to the same path recovers the cached quests.
        let reloaded = QuestPanel::new(path);
        assert!(reloaded.from_cache);
        assert_eq!(reloaded.quests.len(), 1);
        assert_eq!(reloaded.quests[0].name, "Daily");
    }

    #[test]
    fn two_accounts_use_isolated_quest_paths() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a").join("quests.json");
        let b = temp.path().join("b").join("quests.json");
        let panel_a = QuestPanel::new(a.clone());
        panel_a.save_cache();
        assert!(a.exists());
        assert!(!b.exists(), "the second profile's quest file is untouched");
        assert_ne!(a, b);
    }

    #[test]
    fn quest_timer_ignores_empty_and_far_future() {
        let now = Utc::now();
        let mut q = mk("q", 500); // Other category, no daily synthesis
        assert!(quest_timer(&q, now).is_none()); // empty
        q.expiration = "not-a-date".into();
        assert!(quest_timer(&q, now).is_none()); // unparseable
        q.expiration = (now + chrono::Duration::days(400)).to_rfc3339();
        assert!(quest_timer(&q, now).is_none()); // unreasonably far
        q.expiration = (now + chrono::Duration::hours(5)).to_rfc3339();
        assert!(quest_timer(&q, now).is_some());
    }

    #[test]
    fn parse_expiration_accepts_epoch_seconds() {
        // Live server sends Unix epoch seconds, sometimes with a ".0" suffix.
        let dt = parse_expiration("1787644800.0").expect("epoch parses");
        assert_eq!(dt.to_rfc3339(), "2026-08-25T08:00:00+00:00");
        assert_eq!(parse_expiration("1787644800").unwrap(), dt);
        assert!(parse_expiration("0").is_none());
        assert!(parse_expiration("0.0").is_none());
        assert!(parse_expiration("").is_none());
    }

    #[test]
    fn new_quest_reanchors_reset_but_stale_refetch_does_not() {
        let quest = |id: &str| {
            let mut q = mk("daily", 0);
            q.id = id.into();
            q
        };
        let held = vec![quest("a"), quest("b")];
        // Re-entering the Tinkerer with the same ids is not a new day.
        assert!(!introduces_new_quest(&held, &[quest("a"), quest("b")]));
        // Completing a quest can shrink the set to a subset -- still not new.
        assert!(!introduces_new_quest(&held, &[quest("a")]));
        // A genuine daily re-roll brings a previously-unseen id.
        assert!(introduces_new_quest(&held, &[quest("a"), quest("c")]));
        // First-ever fetch (nothing held) is always treated as new.
        assert!(introduces_new_quest(&[], &[quest("a")]));
    }

    #[test]
    fn daily_reset_is_next_midnight_and_can_expire() {
        // Fetched at 14:00 UTC -> resets at the next day's 00:00 UTC.
        let fetched = DateTime::parse_from_rfc3339("2026-01-20T14:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let reset = daily_reset(Some(fetched), fetched);
        assert_eq!(reset.to_rfc3339(), "2026-01-21T00:00:00+00:00");
        // Playing past that midnight without a refetch -> expired (reset < now).
        let later = DateTime::parse_from_rfc3339("2026-01-21T03:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(daily_reset(Some(fetched), later) - later < chrono::Duration::zero());
    }

    #[test]
    fn chest_completable_needs_one_full_storage_side() {
        let mut q = mk("Advanced Kogbold", 0);
        q.requirements = vec![10, 10, 20]; // two of mark 10, one of mark 20
                                           // Neither side complete.
        let none: HashMap<i32, (u32, u32)> = HashMap::new();
        assert!(!chest_completable(&q, &none));
        // Seasonal side satisfies all (2x id10, 1x id20); regular short.
        let seasonal_ok: HashMap<i32, (u32, u32)> = [(10, (2u32, 0u32)), (20, (1u32, 0u32))]
            .into_iter()
            .collect();
        assert!(chest_completable(&q, &seasonal_ok));
        // Split across sides does NOT count - must be one full side.
        let split: HashMap<i32, (u32, u32)> = [(10, (2u32, 0u32)), (20, (0u32, 1u32))]
            .into_iter()
            .collect();
        assert!(!chest_completable(&q, &split));
        // Empty requirements are never completable.
        let empty = mk("Empty", 0);
        assert!(!chest_completable(&empty, &seasonal_ok));
    }

    #[test]
    fn daily_stale_unrefetched_flags_only_post_reset_restart() {
        use chrono::TimeZone;
        // Quests fetched 2024-06-01 10:00 UTC -> reset boundary 2024-06-02 00:00.
        let fetched = Some(Utc.with_ymd_and_hms(2024, 6, 1, 10, 0, 0).unwrap());
        let boundary_ts = Utc
            .with_ymd_and_hms(2024, 6, 2, 0, 0, 0)
            .unwrap()
            .timestamp();

        // Before the reset: never stale, whatever the start time.
        let before = Utc.with_ymd_and_hms(2024, 6, 1, 23, 0, 0).unwrap();
        assert!(!daily_stale_unrefetched(
            fetched,
            Some(boundary_ts + 60),
            before
        ));

        // After the reset but the game has been running since before it: keep the
        // normal "restart to refresh" behavior, not stale.
        let after = Utc.with_ymd_and_hms(2024, 6, 2, 1, 0, 0).unwrap();
        assert!(!daily_stale_unrefetched(
            fetched,
            Some(boundary_ts - 3600),
            after
        ));

        // After the reset, game (re)started at/after the boundary yet quests were
        // not re-fetched -> stale.
        assert!(daily_stale_unrefetched(
            fetched,
            Some(boundary_ts + 300),
            after
        ));
        assert!(daily_stale_unrefetched(fetched, Some(boundary_ts), after));

        // Unknown or non-positive start time fails safe to not-stale.
        assert!(!daily_stale_unrefetched(fetched, None, after));
        assert!(!daily_stale_unrefetched(fetched, Some(0), after));
        assert!(!daily_stale_unrefetched(fetched, Some(-5), after));
    }

    #[test]
    fn category_maps_to_expected_base_section() {
        use QuestSection::*;
        assert_eq!(QuestSection::base_for_category(0), DailyChests);
        // Category 100 (The Cursed Heart's seasonal chest) is not a known base
        // category; it reaches Daily Quest Chests structurally via its reward,
        // not through base_for_category.
        assert_eq!(QuestSection::base_for_category(100), Other);
        // Permanent-rewards category range (Umi + ore + craftsmanship live here
        // before name refinement), inclusive endpoints.
        for c in [999, 1000, 1001, 6999, 7000, 7162, 7163] {
            assert_eq!(
                QuestSection::base_for_category(c),
                PermanentRewards,
                "cat {c}"
            );
        }
        // Repeatable exchange (equipment + saddlebag).
        for c in [7010, 7011, 7012, 11000] {
            assert_eq!(
                QuestSection::base_for_category(c),
                RepeatableExchange,
                "cat {c}"
            );
        }
        for c in [9080, 9095, 9096, 9097] {
            assert_eq!(QuestSection::base_for_category(c), SkinExchange, "cat {c}");
        }
        for c in [
            1, 100, 101, 998, 1002, 7009, 7013, 7161, 7164, 9094, 9098, 10999, 11001,
        ] {
            assert_eq!(QuestSection::base_for_category(c), Other, "cat {c}");
        }
    }

    #[test]
    fn quest_chest_name_predicate_matches_all_tiers() {
        assert!(name_is_quest_chest("Standard Quest Chest"));
        assert!(name_is_quest_chest("Cultish Epic Quest Chest"));
        assert!(name_is_quest_chest("Spectral Epic Quest Chest"));
        assert!(name_is_quest_chest("beginner quest chest"));
        assert!(!name_is_quest_chest("Greater Potion of Life"));
        assert!(!name_is_quest_chest("Egg"));
    }

    #[test]
    fn unknown_category_without_chest_reward_is_other() {
        // The Cursed Heart's category (100) is unknown; without a resolvable
        // chest reward it falls back to Other. In-game it reaches Daily Quest
        // Chests structurally via awards_quest_chest once assets are loaded.
        assert_eq!(
            QuestSection::for_quest(&mk("The Cursed Heart", 100)),
            QuestSection::Other
        );
    }

    #[test]
    fn named_quests_move_to_refined_sections() {
        use QuestSection::*;
        assert_eq!(
            QuestSection::for_quest(&mk("Eggs for Breakfast", 0)),
            DailyMisc
        );
        assert_eq!(QuestSection::for_quest(&mk("Scout the Pit", 0)), DailyMisc);
        assert_eq!(
            QuestSection::for_quest(&mk("Potion Fusion: Wis", 0)),
            DailyMisc
        );
        assert_eq!(QuestSection::for_quest(&mk("Clean Laundry", 0)), DailyMisc);
        assert_eq!(
            QuestSection::for_quest(&mk("Advanced Kogbold", 0)),
            DailyChests
        );
        // "The Ancients" is a daily chest quest, not a schematic exchange.
        assert_eq!(QuestSection::for_quest(&mk("The Ancients", 0)), DailyChests);
        assert_eq!(
            QuestSection::for_quest(&mk("Forgotten Craftsmanship", 6999)),
            WeeklySchematic
        );
        assert_eq!(
            QuestSection::for_quest(&mk("Ancient Craftsmanship", 7000)),
            WeeklySchematic
        );
        assert_eq!(
            QuestSection::for_quest(&mk("Common Ore Refinement", 999)),
            RepeatableExchange
        );
        assert_eq!(
            QuestSection::for_quest(&mk("Umi's Benevolence", 999)),
            PermanentRewards
        );
    }

    #[test]
    fn unique_quests_detected() {
        assert!(is_unique_quest(&mk("Umi's Benevolence", 999)));
        assert!(is_unique_quest(&mk("Pet Trial", 0)));
        assert!(!is_unique_quest(&mk("Advanced Kogbold", 0)));
    }

    #[test]
    fn section_ids_are_stable_and_unique() {
        let ids: Vec<u8> = QuestSection::ALL.iter().map(|s| s.id()).collect();
        assert_eq!(ids, vec![0, 1, 2, 3, 4, 5, 6, 7, 8]);
    }
}

//! Combat History panel - browsable record of reconstructed boss fights.
//!
//! Lists recent fights as cards (dungeon portal, boss portrait, time, local
//! character, participant count, duration) and opens a DPS
//! breakdown when a card is selected. All figures come from data already
//! persisted by the combat engine; each damage value carries a provenance badge
//! so the UI never presents unavailable data as a factual zero.

use crate::ui_ext::HoverTooltipExt;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

use chrono::{Local, TimeZone, Utc};
use eframe::egui::{self, Color32, RichText, ScrollArea};
use realmhound_core::{
    assets::{get_asset_manager, get_dungeon_portal_map, BossGroup, CatalogEntry},
    combat::{
        build_bundle, sanitize_player_name, CombatDatabase, DamageProvenance,
        DamageTakenProvenance, EncounterRecord, FightQuery, FightRecord, FightSelection,
        FightSummary, ParticipantEndStatus, ParticipantRecord, LOOT_LINK_POST_MS, LOOT_LINK_PRE_MS,
    },
    loot::{LootDatabase, LootDropRecord, LootItemRecord},
};

use crate::panels::loot::DateFilter;
use crate::panels::{empty_state, empty_state_lines, AppAction, Panel, PanelContext};

/// How often to poll the fight count for new fights (wall clock).
const POLL_INTERVAL_MS: u128 = 1000;

/// How many fights are fetched per page. The list starts with one page and
/// grows by another whenever the user scrolls near the bottom.
const PAGE_SIZE: i64 = 200;

/// Fixed height of a fight card in the list.
const CARD_HEIGHT: f32 = 52.0;

/// Oryx the Mad God 3 boss object type. Only these fights show the "Guarded"
/// damage column. The Messenger aux representative (45365) is
/// excluded since guard damage is only tracked on the main O3 body.
const O3_BOSS_TYPE: i32 = 45363;

/// Cached autocomplete data (distinct bosses/dungeons) for search suggestions.
#[derive(Default)]
struct AutocompleteCache {
    dungeons: Vec<String>,
    bosses: Vec<(i32, String)>,
    players: Vec<(String, i32, u32, u32)>,
    /// Distinct participant counts present in the DB, ascending.
    participant_counts: Vec<i64>,
    loaded: bool,
}

/// A single suggestion row in the search autocomplete popup.
#[derive(Clone)]
enum SearchResult {
    Dungeon {
        name: String,
        icon_id: i32,
        display_name: String,
    },
    Boss {
        object_type: i32,
        name: String,
    },
    Player {
        name: String,
        icon_id: i32,
        tex1: u32,
        tex2: u32,
    },
    PlayerCount {
        count: i64,
    },
}

/// A pending destructive action awaiting confirmation in the modal dialog.
///
/// Delete actions snapshot the exact set of rows at the moment the dialog opens
/// so that later filter changes cannot alter what gets removed on confirm.
#[derive(Clone)]
enum ConfirmAction {
    /// Delete the entries that were checked when opened.
    DeleteSelected(Vec<FightSelection>),
}

/// Combat History panel state.
pub struct CombatHistoryPanel {
    /// Loaded fight summaries for the list (most recent first).
    summaries: Vec<FightSummary>,
    /// Loot drops correlated to each fight/encounter selection.
    loot_links: HashMap<FightSelection, Vec<LootDropRecord>>,
    /// Currently opened selection (single fight or grouped encounter), if any.
    selected: Option<FightSelection>,
    /// Cached full record for an opened standalone fight.
    detail: Option<FightRecord>,
    /// Cached full record for an opened multi-phase encounter.
    encounter: Option<EncounterRecord>,
    /// Which phase rows are expanded in the encounter Phases section.
    expanded_phases: std::collections::HashSet<i64>,
    /// Free-text search box contents.
    search: String,
    /// Active dungeon filter (exact name).
    filter_dungeon: Option<String>,
    /// Active boss filter (object type + display name for the chip).
    filter_boss: Option<(i32, String)>,
    /// Active character filter (char id + display name for the chip).
    filter_char: Option<(i32, String)>,
    /// Active player-IGN filter (IGN + most-recent character sprite/dyes for the chip).
    filter_player: Option<(String, i32, u32, u32)>,
    /// Active participant-count filter (exact roster size).
    filter_player_count: Option<i64>,
    /// Active date-range filter for the fight list.
    filter_date: DateFilter,
    /// Whether the date-range dropdown is currently open (its popup paints over
    /// the cards without blocking them, so we suppress card clicks while open).
    date_menu_open: bool,
    /// Last observed total fight count (for change detection).
    last_count: i64,
    /// Last observed total loot-drop count, so late bags refresh the links.
    last_loot_count: i64,
    /// When the fight count was last polled.
    last_poll: Option<Instant>,
    /// Force a reload on the next frame (filters changed / tab entered).
    needs_refresh: bool,
    /// Current page-growing limit passed to `list_fights`. Starts at one
    /// [`PAGE_SIZE`] and grows by a page when the user scrolls near the bottom.
    /// Reset to one page whenever the query changes.
    loaded_limit: i64,
    /// Whether the last reload filled the whole window, i.e. more rows may exist
    /// past the ones currently loaded. Drives scroll-triggered load-more.
    has_more: bool,
    /// Query used for the last successful reload, so a filter/search/date change
    /// resets the paging window while a poll/grow reload preserves it.
    last_query: Option<FightQuery>,
    /// Whether the last reload attempt failed (throttles retries).
    last_refresh_failed: bool,
    /// Show the Raw DMG column in the detail table.
    show_dmg: bool,
    /// Show the Hits column in the detail table.
    show_hits: bool,
    /// Show the Damage taken column in the detail table.
    show_damage_taken: bool,
    /// Show the Damage blocked column in the detail table.
    show_damage_blocked: bool,
    /// Cached distinct bosses/dungeons for search autocomplete.
    autocomplete: AutocompleteCache,
    /// Whether the search suggestion popup is open.
    search_popup_open: bool,
    /// Keyboard-selected suggestion index (None = no selection).
    search_selected_index: Option<usize>,
    /// Previous search text, to reset the selection when it changes.
    search_prev_text: String,
    /// Active group filter keys (`BossGroup::as_str`); empty means "all groups".
    filter_groups: std::collections::HashSet<&'static str>,
    /// Secret-stat toggle filters (union): only cards with the enabled flag show.
    filter_lone_fighter: bool,
    filter_last_hero: bool,
    filter_most_damage_taken: bool,
    filter_close_calls: bool,
    /// Boss-group tooltip entries, built once on first hover for each group.
    tooltip_catalogs: HashMap<BossGroup, Vec<CatalogEntry>>,
    /// Entries checked for batch deletion.
    selected_for_delete: std::collections::HashSet<FightSelection>,
    /// All fight selections matching the current filters, refreshed on reload.
    /// Backs the "Select filtered" toggle without querying every frame.
    filtered_selection: Vec<FightSelection>,
    /// Pending destructive action awaiting the confirmation modal.
    confirm: Option<ConfirmAction>,
    /// Last known vertical scroll offset of the fight list, so PgUp/PgDn can
    /// page relative to the current position.
    list_scroll_offset: f32,
    /// When set, the fight list is scrolled back to the top on the next render
    /// (used to reset scroll position when the tab is (re)activated).
    reset_scroll_pending: bool,
    /// Transient status line shown in the toolbar after an export (message +
    /// when it was set + whether it is an error).
    status_message: Option<(String, Instant, bool)>,
    /// Combat history database path for the short-lived delete writers. Injected
    /// so the panel resolves no path itself.
    combat_db_path: std::path::PathBuf,
}

impl Default for CombatHistoryPanel {
    fn default() -> Self {
        Self {
            summaries: Vec::new(),
            loot_links: HashMap::new(),
            selected: None,
            detail: None,
            encounter: None,
            expanded_phases: std::collections::HashSet::new(),
            search: String::new(),
            filter_dungeon: None,
            filter_boss: None,
            filter_char: None,
            filter_player: None,
            filter_player_count: None,
            filter_date: DateFilter::AllTime,
            date_menu_open: false,
            last_count: -1,
            last_loot_count: -1,
            last_poll: None,
            needs_refresh: true,
            loaded_limit: PAGE_SIZE,
            has_more: false,
            last_query: None,
            last_refresh_failed: false,
            show_dmg: true,
            show_hits: true,
            show_damage_taken: true,
            show_damage_blocked: true,
            autocomplete: AutocompleteCache::default(),
            search_popup_open: false,
            search_selected_index: None,
            search_prev_text: String::new(),
            filter_groups: std::collections::HashSet::new(),
            filter_lone_fighter: false,
            filter_last_hero: false,
            filter_most_damage_taken: false,
            filter_close_calls: false,
            tooltip_catalogs: HashMap::new(),
            selected_for_delete: std::collections::HashSet::new(),
            filtered_selection: Vec::new(),
            confirm: None,
            list_scroll_offset: 0.0,
            reset_scroll_pending: false,
            status_message: None,
            combat_db_path: std::path::PathBuf::new(),
        }
    }
}

impl CombatHistoryPanel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind the combat history database path used by the short-lived delete
    /// writers.
    pub fn set_database_path(&mut self, combat_db_path: std::path::PathBuf) {
        self.combat_db_path = combat_db_path;
    }

    /// Filter the list to a specific character and open the list view. Invoked
    /// from a character card's "View boss fights" entry.
    pub fn view_character_fights(&mut self, char_id: i32, char_name: String) {
        self.filter_char = Some((char_id, char_name));
        self.selected = None;
        self.detail = None;
        self.encounter = None;
        self.needs_refresh = true;
    }

    pub fn open_fight(&mut self, selection: FightSelection) {
        self.filter_dungeon = None;
        self.filter_boss = None;
        self.filter_char = None;
        self.filter_player = None;
        self.filter_player_count = None;
        self.filter_groups.clear();
        self.search.clear();
        self.search_prev_text.clear();
        self.filter_date = DateFilter::AllTime;
        self.selected = Some(selection);
        self.detail = None;
        self.encounter = None;
        self.expanded_phases.clear();
        self.needs_refresh = true;
    }

    /// Apply a player filter selected by clicking a participant's icon in a
    /// detail view: filter the list to that IGN and return to the list.
    fn apply_player_click(&mut self, name: String, icon: i32, tex1: u32, tex2: u32) {
        self.filter_player = Some((name, icon, tex1, tex2));
        self.selected = None;
        self.detail = None;
        self.encounter = None;
        self.expanded_phases.clear();
        self.needs_refresh = true;
    }

    /// Move the opened card to its neighbour in the current filtered list
    /// (`delta` of -1 = previous, +1 = next). No-op at the list boundaries or
    /// when the current selection is not part of the filtered set.
    fn navigate_selection(&mut self, delta: i32) {
        let Some(cur) = self.selected.clone() else {
            return;
        };
        let Some(idx) = self.filtered_selection.iter().position(|s| *s == cur) else {
            return;
        };
        let new_idx = idx as i32 + delta;
        if new_idx < 0 || new_idx as usize >= self.filtered_selection.len() {
            return;
        }
        self.selected = Some(self.filtered_selection[new_idx as usize].clone());
        self.detail = None;
        self.encounter = None;
        self.expanded_phases.clear();
    }

    /// Whether the opened card has a previous/next neighbour in the filtered
    /// list, driving the enabled state of the navigation buttons.
    fn selection_neighbours(&self) -> (bool, bool) {
        let idx = self
            .selected
            .as_ref()
            .and_then(|cur| self.filtered_selection.iter().position(|s| s == cur));
        match idx {
            Some(i) => (i > 0, i + 1 < self.filtered_selection.len()),
            None => (false, false),
        }
    }

    /// Reset the fight list back to the top on the next render. Called when the
    /// Combat History tab is (re)activated so it always opens on the most recent
    /// entries instead of inheriting a stale scroll position.
    pub fn reset_scroll(&mut self) {
        self.reset_scroll_pending = true;
    }

    /// Force the list to reload from the database on the next render and drop
    /// any cached view/selection state. Invoked when the Combat History
    /// database is mutated from outside the panel (e.g. the settings-tab
    /// "Delete logs of all boss fights" action).
    pub fn request_reload(&mut self) {
        self.selected = None;
        self.detail = None;
        self.encounter = None;
        self.selected_for_delete.clear();
        self.filtered_selection.clear();
        self.autocomplete.loaded = false;
        self.last_count = -1;
        self.loaded_limit = PAGE_SIZE;
        self.last_query = None;
        self.needs_refresh = true;
    }

    fn current_query(&self) -> FightQuery {
        let (after, before) = match self.filter_date {
            DateFilter::AllTime => (None, None),
            other => {
                let (start, end) = other.range_millis();
                (Some(start), Some(end))
            }
        };
        FightQuery {
            text: {
                let t = self.search.trim();
                if t.is_empty() {
                    None
                } else {
                    Some(t.to_string())
                }
            },
            dungeon: self.filter_dungeon.clone(),
            boss_object_type: self.filter_boss.as_ref().map(|(t, _)| *t),
            char_id: self.filter_char.as_ref().map(|(id, _)| *id),
            player_name: self.filter_player.as_ref().map(|(n, ..)| n.clone()),
            participant_count: self.filter_player_count,
            after,
            before,
            groups: if self.filter_groups.is_empty() {
                None
            } else {
                Some(self.filter_groups.iter().map(|s| s.to_string()).collect())
            },
            filter_lone_fighter: self.filter_lone_fighter,
            filter_last_hero: self.filter_last_hero,
            filter_most_damage_taken: self.filter_most_damage_taken,
            filter_close_calls: self.filter_close_calls,
        }
    }

    fn reload(&mut self, db: &CombatDatabase, loot_db: Option<&LootDatabase>) -> bool {
        let query = self.current_query();
        // A new filter/search/date starts the window fresh at one page; a poll or
        // scroll-triggered grow keeps the (possibly enlarged) window.
        if self.last_query.as_ref() != Some(&query) {
            self.loaded_limit = PAGE_SIZE;
        }
        match db.list_fights(&query, self.loaded_limit) {
            Ok(list) => {
                let selections = match db.selections_matching(&query) {
                    Ok(selections) => selections,
                    Err(e) => {
                        // Keep the prior list and selection state on a transient query error.
                        tracing::warn!("[COMBAT] selections_matching failed: {}", e);
                        return false;
                    }
                };
                // A full window means more rows may exist past what we loaded.
                self.has_more = list.len() as i64 >= self.loaded_limit;
                self.summaries = list;
                self.filtered_selection = selections;
                self.last_query = Some(query);
                self.rebuild_loot_links(loot_db);
                true
            }
            Err(e) => {
                // Keep the prior list on a transient query error (R5/R6).
                tracing::warn!("[COMBAT] list_fights failed: {}", e);
                false
            }
        }
    }

    /// Rebuild the fight->loot correlation cache from the current summaries.
    /// Only killed cards are linked, matching the loot->fight predicate, so an
    /// escaped run never shows loot. Cheap enough to run on each list reload or
    /// when the loot count changes.
    fn rebuild_loot_links(&mut self, loot_db: Option<&LootDatabase>) {
        self.loot_links.clear();
        let Some(loot_db) = loot_db else {
            return;
        };
        // Gather candidate drops per eligible (killed) fight. For back-to-back
        // kills of the same encounter in one realm the per-fight windows overlap,
        // so a single bag can appear as a candidate for several fights. The
        // exclusive assignment below resolves each bag to the one closest kill so
        // it is not shown on every neighbouring card.
        let mut fights: Vec<(FightSelection, i64, i64)> = Vec::new();
        let mut candidates: Vec<(usize, LootDropRecord)> = Vec::new();
        for summary in &self.summaries {
            if !summary.killed || summary.map_seed == 0 {
                continue;
            }
            let mut boss_types: Vec<i32> = summary
                .killed_bosses
                .iter()
                .map(|(object_type, _)| *object_type)
                .filter(|object_type| *object_type > 0)
                .collect();
            if boss_types.is_empty() && summary.boss_object_type > 0 {
                boss_types.push(summary.boss_object_type);
            }
            boss_types.sort_unstable();
            boss_types.dedup();
            if boss_types.is_empty() {
                continue;
            }
            // A self-destructing boss's loot is emitted by a proxy chest whose
            // bags carry the chest's mob_type (Legacy Lair of Draconis dragons).
            // Query those chest types too so the bags surface on the boss's card.
            for chest in boss_types
                .iter()
                .filter_map(|t| realmhound_core::assets::loot_emitter_for_boss(*t))
                .collect::<Vec<_>>()
            {
                boss_types.push(chest);
            }

            let time_lo = summary.started_at - LOOT_LINK_PRE_MS;
            let time_hi = summary.ended_at + LOOT_LINK_POST_MS;
            match loot_db.find_drops_for_run(summary.map_seed, &boss_types, time_lo, time_hi) {
                Ok(drops) if !drops.is_empty() => {
                    let idx = fights.len();
                    fights.push((summary.selection(), summary.started_at, summary.ended_at));
                    for drop in drops {
                        candidates.push((idx, drop));
                    }
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("[COMBAT] find_drops_for_run failed: {}", e),
            }
        }

        let windows: Vec<(i64, i64)> = fights.iter().map(|(_, s, e)| (*s, *e)).collect();
        let assign_input: Vec<(usize, i64, i64)> = candidates
            .iter()
            .map(|(fi, drop)| (*fi, drop.id, drop.timestamp))
            .collect();
        let chosen = assign_drops_exclusive(&windows, &assign_input);

        let mut seen: HashSet<i64> = HashSet::new();
        for (fi, drop) in candidates {
            if chosen.get(&drop.id) != Some(&fi) {
                continue;
            }
            if !seen.insert(drop.id) {
                continue;
            }
            self.loot_links
                .entry(fights[fi].0.clone())
                .or_default()
                .push(drop);
        }
    }

    /// Load distinct bosses/dungeons for autocomplete (once, until invalidated).
    fn load_autocomplete(&mut self, db: &CombatDatabase) {
        if self.autocomplete.loaded {
            return;
        }
        if let Ok(d) = db.distinct_dungeons() {
            self.autocomplete.dungeons = d;
        }
        if let Ok(b) = db.distinct_bosses() {
            self.autocomplete.bosses = b;
        }
        if let Ok(p) = db.distinct_players() {
            self.autocomplete.players = p;
        }
        if let Ok(c) = db.distinct_participant_counts() {
            self.autocomplete.participant_counts = c;
        }
        self.autocomplete.loaded = true;
    }

    /// Poll for new fights on a throttle; reload when the count changes or a
    /// refresh was explicitly requested. Never queries the DB every frame.
    fn maybe_refresh(&mut self, db: &CombatDatabase, loot_db: Option<&LootDatabase>) {
        realmhound_core::prof_scope!("combat_db_refresh");
        self.load_autocomplete(db);

        let poll_due = self
            .last_poll
            .map(|t| t.elapsed().as_millis() >= POLL_INTERVAL_MS)
            .unwrap_or(true);

        if self.needs_refresh {
            // A filter/search change refreshes immediately (snappy); only a
            // failed attempt is throttled to avoid hammering an unavailable DB.
            if self.last_refresh_failed && !poll_due {
                return;
            }
            let ok = self.reload(db, loot_db);
            self.last_refresh_failed = !ok;
            if ok {
                self.needs_refresh = false;
                if let Ok(count) = db.fight_count() {
                    self.last_count = count;
                }
            } else {
                // Start a backoff window before retrying.
                self.last_poll = Some(Instant::now());
            }
            return;
        }

        if poll_due {
            self.last_poll = Some(Instant::now());
            if let Ok(count) = db.fight_count() {
                if count != self.last_count && self.reload(db, loot_db) {
                    self.last_count = count;
                    // New fights may introduce new bosses/dungeons.
                    self.autocomplete.loaded = false;
                }
            }
            // Late-arriving loot bags (offscreen drops) don't create a fight, so
            // refresh the correlation cache when the loot count changes even if
            // the fight list is unchanged.
            if let Some(loot_db) = loot_db {
                if let Ok(loot_count) = loot_db.total_drops() {
                    if loot_count != self.last_loot_count {
                        self.rebuild_loot_links(Some(loot_db));
                        self.last_loot_count = loot_count;
                    }
                }
            }
        }
    }

    /// Apply a chosen search suggestion as a boss/dungeon filter and reset the
    /// search box + popup state.
    fn apply_search_result(&mut self, result: SearchResult) {
        match result {
            SearchResult::Boss { object_type, name } => {
                self.filter_boss = Some((object_type, name));
            }
            SearchResult::Dungeon { name, .. } => {
                self.filter_dungeon = Some(name);
            }
            SearchResult::Player {
                name,
                icon_id,
                tex1,
                tex2,
            } => {
                self.filter_player = Some((name, icon_id, tex1, tex2));
            }
            SearchResult::PlayerCount { count } => {
                self.filter_player_count = Some(count);
            }
        }
        self.search.clear();
        self.search_prev_text.clear();
        self.search_popup_open = false;
        self.search_selected_index = None;
        self.needs_refresh = true;
    }

    fn has_filters(&self) -> bool {
        self.filter_dungeon.is_some()
            || self.filter_boss.is_some()
            || self.filter_char.is_some()
            || self.filter_player.is_some()
            || self.filter_player_count.is_some()
            || !self.filter_groups.is_empty()
            || !self.search.trim().is_empty()
    }
}

/// Format a millisecond duration as `m:ss`.
fn fmt_duration(ms: i64) -> String {
    let secs = (ms.max(0)) / 1000;
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// Render the active participant-count filter as a removable pill with a
/// people glyph (mirrors `render_filter_pill`, but text-only since a count has
/// no sprite). Returns true when the close button is clicked.
fn render_count_pill(ui: &mut egui::Ui, count: i64) -> bool {
    const H: f32 = 20.0;
    const LEFT_PAD: f32 = 6.0;
    const GAP: f32 = 6.0; // text -> ✕
    const X_W: f32 = 10.0;
    const RIGHT_PAD: f32 = 6.0;

    // Single atomic widget so the wrapping filter bar can measure and fold it
    // instead of overflowing. Clicking removes the player-count filter.
    let font = egui::FontId::proportional(11.0);
    let text = format!("👥 {count}");
    let galley = ui
        .painter()
        .layout_no_wrap(text, font.clone(), Color32::WHITE);
    let text_w = galley.size().x;
    let width = LEFT_PAD + text_w + GAP + X_W + RIGHT_PAD;

    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, H), egui::Sense::click());
    let hovered = resp.hovered();
    let clicked = resp.clicked();
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    ui.painter().rect_filled(
        rect,
        egui::CornerRadius::same(4),
        Color32::from_rgb(40, 60, 50),
    );

    let text_pos = egui::pos2(
        rect.left() + LEFT_PAD,
        rect.center().y - galley.size().y / 2.0,
    );
    ui.painter().galley(text_pos, galley, Color32::WHITE);

    let x_center = egui::pos2(rect.right() - RIGHT_PAD - X_W / 2.0, rect.center().y);
    let x_color = if hovered {
        Color32::from_rgb(255, 120, 120)
    } else {
        Color32::from_gray(200)
    };
    ui.painter().text(
        x_center,
        egui::Align2::CENTER_CENTER,
        "\u{2715}",
        font,
        x_color,
    );

    resp.on_hover_text("Remove player-count filter");
    clicked
}

/// Format an integer with thousands separators (e.g. `128,000`).
fn fmt_thousands(n: i64) -> String {
    let neg = n < 0;
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    let len = digits.len();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if neg {
        format!("-{out}")
    } else {
        out
    }
}

/// Format an epoch-ms timestamp in the local timezone.
fn fmt_time(ms: i64) -> String {
    match Local.timestamp_millis_opt(ms).single() {
        Some(dt) => dt.format("%Y-%m-%d %H:%M").to_string(),
        None => "-".to_string(),
    }
}

/// Human label + color for a provenance badge.
fn provenance_badge(p: DamageProvenance) -> (&'static str, Color32) {
    match p {
        DamageProvenance::Observed => ("Observed", Color32::from_rgb(120, 200, 120)),
        DamageProvenance::PartiallyObserved => {
            ("Partially observed", Color32::from_rgb(220, 190, 90))
        }
        DamageProvenance::SelfComputed => ("Self-computed", Color32::from_rgb(120, 200, 120)),
        DamageProvenance::SelfPartial => ("Partial", Color32::from_rgb(220, 190, 90)),
        DamageProvenance::SelfPending => ("Pending", Color32::from_rgb(180, 150, 90)),
        DamageProvenance::SelfReconciled => ("Reconciled", Color32::from_rgb(120, 200, 120)),
        DamageProvenance::SelfServerReconciled => ("Reconciled", Color32::from_rgb(120, 200, 120)),
        DamageProvenance::SelfEstimated => ("Estimated", Color32::from_rgb(220, 190, 90)),
        DamageProvenance::Unresolved => ("Unresolved", Color32::GRAY),
    }
}

fn provenance_tooltip(p: DamageProvenance) -> &'static str {
    match p {
        DamageProvenance::Observed => {
            "Summed from server damage packets while this player was visible."
        }
        DamageProvenance::PartiallyObserved => {
            "Some ally damage was dealt before the local player arrived to the \
             boss fight and wasn't recorded."
        }
        DamageProvenance::SelfComputed => {
            "The game never sends your own damage in any packet - it's recomputed \
             here from the map seed, your weapon, stats, and the target's defense. \
             Every hit was resolved, though the total may still differ slightly \
             from in-game."
        }
        DamageProvenance::SelfPartial => {
            "The game never sends your own damage in any packet, so it's recomputed \
             here. Some of your hits couldn't be matched to a shot (a capture gap, \
             or you joined mid-fight), so this is a lower bound - your real damage \
             was higher."
        }
        DamageProvenance::SelfPending => {
            "The game never sends your own damage in any packet, and it couldn't be \
             recomputed for this fight (missing map seed or weapon data). Hit count \
             only."
        }
        DamageProvenance::SelfReconciled => {
            "You were the only attacker and the boss was fully killed, so its entire \
             HP loss is your damage. Some of it (server-fired ability or summon \
             projectiles) never arrives in any packet, so the unaccounted remainder \
             is credited to you to match the boss's HP exactly."
        }
        DamageProvenance::SelfServerReconciled => {
            "The game never sends your own damage in any packet, but in this solo \
             fight the server re-broadcast your hits with the owner hidden. That \
             gives an exact total for your direct damage, which is used here in \
             place of the approximate recomputed figure."
        }
        DamageProvenance::SelfEstimated => {
            "The game never sends your own damage in any packet, so it's recomputed \
             here. Some of your hits couldn't be matched to a shot, so their damage \
             was estimated from the average of the hits that were - an approximation, \
             not an exact total."
        }
        DamageProvenance::Unresolved => {
            "Attacker could not be resolved to a named player (out of range or minion)."
        }
    }
}

fn damage_taken_tooltip(p: DamageTakenProvenance) -> &'static str {
    match p {
        DamageTakenProvenance::Observed => {
            "Summed from server damage packets while this player was visible - \
             exact, post-defense damage they took from the boss/enemies."
        }
        DamageTakenProvenance::Estimated => {
            "The game never sends your own damage taken in any packet - it's \
             recomputed here from each enemy projectile's damage and your defense \
             stat. Approximate."
        }
        DamageTakenProvenance::Partial => {
            "Recomputed here from enemy projectiles and your defense, but at least \
             one incoming hit couldn't be valued (uncorrelated bullet or unknown \
             defense), so this is a lower bound - you actually took more."
        }
    }
}

/// Resolve a character's icon sprite id and cloth dyes by char id from account
/// data. Returns `(icon_id, tex1, tex2)`, or `None` when the character is not in
/// the cache (e.g. dead/unknown) so callers can fall back to the recorded class.
fn char_visuals(ctx: &PanelContext, char_id: i32) -> Option<(i32, u32, u32)> {
    if char_id == 0 {
        return None;
    }
    let c = ctx.account_data.find_character(char_id)?;
    let icon_id = if c.skin > 0 {
        c.skin
    } else {
        c.class_id as i32
    };
    Some((icon_id, c.tex1, c.tex2))
}

/// Fall back to the appearance recorded on the fight's local participant when
/// the live account cache no longer has the character (e.g. after death), so
/// the card icon keeps its skin and dyes instead of reverting to the plain
/// class sprite. Returns `None` when there is nothing custom to draw (no skin
/// and no dyes), letting the caller render the default class sprite.
fn stored_char_visuals(fight: &FightSummary) -> Option<(i32, u32, u32)> {
    if fight.local_skin_id <= 0 && fight.local_tex1 == 0 && fight.local_tex2 == 0 {
        return None;
    }
    let icon_id = if fight.local_skin_id > 0 {
        fight.local_skin_id
    } else {
        fight.local_object_type.unwrap_or(0)
    };
    if icon_id == 0 {
        return None;
    }
    Some((icon_id, fight.local_tex1, fight.local_tex2))
}

/// Sentinel key stored in `filter_groups` to represent the explicit "None"
/// state (hide every group). An empty set means "all groups", so a real key is
/// needed to distinguish "none" from "all".
const NO_GROUPS_KEY: &str = "__none__";

/// A sprite toggle button for one boss group, styled like the loot filter bar.
/// `filter_groups` uses "empty == all groups" semantics, so an empty set shows
/// every button as enabled.
fn group_filter_button(
    ui: &mut egui::Ui,
    ctx: &mut PanelContext,
    group: BossGroup,
    filter_groups: &mut std::collections::HashSet<&'static str>,
    tooltip_catalogs: &mut HashMap<BossGroup, Vec<CatalogEntry>>,
    needs_refresh: &mut bool,
) {
    let key = group.as_str();
    let enabled = filter_groups.is_empty() || filter_groups.contains(key);

    let size = egui::vec2(20.0, 20.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let drawn = ctx
            .sprite_renderer
            .draw_sprite_in_rect(ui, group.sprite_id(), rect);
        if !drawn {
            ui.painter()
                .rect_filled(rect, 2.0, Color32::from_rgb(55, 55, 60));
        }
        if enabled {
            ui.painter().rect_stroke(
                rect,
                2.0,
                egui::Stroke::new(1.0_f32, Color32::WHITE),
                egui::StrokeKind::Outside,
            );
        } else {
            ui.painter().rect_filled(
                rect.expand(2.0),
                2.0,
                Color32::from_rgba_unmultiplied(0, 0, 0, 180),
            );
        }
    }

    // Tooltip: group name + every boss in the group, each with a sprite.
    // Build lazily, then retain it for subsequent hover frames.
    let response = if response.hovered() {
        let bosses = tooltip_catalogs
            .entry(group)
            .or_insert_with(|| group.catalog_bosses());
        response.hover_tip_ui(|ui| {
            ui.label(RichText::new(group.label()).strong());
            ui.add_space(2.0);
            if bosses.is_empty() {
                ui.label(RichText::new("No bosses in this group").italics().weak());
            } else {
                // Lay bosses out in fixed-height columns rather than a ScrollArea:
                // a ScrollArea inside a hover tooltip fights the tooltip's auto-size
                // (it shrinks on each re-hover) and can't be scrolled anyway. Columns
                // grow the tooltip horizontally so even 100+ minibosses all fit.
                const COL_ROWS: usize = 18;
                ui.horizontal_top(|ui| {
                    for chunk in bosses.chunks(COL_ROWS) {
                        ui.vertical(|ui| {
                            for entry in chunk {
                                ui.horizontal(|ui| {
                                    for otype in &entry.sprite_ids {
                                        let (r, _) = ui.allocate_exact_size(
                                            egui::vec2(16.0, 16.0),
                                            egui::Sense::hover(),
                                        );
                                        let sid = realmhound_core::assets::normalize_train_sprite(
                                            *otype,
                                            &entry.name,
                                        );
                                        ctx.sprite_renderer.draw_sprite_in_rect(ui, sid, r);
                                    }
                                    ui.label(&entry.name);
                                });
                            }
                        });
                        ui.add_space(10.0);
                    }
                });
            }
        })
    } else {
        response
    };

    if response.clicked() {
        // Expand the "all" shorthand before turning one off.
        if filter_groups.is_empty() {
            for g in BossGroup::ALL {
                filter_groups.insert(g.as_str());
            }
        }
        if enabled {
            filter_groups.remove(key);
        } else {
            filter_groups.insert(key);
        }
        filter_groups.remove(NO_GROUPS_KEY);
        // A full real-key set is equivalent to "all"; normalize to empty.
        if BossGroup::ALL
            .iter()
            .all(|g| filter_groups.contains(g.as_str()))
        {
            filter_groups.clear();
        } else if filter_groups.is_empty() {
            // Deselecting the last group means "show none", not "show all"
            // (empty set) -- mark it explicitly so the list stays empty.
            filter_groups.insert(NO_GROUPS_KEY);
        }
        *needs_refresh = true;
    }
}

impl CombatHistoryPanel {
    fn render_list(&mut self, ui: &mut egui::Ui, ctx: &mut PanelContext) {
        realmhound_core::prof_scope!("combat_list_render");
        let shadcn = ctx.shadcn;

        // The date-range dropdown popup paints over the cards this frame while it
        // is open; capture that before the band toggles the state.
        let date_menu_open = self.date_menu_open;

        // --- Search + filter band ---
        shadcn.header_band(ui, shadcn.secondary_header_fill(), |ui| {
            shadcn.band_row_wrapped(ui, |ui| {
                // "Select" toggle: always available. It
                // reflects whether every entry in the current (optionally
                // filtered) list is selected. Clicking when not all are selected
                // selects the whole current list; clicking when all are selected
                // clears the ENTIRE selection -- including entries hidden by a
                // filter that has since been reset, which would otherwise be
                // impossible to find and de-select.
                let all_selected = !self.filtered_selection.is_empty()
                    && self
                        .filtered_selection
                        .iter()
                        .all(|s| self.selected_for_delete.contains(s));
                let mut select_all = all_selected;
                if ui.checkbox(&mut select_all, "Select").changed() {
                    if all_selected {
                        self.selected_for_delete.clear();
                    } else {
                        let filtered = self.filtered_selection.clone();
                        self.selected_for_delete.extend(filtered);
                    }
                }
                shadcn.band_divider(ui);

                // Date-range filter (mirrors Loot History).
                ui.label("Date Range:");
                let old_date = self.filter_date;
                let mut date_key = Some(
                    match self.filter_date {
                        DateFilter::Today => "today",
                        DateFilter::ThisWeek => "week",
                        DateFilter::ThisMonth => "month",
                        DateFilter::AllTime => "all",
                        DateFilter::Custom { .. } => "custom",
                    }
                    .to_string(),
                );
                let was_menu_open = self.date_menu_open;
                let date_resp = shadcn.sel(
                    ui,
                    "combat_date_filter",
                    &mut date_key,
                    120.0,
                    &[
                        ("today", "Today"),
                        ("week", "This Week"),
                        ("month", "This Month"),
                        ("all", "All Time"),
                        ("custom", "Custom"),
                    ],
                );
                // Mirror the dropdown's open state: the trigger toggles it, and it
                // closes on option select (keyboard), Escape, or any click that is
                // not on the trigger itself (option pick / outside click).
                if date_resp.clicked() {
                    self.date_menu_open = !was_menu_open;
                } else if date_resp.changed() {
                    self.date_menu_open = false;
                } else if was_menu_open {
                    let escaped = ui.input(|i| i.key_pressed(egui::Key::Escape));
                    let clicked_away = ui.input(|i| i.pointer.any_click());
                    if escaped || clicked_away {
                        self.date_menu_open = false;
                    }
                }
                if let Some(key) = &date_key {
                    self.filter_date = match key.as_str() {
                        "today" => DateFilter::Today,
                        "week" => DateFilter::ThisWeek,
                        "month" => DateFilter::ThisMonth,
                        "all" => DateFilter::AllTime,
                        "custom" => {
                            if matches!(self.filter_date, DateFilter::Custom { .. }) {
                                self.filter_date
                            } else {
                                let today = Utc::now().date_naive();
                                DateFilter::Custom {
                                    start: today - chrono::Duration::days(30),
                                    end: today,
                                }
                            }
                        }
                        _ => DateFilter::AllTime,
                    };
                }
                if let DateFilter::Custom { mut start, mut end } = self.filter_date {
                    shadcn.date_range_picker(ui, "combat_hist_range", &mut start, &mut end);
                    self.filter_date = DateFilter::Custom { start, end };
                }
                if old_date != self.filter_date {
                    self.needs_refresh = true;
                }
                shadcn.band_divider(ui);

                // Reset the keyboard selection whenever the query text changes.
                if self.search != self.search_prev_text {
                    self.search_selected_index = None;
                    self.search_prev_text = self.search.clone();
                }

                ui.label("🔍");
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .id_source("combat_search_box")
                        .desired_width(230.0)
                        .hint_text("Search boss, dungeon or player..."),
                );
                if resp.changed() {
                    self.needs_refresh = true;
                }
                if resp.has_focus() && !self.search.trim().is_empty() {
                    self.search_popup_open = true;
                }
                let enter_pressed =
                    resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

                if !self.search.is_empty() && shadcn.btn(ui, "✕").clicked() {
                    self.search.clear();
                    self.search_popup_open = false;
                    self.search_selected_index = None;
                    self.needs_refresh = true;
                }

                // Build the suggestion list (bosses first, then dungeons) from
                // the cached distinct values, filtered by the current query.
                let search_lower = self.search.trim().to_lowercase();
                let mut results: Vec<SearchResult> = Vec::new();
                if !search_lower.is_empty() {
                    let portal_map = get_dungeon_portal_map();
                    for (object_type, name) in self
                        .autocomplete
                        .bosses
                        .iter()
                        .filter(|(_, n)| n.to_lowercase().contains(&search_lower))
                        .take(8)
                    {
                        results.push(SearchResult::Boss {
                            object_type: *object_type,
                            name: name.clone(),
                        });
                    }
                    for dungeon in self
                        .autocomplete
                        .dungeons
                        .iter()
                        .filter(|d| d.to_lowercase().contains(&search_lower))
                        .take(8)
                    {
                        let icon_id = portal_map.get_portal_id(dungeon).unwrap_or(0);
                        let display_name = portal_map.normalize_dungeon_name(dungeon);
                        results.push(SearchResult::Dungeon {
                            name: dungeon.clone(),
                            icon_id,
                            display_name,
                        });
                    }
                    for (name, icon_id, tex1, tex2) in self
                        .autocomplete
                        .players
                        .iter()
                        .filter(|(n, ..)| n.to_lowercase().contains(&search_lower))
                        .take(8)
                    {
                        results.push(SearchResult::Player {
                            name: name.clone(),
                            icon_id: *icon_id,
                            tex1: *tex1,
                            tex2: *tex2,
                        });
                    }
                    // Player-count suggestions: only when the query is all
                    // digits, offer DB counts whose decimal form starts with the
                    // typed digits (people-glyph rows).
                    if search_lower.chars().all(|c| c.is_ascii_digit()) {
                        for count in self
                            .autocomplete
                            .participant_counts
                            .iter()
                            .filter(|c| c.to_string().starts_with(&search_lower))
                            .take(8)
                        {
                            results.push(SearchResult::PlayerCount { count: *count });
                        }
                    }
                }
                let total_results = results.len();

                // Enter applies the selected (or first) suggestion.
                if enter_pressed && self.search_popup_open && total_results > 0 {
                    let idx = self.search_selected_index.unwrap_or(0);
                    if let Some(result) = results.get(idx).cloned() {
                        self.apply_search_result(result);
                    }
                }

                // Arrow-key navigation / Escape while the field is focused.
                if self.search_popup_open && resp.has_focus() && total_results > 0 {
                    let key_down = ui.input(|i| i.key_pressed(egui::Key::ArrowDown));
                    let key_up = ui.input(|i| i.key_pressed(egui::Key::ArrowUp));
                    let key_escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
                    if key_escape {
                        self.search_popup_open = false;
                        self.search_selected_index = None;
                    } else if key_down {
                        self.search_selected_index = Some(match self.search_selected_index {
                            None => 0,
                            Some(i) => (i + 1).min(total_results - 1),
                        });
                    } else if key_up {
                        self.search_selected_index = match self.search_selected_index {
                            None => None,
                            Some(0) => None,
                            Some(i) => Some(i - 1),
                        };
                    }
                }

                // Render the suggestion popup anchored under the text box.
                if self.search_popup_open && !search_lower.is_empty() {
                    let popup_id = ui.make_persistent_id("combat_search_popup");
                    let popup_pos = resp.rect.left_bottom() + egui::vec2(0.0, 2.0);
                    egui::Area::new(popup_id)
                        .fixed_pos(popup_pos)
                        .order(egui::Order::Foreground)
                        .show(ui.ctx(), |ui| {
                            egui::Frame::popup(ui.style()).show(ui, |ui| {
                                ui.set_min_width(240.0);
                                ui.set_max_height(300.0);
                                egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                                    if results.is_empty() {
                                        ui.label(
                                            RichText::new("No matches found")
                                                .italics()
                                                .color(Color32::GRAY),
                                        );
                                    } else {
                                        let mut last_category: Option<&str> = None;
                                        for (idx, result) in results.iter().enumerate() {
                                            let category = match result {
                                                SearchResult::Boss { .. } => "Bosses",
                                                SearchResult::Dungeon { .. } => "Dungeons",
                                                SearchResult::Player { .. } => "Players",
                                                SearchResult::PlayerCount { .. } => "Player count",
                                            };
                                            if last_category != Some(category) {
                                                if last_category.is_some() {
                                                    ui.add_space(4.0);
                                                }
                                                ui.label(
                                                    RichText::new(category)
                                                        .small()
                                                        .color(Color32::GRAY),
                                                );
                                                last_category = Some(category);
                                            }
                                            let is_selected =
                                                self.search_selected_index == Some(idx);
                                            let (icon_id, char_dyes, text): (
                                                i32,
                                                Option<(u32, u32)>,
                                                String,
                                            ) = match result {
                                                SearchResult::Boss { object_type, name } => {
                                                    (*object_type, None, name.clone())
                                                }
                                                SearchResult::Dungeon {
                                                    icon_id,
                                                    display_name,
                                                    ..
                                                } => (*icon_id, None, display_name.clone()),
                                                SearchResult::Player { name, icon_id, tex1, tex2 } => {
                                                    (*icon_id, Some((*tex1, *tex2)), name.clone())
                                                }
                                                SearchResult::PlayerCount { count } => {
                                                    (0, None, format!("👥 {count}"))
                                                }
                                            };
                                            let mut clicked = false;
                                            ui.horizontal(|ui| {
                                                let (rect, _) = ui.allocate_exact_size(
                                                    egui::vec2(20.0, 20.0),
                                                    egui::Sense::hover(),
                                                );
                                                if let Some((tex1, tex2)) = char_dyes {
                                                    if !ctx.sprite_renderer.draw_dyed_outlined_character_sprite(
                                                        ui, icon_id, rect, 6, tex1, tex2,
                                                    ) && icon_id > 0 {
                                                        ctx.sprite_renderer
                                                            .draw_sprite_in_rect(ui, icon_id, rect);
                                                    }
                                                } else if icon_id > 0 {
                                                    // Fit oversized boss/portal art (e.g. Ancient
                                                    // Kaiju, Oryx the Mad God 3) to the row box,
                                                    // matching the active filter pill.
                                                    ctx.sprite_renderer
                                                        .draw_outlined_sprite_in_rect_filled(ui, icon_id, rect);
                                                }
                                                let r = ui.add(
                                                    egui::Button::new(text).selected(is_selected),
                                                );
                                                if is_selected {
                                                    r.scroll_to_me(Some(egui::Align::Center));
                                                }
                                                if r.clicked() {
                                                    clicked = true;
                                                }
                                            });
                                            if clicked {
                                                self.apply_search_result(result.clone());
                                            }
                                        }
                                    }
                                });
                            });
                        });

                    // Close the popup when clicking outside the text field.
                    if ui.input(|i| i.pointer.any_click()) && !resp.has_focus() {
                        self.search_popup_open = false;
                        self.search_selected_index = None;
                    }
                }

                // --- Divider, then group filter + batch-delete controls ---
                shadcn.band_divider(ui);
                // Group filter: one sprite toggle per boss group.
                for group in BossGroup::ALL {
                    group_filter_button(
                        ui,
                        ctx,
                        group,
                        &mut self.filter_groups,
                        &mut self.tooltip_catalogs,
                        &mut self.needs_refresh,
                    );
                }
                if shadcn
                    .btn_small(ui, RichText::new("All").small())
                    .hover_tip("Show all groups")
                    .clicked()
                {
                    self.filter_groups.clear();
                    self.needs_refresh = true;
                }
                if shadcn
                    .btn_small(ui, RichText::new("None").small())
                    .hover_tip("Hide all groups")
                    .clicked()
                {
                    // A single non-existent key hides every real group without
                    // matching anything (empty would mean "all").
                    self.filter_groups.clear();
                    self.filter_groups.insert(NO_GROUPS_KEY);
                    self.needs_refresh = true;
                }

                shadcn.band_divider(ui);

                // Secret-stat toggle filters (union). Same size as All/None; the
                // border is tinted to match the character-sprite glow colour, and
                // an accent wash marks the active state.
                let stat_filter_toggle =
                    |ui: &mut egui::Ui,
                     on: bool,
                     label: &str,
                     tip: &str,
                     accent: Color32,
                     base_tint: Option<Color32>|
                     -> bool {
                        let resp = shadcn.btn_small(ui, RichText::new(label).small()).hover_tip(tip);
                        if let Some(t) = base_tint {
                            ui.painter().rect_filled(resp.rect, 3.0, t);
                        }
                        if on {
                            let wash = Color32::from_rgba_unmultiplied(
                                accent.r(),
                                accent.g(),
                                accent.b(),
                                55,
                            );
                            ui.painter().rect_filled(resp.rect, 3.0, wash);
                        }
                        ui.painter().rect_stroke(
                            resp.rect,
                            3.0,
                            egui::Stroke::new(if on { 2.0_f32 } else { 1.0_f32 }, accent),
                            egui::StrokeKind::Outside,
                        );
                        resp.clicked()
                    };
                if stat_filter_toggle(
                    ui,
                    self.filter_lone_fighter,
                    "Lone Fighter",
                    "Show only fights where your character finished a challenging boss solo",
                    crate::rendering::sprite_renderer::LONE_FIGHTER_GLOW,
                    None,
                ) {
                    self.filter_lone_fighter = !self.filter_lone_fighter;
                    self.needs_refresh = true;
                }
                if stat_filter_toggle(
                    ui,
                    self.filter_last_hero,
                    "Last Hero",
                    "Show only fights where your character was the last survivor to finish a challenging boss",
                    crate::rendering::sprite_renderer::LAST_HERO_GLOW,
                    None,
                ) {
                    self.filter_last_hero = !self.filter_last_hero;
                    self.needs_refresh = true;
                }
                if stat_filter_toggle(
                    ui,
                    self.filter_most_damage_taken,
                    "Beaten Up",
                    "Show only fights where your character took the most damage of the group during a challenging boss",
                    crate::rendering::sprite_renderer::MOST_DAMAGE_TAKEN_GLOW,
                    None,
                ) {
                    self.filter_most_damage_taken = !self.filter_most_damage_taken;
                    self.needs_refresh = true;
                }
                if stat_filter_toggle(
                    ui,
                    self.filter_close_calls,
                    "Dangerously Low",
                    "Show only fights where your character had at least one close call",
                    Color32::from_rgb(255, 0, 0),
                    Some(Color32::from_rgba_unmultiplied(255, 0, 0, 45)),
                ) {
                    self.filter_close_calls = !self.filter_close_calls;
                    self.needs_refresh = true;
                }

                shadcn.band_divider(ui);

                // Batch-delete control. Per-card checkboxes are
                // always visible; deletion acts on the current selection. The
                // filtered-delete flow is now: filter -> Select filtered ->
                // Delete selected.
                let n = self.selected_for_delete.len();
                if n > 0 && shadcn.btn(ui, format!("Delete selected ({n})")).clicked() {
                    let sels: Vec<FightSelection> =
                        self.selected_for_delete.iter().cloned().collect();
                    self.confirm = Some(ConfirmAction::DeleteSelected(sels));
                }

                if n > 0 {
                    let resp = shadcn
                        .btn(ui, format!("Export selected ({n})"))
                        .hover_tip(
                            "Save selected fights to a JSON file. The file contains account and \
                             player names for investigation.",
                        );
                    if resp.clicked() {
                        let sels: Vec<FightSelection> =
                            self.selected_for_delete.iter().cloned().collect();
                        self.export_selected(ctx, &sels);
                    }
                }

                if let Some((msg, time, is_err)) = &self.status_message {
                    if time.elapsed().as_secs() < 5 {
                        let color = if *is_err { Color32::LIGHT_RED } else { Color32::GREEN };
                        ui.label(RichText::new(msg).color(color));
                    } else {
                        self.status_message = None;
                    }
                }

                shadcn.band_divider(ui);

                // Active filter chips (icon + name pills, matching Loot History).
                let portal_map = get_dungeon_portal_map();
                if let Some(dungeon) = self.filter_dungeon.clone() {
                    let icon = portal_map.get_portal_id(&dungeon).unwrap_or(0);
                    let label = portal_map.normalize_dungeon_name(&dungeon);
                    if ctx.sprite_renderer.render_filter_pill(
                        ui,
                        icon,
                        &label,
                        Color32::from_rgb(40, 50, 70),
                    ) {
                        self.filter_dungeon = None;
                        self.needs_refresh = true;
                    }
                }
                if let Some((boss_type, name)) = self.filter_boss.clone() {
                    if ctx.sprite_renderer.render_filter_pill(
                        ui,
                        boss_type,
                        &name,
                        Color32::from_rgb(60, 40, 80),
                    ) {
                        self.filter_boss = None;
                        self.needs_refresh = true;
                    }
                }
                if let Some((char_id, name)) = self.filter_char.clone() {
                    let (icon, tex1, tex2) = char_visuals(ctx, char_id).unwrap_or((0, 0, 0));
                    if ctx.sprite_renderer.render_filter_pill_character(
                        ui,
                        icon,
                        tex1,
                        tex2,
                        &name,
                        Color32::from_rgb(40, 60, 50),
                    ) {
                        self.filter_char = None;
                        self.needs_refresh = true;
                    }
                }
                if let Some((name, icon, tex1, tex2)) = self.filter_player.clone() {
                    if ctx.sprite_renderer.render_filter_pill_character(
                        ui,
                        icon,
                        tex1,
                        tex2,
                        &name,
                        Color32::from_rgb(40, 60, 50),
                    ) {
                        self.filter_player = None;
                        self.needs_refresh = true;
                    }
                }
                if let Some(count) = self.filter_player_count {
                    if render_count_pill(ui, count) {
                        self.filter_player_count = None;
                        self.needs_refresh = true;
                    }
                }
            });
        });
        // Bottom edge of the header band (where its divider is drawn) plus the
        // normal inter-widget gap = top of the list area. Used to clip the
        // scrolling list so rows can't paint up over the band's divider.
        // NOTE: use the cursor, not `ui.min_rect()`, which for a panel spans the
        // whole panel and would collapse the clip to a thin bottom strip.
        let list_clip_top = ui.cursor().top();

        // Render the confirm dialog before the empty-state early return so a
        // pending delete still resolves once the list becomes empty.
        self.render_confirm_modal(ui, ctx);

        if self.summaries.is_empty() {
            if self.has_filters() || !matches!(self.filter_date, DateFilter::AllTime) {
                // Fights exist but the current filters hide them all.
                empty_state(
                    ui,
                    "All entries hidden by filters",
                    Some("Adjust the filter bar above to show your recorded fights."),
                );
            } else {
                empty_state_lines(
                    ui,
                    "No boss fights recorded yet",
                    &[
                        "All types of boss fights are recorded by default and will show up here",
                        "Selected boss categories can be excluded from tracking in the Settings menu",
                    ],
                );
            }
            return;
        }

        let assets = get_asset_manager();
        let mut open_fight: Option<FightSelection> = None;
        let mut set_dungeon: Option<String> = None;
        let mut set_boss: Option<(i32, String)> = None;
        let mut set_char: Option<(i32, String)> = None;
        let mut delete_fight: Option<FightSelection> = None;
        let mut toggle_select: Option<FightSelection> = None;

        // PgUp/PgDn/Home/End navigation: only when no text field (search box)
        // has focus, so typing in the search bar is unaffected. Pages by ~90% of
        // the viewport; Home/End jump to the top/bottom of the list.
        let text_focused = ui.ctx().memory(|m| m.focused()).is_some();
        let page = (ui.available_height() * 0.9).max(CARD_HEIGHT);
        // Bottom-most valid offset. Kept in range so the virtualized list below
        // never derives an out-of-bounds row index from the offset.
        let max_scroll =
            (self.summaries.len() as f32 * (CARD_HEIGHT + 4.0) - ui.available_height()).max(0.0);
        let scroll_override = if self.reset_scroll_pending {
            self.reset_scroll_pending = false;
            Some(0.0)
        } else if text_focused {
            None
        } else {
            ui.input(|i| {
                if i.key_pressed(egui::Key::Home) {
                    return Some(0.0);
                }
                if i.key_pressed(egui::Key::End) {
                    return Some(max_scroll);
                }
                let mut delta = 0.0_f32;
                if i.key_pressed(egui::Key::PageDown) {
                    delta += page;
                }
                if i.key_pressed(egui::Key::PageUp) {
                    delta -= page;
                }
                if delta != 0.0 {
                    Some((self.list_scroll_offset + delta).max(0.0))
                } else {
                    None
                }
            })
        };

        let mut area = ScrollArea::vertical()
            .id_salt("combat_history_fight_list")
            .auto_shrink([false, false]);
        if let Some(offset) = scroll_override {
            area = area.vertical_scroll_offset(offset);
        }
        // When a popup (the date-range combo dropdown or the search suggestion
        // list) is open above the cards, clicks/hovers land on the popup, not the
        // card beneath it. The combo popup is painted on a raw layer and does not
        // register an interactable area, so pair the area check with our own
        // tracked dropdown state. Frame-global, so it is hoisted out of the row
        // callback.
        let pointer_over_popup = ui.ctx().is_pointer_over_area() || date_menu_open;
        // Virtualize: fold the per-card 4px lead into the pitch passed to
        // show_rows (which adds item_spacing.y itself) so only the visible cards
        // are built while scroll math and card positions stay pixel-correct.
        let row_pitch = CARD_HEIGHT + 4.0;
        let area_out = crate::virtual_list::show(
            area,
            ui,
            row_pitch,
            self.summaries.len(),
            |ui| {
                // Clip the list so scrolled rows disappear beneath the header
                // band instead of painting over its bottom divider.
                let clipped = ui
                    .clip_rect()
                    .intersect(egui::Rect::everything_below(list_clip_top));
                ui.set_clip_rect(clipped);
            },
            |ui, fight_idx| {
                let fight = &self.summaries[fight_idx];
                ui.add_space(4.0);

                // Allocate the card background FIRST so it sits beneath the inner
                // icons in the interaction z-order; the icons (rendered into the
                // child UI afterwards) therefore win overlapping clicks, and the
                // card only receives clicks on its empty areas (-> open detail).
                let full_width = ui.available_width();
                let (card_rect, card_resp) = ui
                    .allocate_exact_size(egui::vec2(full_width, CARD_HEIGHT), egui::Sense::click());
                ui.painter()
                    .rect_filled(card_rect, 6.0, ctx.shadcn.secondary_header_fill());

                // Symmetric padding so the content is vertically centered with
                // equal top and bottom margins.
                let content_rect = egui::Rect::from_min_max(
                    card_rect.min + egui::vec2(10.0, 8.0),
                    card_rect.max - egui::vec2(10.0, 8.0),
                );
                let mut cui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(content_rect)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                let cui = &mut cui;
                // The boss/dungeon name labels sit on top of the clickable card.
                // egui selectable labels would otherwise swallow clicks over the
                // text (opening details only worked on empty areas), so disable
                // text selection inside the card body.
                cui.style_mut().interaction.selectable_labels = false;

                // Tracks whether any filterable sub-icon is hovered, so the
                // card-level "View fight details" tooltip is suppressed there
                // (the icon's own "Click to filter" tooltip takes over instead).
                let mut icon_hovered = false;

                // Multi-select checkbox, left-most (always shown).
                {
                    let sel = fight.selection();
                    let mut checked = self.selected_for_delete.contains(&sel);
                    if cui.checkbox(&mut checked, "").changed() {
                        toggle_select = Some(sel);
                    }
                    cui.add_space(4.0);
                }

                // Date/time on the far left (Loot History style: monospace).
                cui.label(
                    RichText::new(fmt_time(fight.started_at))
                        .monospace()
                        .size(12.0)
                        .color(Color32::from_gray(160)),
                );
                cui.add_space(6.0);

                // Dungeon portal icon (click -> dungeon filter).
                let portal_map = get_dungeon_portal_map();
                let portal_id = portal_map.get_portal_id(&fight.dungeon).unwrap_or(0);
                let (rect, resp) =
                    cui.allocate_exact_size(egui::vec2(32.0, 32.0), egui::Sense::click());
                if portal_id > 0 {
                    ctx.sprite_renderer
                        .draw_outlined_sprite_in_rect(cui, portal_id, rect);
                }
                if resp.hovered() {
                    icon_hovered = true;
                    cui.painter().rect_stroke(
                        rect,
                        3.0,
                        egui::Stroke::new(2.0_f32, Color32::from_rgb(100, 150, 200)),
                        egui::StrokeKind::Outside,
                    );
                    cui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                let dungeon_label = portal_map.normalize_dungeon_name(&fight.dungeon);
                if resp
                    .hover_tip(format!("{}\n(Click to filter)", dungeon_label))
                    .clicked()
                {
                    set_dungeon = Some(fight.dungeon.clone());
                }

                // Boss portrait (click -> boss filter).
                let (brect, bresp) =
                    cui.allocate_exact_size(egui::vec2(32.0, 32.0), egui::Sense::click());
                if fight.boss_object_type != 0 {
                    ctx.sprite_renderer.draw_outlined_sprite_in_rect(
                        cui,
                        fight.boss_object_type,
                        brect,
                    );
                }
                if bresp.hovered() {
                    icon_hovered = true;
                    cui.painter().rect_stroke(
                        brect,
                        3.0,
                        egui::Stroke::new(2.0_f32, Color32::from_rgb(150, 100, 200)),
                        egui::StrokeKind::Outside,
                    );
                    cui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                // On combined dungeon cards `boss_name` is the dungeon headline;
                // prefer the actual boss matching the portrait's object type.
                let boss_display = fight
                    .killed_bosses
                    .iter()
                    .find(|(t, _)| *t == fight.boss_object_type)
                    .map(|(_, n)| n.clone())
                    .unwrap_or_else(|| fight.boss_name.clone());
                if bresp
                    .hover_tip(format!("{}\n(Click to filter)", boss_display))
                    .clicked()
                    && fight.boss_object_type != 0
                {
                    set_boss = Some((fight.boss_object_type, boss_display.clone()));
                }

                // Character sprite (moved to the left, same size as the portal
                // and boss). Order: Dungeon, Main Boss, Character. Click -> char
                // filter. A fixed 32x32 slot is always reserved so text lines up
                // across cards even when the character is unknown.
                let vis =
                    char_visuals(ctx, fight.local_char_id).or_else(|| stored_char_visuals(fight));
                let ot = fight.local_object_type.unwrap_or(0);
                let (crect, cresp) =
                    cui.allocate_exact_size(egui::vec2(32.0, 32.0), egui::Sense::click());
                if let Some((icon, t1, t2)) = vis {
                    let overlay = (fight.local_close_calls > 0)
                        .then(|| Color32::from_rgba_unmultiplied(255, 0, 0, 128));
                    // Secret-stat halo precedence: gold "Lone fighter" > purple
                    // "Last hero standing" > red "Most damage taken".
                    let glow = if fight.lone_fighter {
                        Some(crate::rendering::sprite_renderer::LONE_FIGHTER_GLOW)
                    } else if fight.last_hero_standing {
                        Some(crate::rendering::sprite_renderer::LAST_HERO_GLOW)
                    } else if fight.most_damage_taken {
                        Some(crate::rendering::sprite_renderer::MOST_DAMAGE_TAKEN_GLOW)
                    } else {
                        None
                    };
                    if let Some(glow_color) = glow {
                        ctx.sprite_renderer
                            .draw_dyed_outlined_character_sprite_glow(
                                cui, icon, crect, 8, t1, t2, glow_color, 7, overlay,
                            );
                    } else if let Some(ov) = overlay {
                        // Close-call sprite (no secret-stat halo): red contour to
                        // match the red overlay.
                        ctx.sprite_renderer
                            .draw_dyed_outlined_character_sprite_colored_outline(
                                cui,
                                icon,
                                crect,
                                8,
                                t1,
                                t2,
                                Some(ov),
                                Color32::from_rgb(255, 0, 0),
                            );
                    } else {
                        ctx.sprite_renderer
                            .draw_dyed_outlined_character_sprite_overlay(
                                cui, icon, crect, 8, t1, t2, None,
                            );
                    }
                } else if ot != 0 {
                    ctx.sprite_renderer
                        .draw_outlined_sprite_in_rect(cui, ot, crect);
                }
                if fight.local_char_id != 0 && (vis.is_some() || ot != 0) {
                    if cresp.hovered() {
                        icon_hovered = true;
                        cui.painter().rect_stroke(
                            crect,
                            3.0,
                            egui::Stroke::new(2.0_f32, Color32::from_rgb(100, 200, 150)),
                            egui::StrokeKind::Outside,
                        );
                        cui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    let base_icon = vis.map(|(i, _, _)| i).unwrap_or(ot);
                    let cname = ctx
                        .labels
                        .get(&fight.local_char_id)
                        .cloned()
                        .or_else(|| assets.object_name(base_icon))
                        .unwrap_or_else(|| "Character".to_string());
                    if cresp
                        .hover_tip(format!("{}\n(Click to filter)", cname))
                        .clicked()
                    {
                        set_char = Some((fight.local_char_id, cname));
                    }
                }

                cui.add_space(6.0);

                // Participant count in a fixed-width slot (fits up to 3 digits)
                // so the text block starts at the same x on every card. The
                // icon+number are centered within the slot.
                cui.allocate_ui_with_layout(
                    egui::vec2(48.0, 32.0),
                    egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                    |ui| {
                        let count_color = if fight.flawless {
                            Color32::from_gray(110)
                        } else {
                            Color32::from_gray(200)
                        };
                        ui.label(
                            RichText::new(format!("👥 {}", fight.participant_count))
                                .size(16.0)
                                .strong()
                                .color(count_color),
                        );
                    },
                );

                cui.add_space(4.0);

                // Right-aligned meta: only the delete button now. Reserve its slot
                // (plus a right margin so the scrollbar never overlaps it) before
                // the text so the text fills the middle and never collides with it.
                cui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(12.0);
                    let (drect, dresp) =
                        ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
                    let del_hovered = dresp.hovered();
                    let del_color = if del_hovered {
                        Color32::from_rgb(220, 90, 90)
                    } else {
                        Color32::from_rgb(130, 130, 130)
                    };
                    ui.painter().text(
                        drect.center(),
                        egui::Align2::CENTER_CENTER,
                        "🗑",
                        egui::FontId::proportional(14.0),
                        del_color,
                    );
                    if del_hovered {
                        icon_hovered = true;
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if dresp.hover_tip("Delete this entry").clicked() {
                        delete_fight = Some(fight.selection());
                    }

                    // Text block fills the remaining width to the left of delete.
                    ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        // Line 1: dungeon • active time • Completed/Escaped.
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.label(
                                RichText::new(&dungeon_label)
                                    .size(17.0)
                                    .strong()
                                    .color(Color32::WHITE),
                            );
                            let (status_text, status_color) = if fight.killed {
                                ("Completed", Color32::from_rgb(120, 200, 120))
                            } else if fight.local_died {
                                ("Died", Color32::from_rgb(220, 90, 90))
                            } else {
                                ("Escaped", Color32::from_rgb(210, 150, 90))
                            };
                            ui.label(
                                RichText::new(format!("•  {}", fmt_duration(fight.duration_ms())))
                                    .size(13.0)
                                    .color(Color32::GRAY),
                            );
                            ui.label(RichText::new("•").size(13.0).color(Color32::GRAY));
                            ui.label(RichText::new(status_text).size(13.0).color(status_color));
                            if let Some(drops) = self.loot_links.get(&fight.selection()) {
                                if let Some(best_drop) = drops.iter().max_by_key(|drop| {
                                    drop.bag_type_enum()
                                        .map(|bag| bag.rarity_rank())
                                        .unwrap_or_default()
                                }) {
                                    ui.label(RichText::new("•").size(13.0).color(Color32::GRAY));
                                    let (rect, response) = ui.allocate_exact_size(
                                        egui::vec2(16.0, 16.0),
                                        egui::Sense::hover(),
                                    );
                                    ctx.sprite_renderer.draw_outlined_sprite_in_rect(
                                        ui,
                                        best_drop.bag_type,
                                        rect,
                                    );
                                    response
                                        .hover_tip(format!("{} loot bag(s) dropped", drops.len()));
                                }
                            }
                        });
                        // Line 2: killed boss names, comma-separated, no icons.
                        // When the Treasure crates filter is off, crate members
                        // are hidden from the card, so drop them from the line too.
                        let hide_crates = !self.filter_groups.is_empty()
                            && !self
                                .filter_groups
                                .contains(BossGroup::TreasureCrate.as_str());
                        let am = get_asset_manager();
                        let names: Vec<&str> = fight
                            .killed_bosses
                            .iter()
                            .filter(|(t, _)| !(hide_crates && am.is_treasure_crate(*t)))
                            .map(|(_, n)| n.as_str())
                            .collect();
                        let boss_line = if names.is_empty() {
                            fight.boss_name.clone()
                        } else {
                            names.join(", ")
                        };
                        ui.label(
                            RichText::new(boss_line)
                                .color(Color32::from_gray(175))
                                .size(12.5),
                        );
                    });
                });

                // Highlight the whole card border on hover with a details hint.
                if ui.rect_contains_pointer(card_rect) && !pointer_over_popup {
                    ui.painter().rect_stroke(
                        card_rect,
                        6.0,
                        egui::Stroke::new(1.5_f32, Color32::from_rgb(90, 130, 170)),
                        egui::StrokeKind::Inside,
                    );
                    // The card (and its text labels) are clickable to open details,
                    // so force the pointing-hand cursor over the whole card. This
                    // overrides the I-beam that selectable labels would otherwise
                    // show over the boss name, and the default arrow on empty areas.
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }

                // Suppress the card-level details tooltip while a sub-icon is
                // hovered so it doesn't overlap the icon's own filter tooltip.
                let card_resp = if icon_hovered {
                    card_resp
                } else {
                    card_resp.hover_tip("View fight details")
                };
                if card_resp.clicked() && !pointer_over_popup {
                    open_fight = Some(fight.selection());
                }
            },
        );
        self.list_scroll_offset = area_out.state.offset.y;

        // Load-more on demand: when the user scrolls within a couple of cards of
        // the bottom and more rows may exist, grow the window by a page and
        // reload next frame. `!needs_refresh` avoids re-triggering while a grow
        // reload is already pending.
        if self.has_more && !self.needs_refresh {
            let viewport = area_out.inner_rect.height();
            let remaining = area_out.content_size.y - (self.list_scroll_offset + viewport);
            if remaining <= CARD_HEIGHT * 2.0 {
                self.loaded_limit += PAGE_SIZE;
                self.needs_refresh = true;
                ui.ctx().request_repaint();
            }
        }

        if let Some(sel) = open_fight {
            self.selected = Some(sel);
            self.detail = None;
            self.encounter = None;
            self.expanded_phases.clear();
        }
        if let Some(d) = set_dungeon {
            self.filter_dungeon = Some(d);
            self.needs_refresh = true;
        }
        if let Some(b) = set_boss {
            self.filter_boss = Some(b);
            self.needs_refresh = true;
        }
        if let Some(c) = set_char {
            self.filter_char = Some(c);
            self.needs_refresh = true;
        }
        if let Some(sel) = delete_fight {
            // The UI holds a read-only connection; open a short-lived writer to
            // remove just this entry, then refresh the list from the reader.
            match CombatDatabase::open_writer(&self.combat_db_path) {
                Ok(mut writer) => match writer.delete_selection(&sel) {
                    Ok(()) => {
                        if self.selected.as_ref() == Some(&sel) {
                            self.selected = None;
                            self.detail = None;
                            self.encounter = None;
                        }
                        self.selected_for_delete.remove(&sel);
                        if let Some(db) = ctx.combat_database {
                            if self.reload(db, ctx.loot_database) {
                                if let Ok(count) = db.fight_count() {
                                    self.last_count = count;
                                }
                            } else {
                                // Retry next tick so the deleted row does not linger.
                                self.needs_refresh = true;
                            }
                        }
                        self.autocomplete.loaded = false;
                    }
                    Err(e) => tracing::warn!("[COMBAT] delete_selection failed: {}", e),
                },
                Err(e) => tracing::warn!("[COMBAT] open writer for delete failed: {}", e),
            }
        }

        if let Some(sel) = toggle_select {
            if !self.selected_for_delete.remove(&sel) {
                self.selected_for_delete.insert(sel);
            }
        }
    }

    /// Loads the given selections in full, then writes them to a user-chosen
    /// JSON file. Selections are left intact so the user can export again or
    /// clean up manually.
    fn export_selected(&mut self, ctx: &mut PanelContext, selections: &[FightSelection]) {
        let Some(db) = ctx.combat_database else {
            self.status_message = Some((
                "Export failed: database unavailable".to_string(),
                Instant::now(),
                true,
            ));
            return;
        };

        let mut fights: Vec<FightRecord> = Vec::new();
        let mut encounters: Vec<EncounterRecord> = Vec::new();
        let mut failed = 0usize;
        for sel in selections {
            match sel {
                FightSelection::Single(id) => match db.fight_detail(*id) {
                    Ok(Some(rec)) => fights.push(rec),
                    Ok(None) => {
                        failed += 1;
                        tracing::warn!("[COMBAT] export: fight {id} not found");
                    }
                    Err(e) => {
                        failed += 1;
                        tracing::warn!("[COMBAT] export: load fight {id} failed: {e}");
                    }
                },
                FightSelection::Encounter(run_id) => match db.encounter_detail(run_id) {
                    Ok(Some(rec)) => encounters.push(rec),
                    Ok(None) => {
                        failed += 1;
                        tracing::warn!("[COMBAT] export: encounter {run_id} not found");
                    }
                    Err(e) => {
                        failed += 1;
                        tracing::warn!("[COMBAT] export: load encounter {run_id} failed: {e}");
                    }
                },
            }
        }

        if fights.is_empty() && encounters.is_empty() {
            self.status_message = Some((
                "Export failed: no records could be loaded".to_string(),
                Instant::now(),
                true,
            ));
            return;
        }

        let bundle = build_bundle(crate::app::VERSION, &fights, &encounters);
        let json = match serde_json::to_string_pretty(&bundle) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!("[COMBAT] export: serialize failed: {e}");
                self.status_message = Some((
                    "Export failed: could not serialize".to_string(),
                    Instant::now(),
                    true,
                ));
                return;
            }
        };

        let default_name = format!(
            "combat-export-{}.json",
            Local::now().format("%Y%m%d-%H%M%S")
        );
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(&default_name)
            .add_filter("JSON", &["json"])
            .save_file()
        else {
            return;
        };

        match std::fs::write(&path, json) {
            Ok(()) => {
                let total = fights.len() + encounters.len();
                tracing::info!(
                    "[COMBAT] exported {} fight(s) + {} encounter(s) to {}",
                    fights.len(),
                    encounters.len(),
                    path.display()
                );
                let noun = if total == 1 { "entry" } else { "entries" };
                let msg = if failed > 0 {
                    format!("Exported {total} {noun}; {failed} could not be loaded")
                } else {
                    format!("Exported {total} {noun}")
                };
                self.status_message = Some((msg, Instant::now(), failed > 0));
            }
            Err(e) => {
                tracing::warn!("[COMBAT] export: write {} failed: {e}", path.display());
                self.status_message = Some((format!("Export failed: {e}"), Instant::now(), true));
            }
        }
    }

    /// Renders the destructive-action confirmation dialog and, on confirm,
    /// performs the batch delete via a short-lived writer connection.
    fn render_confirm_modal(&mut self, ui: &mut egui::Ui, ctx: &mut PanelContext) {
        let Some(action) = self.confirm.clone() else {
            return;
        };
        let shadcn = ctx.shadcn;

        let (title, body) = match &action {
            ConfirmAction::DeleteSelected(sels) => (
                "Delete selected entries",
                format!("Permanently delete {} selected fight(s)?", sels.len()),
            ),
        };

        let mut do_confirm = false;
        let mut do_cancel = false;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ui.ctx(), |ui| {
                ui.set_max_width(360.0);
                ui.label(body);
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if shadcn.btn(ui, "Cancel").clicked() {
                        do_cancel = true;
                    }
                    if shadcn.button_destructive(ui, "Delete", true).clicked() {
                        do_confirm = true;
                    }
                });
            });

        if do_cancel {
            self.confirm = None;
            return;
        }
        if !do_confirm {
            return;
        }
        self.confirm = None;

        let mut writer = match CombatDatabase::open_writer(&self.combat_db_path) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!("[COMBAT] open writer for batch delete failed: {}", e);
                return;
            }
        };

        let result = match action {
            ConfirmAction::DeleteSelected(sels) => writer.delete_selections(&sels).map(|_| ()),
        };

        match result {
            Ok(()) => {
                self.selected = None;
                self.detail = None;
                self.encounter = None;
                self.selected_for_delete.clear();
                if let Some(db) = ctx.combat_database {
                    if self.reload(db, ctx.loot_database) {
                        if let Ok(count) = db.fight_count() {
                            self.last_count = count;
                        }
                    } else {
                        // Reload failed this frame; force a retry next tick so
                        // deleted rows don't linger on screen.
                        self.needs_refresh = true;
                    }
                }
                self.autocomplete.loaded = false;
            }
            Err(e) => tracing::warn!("[COMBAT] batch delete failed: {}", e),
        }
    }

    fn render_detail(&mut self, ui: &mut egui::Ui, ctx: &mut PanelContext, fight_id: i64) {
        // Hydrate the detail record on demand.
        if self.detail.as_ref().map(|f| f.id) != Some(fight_id) {
            if let Some(db) = ctx.combat_database {
                self.detail = db.fight_detail(fight_id).ok().flatten();
            }
        }

        let shadcn = ctx.shadcn;
        let (has_prev, has_next) = self.selection_neighbours();
        let mut go_back = false;
        let mut go_prev = false;
        let mut go_next = false;
        shadcn.header_band(ui, shadcn.secondary_header_fill(), |ui| {
            shadcn.band_row(ui, |ui| {
                if shadcn.btn(ui, "← Back").clicked() {
                    go_back = true;
                }
                if ui
                    .add_enabled(has_prev, egui::Button::new("<< Previous"))
                    .clicked()
                {
                    go_prev = true;
                }
                if ui
                    .add_enabled(has_next, egui::Button::new("Next >>"))
                    .clicked()
                {
                    go_next = true;
                }
                ui.checkbox(&mut self.show_dmg, "DMG");
                ui.checkbox(&mut self.show_hits, "Hits landed");
                ui.checkbox(&mut self.show_damage_taken, "Damage taken");
                ui.checkbox(&mut self.show_damage_blocked, "Damage blocked");
            });
        });
        if go_back {
            self.selected = None;
            self.detail = None;
            self.encounter = None;
            return;
        }
        if go_prev {
            self.navigate_selection(-1);
            return;
        }
        if go_next {
            self.navigate_selection(1);
            return;
        }

        let Some(fight) = self.detail.clone() else {
            empty_state(ui, "Fight unavailable", None);
            return;
        };

        let mut player_click: Option<(String, i32, u32, u32)> = None;
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(6.0);
                // --- Header: stylized boss HP bar + meta ---
                let portal_map = get_dungeon_portal_map();
                let portal_id = portal_map.get_portal_id(&fight.dungeon).unwrap_or(0);

                let bar_inset = self.draw_boss_hp_bar(
                    ui,
                    ctx.sprite_renderer,
                    portal_id,
                    fight.boss_object_type,
                    fight.boss_start_hp as i64,
                    fight.boss_max_hp as i64,
                );

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_space(bar_inset);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(RichText::new(&fight.boss_name).size(18.0).strong());

                        // location · date time (no dot between date and time so the
                        // start time doesn't read as a duration).
                        let datetime_s = match Local.timestamp_millis_opt(fight.started_at).single()
                        {
                            Some(dt) => dt.format("%Y-%m-%d %H:%M").to_string(),
                            None => "-".to_string(),
                        };
                        ui.label(
                            RichText::new(format!(
                                "{}  ·  {}",
                                portal_map.normalize_dungeon_name(&fight.dungeon),
                                datetime_s,
                            ))
                            .color(Color32::GRAY),
                        );
                        ui.label(
                            RichText::new(format!(
                                "Duration: {}",
                                fmt_duration(fight.duration_ms())
                            ))
                            .color(Color32::GRAY),
                        );
                        if fight.local_close_calls > 0 {
                            // After death the live cache drops the character, so fall
                            // back to the local participant's stored appearance so the
                            // icon keeps its skin/dyes instead of disappearing.
                            let cc_vis = char_visuals(ctx, fight.local_char_id).or_else(|| {
                                fight
                                    .participants
                                    .iter()
                                    .find(|p| p.is_local)
                                    .and_then(|p| {
                                        let icon_id = if p.skin_id > 0 {
                                            p.skin_id
                                        } else {
                                            p.object_type
                                        };
                                        (icon_id != 0).then_some((icon_id, p.tex1, p.tex2))
                                    })
                            });
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                ui.label(
                                    RichText::new(format!(
                                        "Close calls: {}",
                                        fight.local_close_calls
                                    ))
                                    .color(Color32::GRAY),
                                );
                                if let Some((icon, t1, t2)) = cc_vis {
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(20.0, 20.0),
                                        egui::Sense::hover(),
                                    );
                                    ctx.sprite_renderer
                                        .draw_dyed_outlined_character_sprite_colored_outline(
                                            ui,
                                            icon,
                                            rect,
                                            8,
                                            t1,
                                            t2,
                                            Some(Color32::from_rgba_unmultiplied(255, 0, 0, 128)),
                                            Color32::from_rgb(255, 0, 0),
                                        );
                                }
                            })
                            .response
                            .hover_tip(
                                "Times you dropped to 20% HP or below during this fight \
                             (deaths included).",
                            );
                        }
                        // Participants: finished / total. "Finished" means the player
                        // was still present when the fight ended (did not nexus, move
                        // out of range, or die).
                        let finished = fight
                            .participants
                            .iter()
                            .filter(|p| matches!(p.end_status, ParticipantEndStatus::Present))
                            .count();
                        let total = fight.participants.len();
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            ui.label(RichText::new("Participants:").color(Color32::GRAY).small());
                            ui.label(
                                RichText::new(format!("{}", finished))
                                    .color(Color32::GRAY)
                                    .small(),
                            )
                            .hover_tip(
                                "Players who finished the fight (were still present \
                             when it ended).",
                            );
                            ui.label(RichText::new("/").color(Color32::GRAY).small());
                            ui.label(
                                RichText::new(format!("{}", total))
                                    .color(Color32::GRAY)
                                    .small(),
                            )
                            .hover_tip("Players who participated in the fight.");
                        });
                    });
                });

                ui.add_space(8.0);
                shadcn.full_width_separator(ui);
                ui.add_space(4.0);

                let loot_x = self.render_participant_table(
                    ui,
                    ctx,
                    &fight.participants,
                    "combat_detail_table",
                    fight.boss_start_hp as i64,
                    fight.killed,
                    !fight.killed,
                    fight.boss_object_type == O3_BOSS_TYPE,
                    &mut player_click,
                );
                self.render_boss_loot(ui, ctx, &FightSelection::Single(fight_id), None, loot_x);
            });
        if let Some((name, icon, tex1, tex2)) = player_click {
            self.apply_player_click(name, icon, tex1, tex2);
        }
    }

    /// Render the loot dropped by one boss as compact rows (bag sprite + item
    /// tiles only), placed right after that boss's damage table. When
    /// `filter_type` is `Some`, only drops from that mob type are shown so each
    /// encounter phase surfaces just its own bags; `None` shows every drop
    /// linked to the selection (used for single-boss fights). `label_x` is the
    /// screen-x of the table's "Total" column so the "Loot:" label lines up
    /// directly beneath it.
    fn render_boss_loot(
        &self,
        ui: &mut egui::Ui,
        ctx: &mut PanelContext,
        selection: &FightSelection,
        filter_type: Option<i32>,
        label_x: f32,
    ) {
        let Some(drops) = self.loot_links.get(selection) else {
            return;
        };
        let matching: Vec<&LootDropRecord> = drops
            .iter()
            .filter(|d| {
                filter_type.map_or(true, |t| {
                    d.mob_type == t
                        || realmhound_core::assets::loot_emitter_for_boss(t) == Some(d.mob_type)
                })
            })
            .collect();
        if matching.is_empty() {
            return;
        }
        // Merge same-tier bags from this boss: two identical bags are just an
        // 8-slot overflow of one drop, so show a single bag sprite with all its
        // items. Distinct bag tiers stay as separate bags.
        let mut bags: Vec<(i32, Vec<&LootItemRecord>)> = Vec::new();
        for drop in &matching {
            match bags
                .iter_mut()
                .find(|(bag_type, _)| *bag_type == drop.bag_type)
            {
                Some((_, items)) => items.extend(drop.items.iter()),
                None => bags.push((drop.bag_type, drop.items.iter().collect())),
            }
        }

        // Moonlight Village's Challenge Gate drops two independent bags of
        // different tier at dungeon end (both attributed to the last boss).
        // Show each on its own "Loot:" row rather than side by side, since they
        // are separate drops, not one multi-bag drop.
        let one_row_per_bag = matching
            .iter()
            .any(|d| d.dungeon == realmhound_core::loot::MOONLIGHT_VILLAGE_NAME);

        ui.add_space(2.0);
        if one_row_per_bag {
            for bag in &bags {
                self.render_loot_row(ui, ctx, label_x, std::slice::from_ref(bag));
            }
        } else {
            self.render_loot_row(ui, ctx, label_x, &bags);
        }
        ui.add_space(2.0);
    }

    /// Render one "Loot:" row: the label aligned under the table's Total column
    /// followed by each bag's sprite and item tiles.
    fn render_loot_row(
        &self,
        ui: &mut egui::Ui,
        ctx: &mut PanelContext,
        label_x: f32,
        bags: &[(i32, Vec<&LootItemRecord>)],
    ) {
        ui.horizontal(|ui| {
            let cur = ui.cursor().left();
            if label_x > cur {
                ui.add_space(label_x - cur);
            }
            // "Loot:" in the same font/size as the table's "Total" row.
            ui.label(RichText::new("Loot:").strong());
            ui.add_space(4.0);
            for (i, (bag_type, items)) in bags.iter().enumerate() {
                if i > 0 {
                    ui.add_space(10.0);
                }
                crate::panels::loot::LootPanel::render_compact_bag(
                    ui,
                    *bag_type,
                    items,
                    ctx.sprite_renderer,
                );
            }
        });
    }

    /// Draw the stylized boss HP bar: the location portal on the
    /// left, a fixed-width rounded dark bar filled to the boss's discovered-HP
    /// fraction (green/orange/red by threshold), the HP numbers overlaid, and a
    /// yellow diamond + boss portrait overlapping the bar's right end. Returns
    /// the x offset (from the left edge) where the bar starts, so the meta text
    /// below can be aligned with it.
    fn draw_boss_hp_bar(
        &self,
        ui: &mut egui::Ui,
        sprite_renderer: &mut crate::rendering::SpriteRenderer,
        portal_id: i32,
        boss_type: i32,
        start_hp: i64,
        max_hp: i64,
    ) -> f32 {
        const YELLOW: Color32 = Color32::from_rgb(0xff, 0xc1, 0x00);
        const DARK_YELLOW: Color32 = Color32::from_rgb(0xab, 0x83, 0x00);
        const GREEN: Color32 = Color32::from_rgb(0x7b, 0xbf, 0x34);
        const ORANGE: Color32 = Color32::from_rgb(0xde, 0x73, 0x00);
        const RED: Color32 = Color32::from_rgb(0xc0, 0x38, 0x34);
        const DARK_BG: Color32 = Color32::from_rgb(0x0b, 0x0a, 0x10);

        const PORTAL: f32 = 40.0;
        const GAP: f32 = 10.0;
        const BAR_WIDTH: f32 = 360.0;
        const BAR_HEIGHT: f32 = 34.0;
        const DIAMOND: f32 = 46.0;
        const PORTRAIT: f32 = 34.0;
        const ROUNDING: f32 = 6.0;

        // The diamond overhangs the bar's right end, so reserve half of it.
        let bar_inset = PORTAL + GAP;
        let total_w = bar_inset + BAR_WIDTH + DIAMOND * 0.5;
        let row_h = DIAMOND;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(total_w, row_h), egui::Sense::hover());
        let painter = ui.painter().clone();

        // Location portal on the far left, vertically centered.
        if portal_id != 0 {
            let p_rect = egui::Rect::from_center_size(
                egui::pos2(rect.left() + PORTAL * 0.5, rect.center().y),
                egui::vec2(PORTAL, PORTAL),
            );
            sprite_renderer.draw_outlined_sprite_in_rect(ui, portal_id, p_rect);
        }

        // Fixed-width bar, vertically centered in the row.
        let bar_left = rect.left() + bar_inset;
        let bar_rect = egui::Rect::from_min_size(
            egui::pos2(bar_left, rect.center().y - BAR_HEIGHT * 0.5),
            egui::vec2(BAR_WIDTH, BAR_HEIGHT),
        );

        // Dark background for the whole bar.
        painter.rect_filled(bar_rect, ROUNDING, DARK_BG);

        // Colored fill proportional to discovered/max HP.
        let frac = if max_hp > 0 {
            (start_hp as f32 / max_hp as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        if frac > 0.0 {
            let fill_color = if frac >= 0.5 {
                GREEN
            } else if frac >= 0.25 {
                ORANGE
            } else {
                RED
            };
            let fill_rect = egui::Rect::from_min_max(
                bar_rect.min,
                egui::pos2(bar_rect.left() + bar_rect.width() * frac, bar_rect.bottom()),
            );
            painter.rect_filled(fill_rect, ROUNDING, fill_color);
        }

        // Yellow outline around the bar.
        painter.rect_stroke(
            bar_rect,
            ROUNDING,
            egui::Stroke::new(3.0_f32, YELLOW),
            egui::StrokeKind::Inside,
        );

        // HP numbers, centered in the bar area left of the diamond: yellow text
        // with a 1px black outline.
        if max_hp > 0 {
            let text = format!("{} / {}", fmt_thousands(start_hp), fmt_thousands(max_hp));
            let font = egui::FontId::proportional(15.0);
            let text_cx = (bar_rect.left() + (bar_rect.right() - DIAMOND * 0.5)) * 0.5;
            let center = egui::pos2(text_cx, bar_rect.center().y);
            for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
                painter.text(
                    center + egui::vec2(dx, dy),
                    egui::Align2::CENTER_CENTER,
                    &text,
                    font.clone(),
                    Color32::BLACK,
                );
            }
            painter.text(center, egui::Align2::CENTER_CENTER, &text, font, YELLOW);
        }

        // Yellow diamond + boss portrait overlapping the bar's right end, drawn
        // last so they sit on top of the bar.
        let dc = egui::pos2(bar_rect.right(), bar_rect.center().y);
        let h = DIAMOND * 0.5;
        let diamond = vec![
            dc + egui::vec2(0.0, -h),
            dc + egui::vec2(h, 0.0),
            dc + egui::vec2(0.0, h),
            dc + egui::vec2(-h, 0.0),
        ];
        painter.add(egui::Shape::convex_polygon(
            diamond,
            DARK_YELLOW,
            egui::Stroke::new(3.0_f32, YELLOW),
        ));
        if boss_type != 0 {
            let p_rect = egui::Rect::from_center_size(dc, egui::vec2(PORTRAIT, PORTRAIT));
            sprite_renderer.draw_outlined_sprite_in_rect(ui, boss_type, p_rect);
        }

        // Tooltip scoped to the bar itself.
        let bar_resp = ui.interact(
            bar_rect,
            ui.id().with("boss_hp_bar_tooltip"),
            egui::Sense::hover(),
        );
        bar_resp.hover_tip(
            "Boss's HP at the start of the fight / Boss's max HP registered during the fight",
        );

        bar_inset
    }

    /// Render the participant DPS table for a set of rows.
    /// `salt` disambiguates the egui Grid id when several tables share a view.
    fn render_participant_table(
        &self,
        ui: &mut egui::Ui,
        ctx: &mut PanelContext,
        participants: &[ParticipantRecord],
        salt: &str,
        boss_hp: i64,
        killed: bool,
        local_escaped: bool,
        show_guarded: bool,
        player_click: &mut Option<(String, i32, u32, u32)>,
    ) -> f32 {
        let attributed: i64 = participants.iter().map(|p| p.damage).sum();
        // On a kill the core capped every participant so their damage sums to
        // the boss HP that was removed; the residual is damage we saw the boss
        // lose but couldn't attribute to an observed player. Reconciling lets
        // the table's Total row equal the boss HP (proving nothing is "lowered",
        // just overkill excluded). Off a kill we keep the plain tracked total.
        let reconcile = killed && boss_hp > 0;
        let unattributed = if reconcile {
            (boss_hp - attributed).max(0)
        } else {
            0
        };
        let grand_total = attributed + unattributed;
        let total_hits: i64 = participants.iter().map(|p| p.hits).sum();
        let total_taken: Option<i64> = {
            let vals: Vec<i64> = participants.iter().filter_map(|p| p.damage_taken).collect();
            if vals.is_empty() {
                None
            } else {
                Some(vals.iter().sum())
            }
        };
        let total_guarded: Option<i64> = if show_guarded {
            let vals: Vec<i64> = participants
                .iter()
                .filter_map(|p| p.guarded_damage)
                .collect();
            if vals.is_empty() {
                None
            } else {
                Some(vals.iter().sum())
            }
        } else {
            None
        };
        let total_blocked: Option<i64> = if self.show_damage_blocked {
            let vals: Vec<i64> = participants
                .iter()
                .filter_map(|p| p.damage_blocked)
                .collect();
            if vals.is_empty() {
                None
            } else {
                Some(vals.iter().sum())
            }
        } else {
            None
        };
        // Denominator for the "%" column: boss HP on a kill, else tracked total.
        let pct_base = if reconcile { grand_total } else { attributed };
        let denom = pct_base.max(1);

        let mut col1_x: f32 = ui.min_rect().left();
        egui::Grid::new(salt)
            .num_columns(8 + show_guarded as usize + self.show_damage_blocked as usize)
            .striped(true)
            .spacing(egui::vec2(6.0, 6.0))
            .show(ui, |ui| {
                ui.label(RichText::new("#").strong());
                col1_x = ui.label(RichText::new("Teammates").strong()).rect.left();
                if self.show_dmg {
                    ui.label(RichText::new("Raw DMG").strong());
                    let (pct_header, pct_hover) = if reconcile {
                        (
                            "% of boss HP",
                            "Share of the boss's starting HP. Players plus \
                             Unattributed sum to 100% because damage is capped \
                             at boss HP (overkill excluded).",
                        )
                    } else {
                        (
                            "% of tracked dmg",
                            "Share of damage tracked by RealmHound, not absolute \
                             raid DPS (visibility gaps exist).",
                        )
                    };
                    ui.label(RichText::new(pct_header).strong())
                        .hover_tip(pct_hover);
                }
                if self.show_hits {
                    ui.label(RichText::new("Hits landed").strong()).hover_tip(
                        "Number of damaging hits this player landed on the \
                             boss/enemies (offensive hits dealt, not damage taken).",
                    );
                }
                if self.show_damage_taken {
                    ui.label(RichText::new("Damage taken").strong()).hover_tip(
                        "Damage this player took from the boss/enemies during \
                             the fight. Exact for other visible players; estimated \
                             for you (the game never sends your own damage taken). \
                             \"-\" means no data was captured.",
                    );
                }
                if self.show_damage_blocked {
                    ui.label(RichText::new("Damage blocked").strong())
                        .hover_tip(
                            "Damage negated by your defense and other post-defense \
                             mitigation (armor Damage Resistance enchants, Armor of \
                             Nil, Knight shields, Armored, Invulnerable). Only \
                             reconstructable for you; other players show \"-\".",
                        );
                }
                if show_guarded {
                    ui.label(RichText::new("Guarded").strong()).hover_tip(
                        "Damage dealt to Oryx 3 while he was Guarded (shield \
                             raised). If excessive, it triggers his Silence \
                             counter. It is also included in \
                             Raw DMG. \"-\" means the fight predates this tracking.",
                    );
                }
                ui.label(RichText::new("Gear").strong()).hover_tip(
                    "Only shows the items that contributed the most to the \
                         player's DPS.",
                );
                let accuracy_header = ui
                    .label(RichText::new("Tracking accuracy").strong())
                    .hover_tip(
                        "How this client recorded the damage value \
                         (Observed / Self-computed), not player hit accuracy.",
                    );
                ui.end_row();

                // Right edge the striped rows extend to (the widest column, the
                // "Tracking accuracy" header). Local-player highlight bands are
                // stretched to this so they end where every other row ends
                // instead of at their own (narrower) last cell.
                let mut table_right = accuracy_header.rect.right();
                let mut local_bands: Vec<(egui::layers::ShapeIdx, egui::Rect)> = Vec::new();

                for (i, p) in participants.iter().enumerate() {
                    // Reserve a background shape up front so the local-player
                    // row highlight paints beneath the cell content (but above
                    // the grid's striping). Filled in after the row is laid out.
                    let bg_idx = p.is_local.then(|| ui.painter().add(egui::Shape::Noop));
                    let mut row_rect: Option<egui::Rect> = None;
                    let track = |rect: egui::Rect, row_rect: &mut Option<egui::Rect>| {
                        *row_rect = Some(match *row_rect {
                            Some(r) => r.union(rect),
                            None => rect,
                        });
                    };

                    // Players who left the fight (died, or nexused / went out of
                    // range) have their row text dimmed so the finished (present)
                    // participants stand out at a glance.
                    let dim = (!matches!(p.end_status, ParticipantEndStatus::Present))
                        .then_some(Color32::from_gray(140));
                    let dimmed = |s: String| match dim {
                        Some(c) => RichText::new(s).color(c),
                        None => RichText::new(s),
                    };

                    // Row number and the death / nexus status icon packed into a
                    // single tight cell so the icon sits close to both the number
                    // and the player, with small equal margins. The number is
                    // right-aligned in a fixed slot so the icons line up in a
                    // column across rows regardless of 1- vs 2-digit numbers.
                    let numstat = ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.allocate_ui_with_layout(
                            egui::vec2(16.0, 20.0),
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                ui.label(dimmed(format!("{}", i + 1)));
                            },
                        );
                        let (rect, resp) =
                            ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                        match p.end_status {
                            ParticipantEndStatus::Died { grave_type } => {
                                let grave = if grave_type > 0 { grave_type } else { 1829 };
                                ctx.sprite_renderer.draw_sprite_in_rect(ui, grave, rect);
                                let tip = match get_asset_manager().grave_tier(grave_type) {
                                    Some(t) => match t.fame {
                                        Some(fame) => format!(
                                            "Died at Lvl.{} during the fight, {}/8",
                                            t.level, fame
                                        ),
                                        None => format!("Died at Lvl.{} during the fight", t.level),
                                    },
                                    None => "Died during the fight".to_string(),
                                };
                                resp.hover_tip(tip);
                            }
                            ParticipantEndStatus::Nexused => {
                                ctx.sprite_renderer.draw_sprite_in_rect(ui, 29714, rect);
                                resp.hover_tip("Left the fight (nexused or moved out of range)");
                            }
                            // An escaped fight ends because the local player left
                            // (nexused or walked out of range), yet the local row
                            // stays "Present" so it isn't dimmed. Still flag it
                            // with a nexus icon so escapes read at a glance. Keyed
                            // off the encounter's completion, not the individual
                            // phase: a completed run's concurrent aux phase (e.g.
                            // O3 Messengers) is never "killed" yet the local player
                            // did not escape.
                            ParticipantEndStatus::Present if p.is_local && local_escaped => {
                                ctx.sprite_renderer.draw_sprite_in_rect(ui, 29714, rect);
                                resp.hover_tip(
                                    "You left the fight (nexused or moved out of range)",
                                );
                            }
                            ParticipantEndStatus::Present => {}
                        }
                    });
                    track(numstat.response.rect, &mut row_rect);

                    // Player icon + name.
                    let name_resp = ui.horizontal(|ui| {
                        let (rect, icon_resp) =
                            ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::click());
                        if p.object_type != 0 || p.skin_id != 0 {
                            let icon = if p.skin_id > 0 {
                                p.skin_id
                            } else {
                                p.object_type
                            };
                            if !ctx.sprite_renderer.draw_dyed_outlined_character_sprite(
                                ui, icon, rect, 6, p.tex1, p.tex2,
                            ) {
                                ctx.sprite_renderer
                                    .draw_sprite_in_rect(ui, p.object_type, rect);
                            }
                        }
                        // Clicking a named player's icon jumps back to the list
                        // filtered by that player. The name label stays a plain
                        // label so it remains selectable for copy/paste.
                        if !p.name.is_empty() {
                            let icon_resp = icon_resp.on_hover_text("View this player's fights");
                            if icon_resp.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            if icon_resp.clicked() {
                                let icon = if p.skin_id > 0 {
                                    p.skin_id
                                } else {
                                    p.object_type
                                };
                                *player_click =
                                    Some((sanitize_player_name(&p.name), icon, p.tex1, p.tex2));
                            }
                        }
                        let name = if p.name.is_empty() {
                            "(unknown)".to_string()
                        } else {
                            sanitize_player_name(&p.name)
                        };
                        // Local player keeps a white name so it stands out
                        // against the accent row highlight; departed players are
                        // dimmed regardless.
                        let text = if let Some(c) = dim {
                            RichText::new(name).color(c)
                        } else if p.is_local {
                            RichText::new(name).color(Color32::WHITE)
                        } else {
                            RichText::new(name)
                        };
                        ui.label(text);
                        // Pet sprite display is temporarily excluded: it
                        // rendered the wrong sprite (health potion).
                    });
                    track(name_resp.response.rect, &mut row_rect);

                    if self.show_dmg {
                        track(
                            ui.label(dimmed(format!("{}", p.damage))).rect,
                            &mut row_rect,
                        );
                        if pct_base > 0 {
                            let pct = (p.damage as f64 / denom as f64) * 100.0;
                            track(ui.label(dimmed(format!("{:.1}%", pct))).rect, &mut row_rect);
                        } else {
                            track(ui.label(dimmed("-".to_string())).rect, &mut row_rect);
                        }
                    }
                    if self.show_hits {
                        track(ui.label(dimmed(format!("{}", p.hits))).rect, &mut row_rect);
                    }
                    if self.show_damage_taken {
                        match p.damage_taken {
                            Some(taken) => {
                                let text = if p.damage_taken_provenance
                                    == DamageTakenProvenance::Partial
                                {
                                    format!("≥{}", taken)
                                } else {
                                    format!("{}", taken)
                                };
                                track(
                                    ui.label(dimmed(text))
                                        .hover_tip(damage_taken_tooltip(p.damage_taken_provenance))
                                        .rect,
                                    &mut row_rect,
                                );
                            }
                            None => {
                                track(
                                    ui.label(RichText::new("-").color(Color32::GRAY))
                                        .hover_tip(
                                            "No damage-taken data captured for this \
                                             player (out of range, or nothing hit them).",
                                        )
                                        .rect,
                                    &mut row_rect,
                                );
                            }
                        }
                    }
                    if self.show_damage_blocked {
                        match p.damage_blocked {
                            Some(blocked) => {
                                track(
                                    ui.label(dimmed(format!("{}", blocked)))
                                        .hover_tip(
                                            "Damage negated by your defense and other \
                                             post-defense mitigation this fight.",
                                        )
                                        .rect,
                                    &mut row_rect,
                                );
                            }
                            None => {
                                track(
                                    ui.label(RichText::new("-").color(Color32::GRAY))
                                        .hover_tip(
                                            "Damage blocked is only reconstructable for \
                                             you; remote players' mitigation is not \
                                             observable.",
                                        )
                                        .rect,
                                    &mut row_rect,
                                );
                            }
                        }
                    }
                    if show_guarded {
                        match p.guarded_damage {
                            Some(gd) => {
                                let hits = p.guarded_hits.unwrap_or(0);
                                track(
                                    ui.label(dimmed(format!("{}", gd)))
                                        .hover_tip(format!(
                                            "{} hit(s) landed while Oryx was Guarded.",
                                            hits
                                        ))
                                        .rect,
                                    &mut row_rect,
                                );
                            }
                            None => {
                                track(
                                    ui.label(RichText::new("-").color(Color32::GRAY))
                                        .hover_tip("This fight predates guarded-damage tracking.")
                                        .rect,
                                    &mut row_rect,
                                );
                            }
                        }
                    }

                    // Bare gear icons (no slot frame) with rarity gems +
                    // item/enchant hover. Suppress the "Owned rarity" tooltip
                    // section: this is another player's gear, not the app-user's.
                    let gear_resp = ui.horizontal(|ui| {
                        ctx.sprite_renderer.set_hide_owned_rarity(true);
                        for (slot, &eq) in p.equipment.iter().enumerate() {
                            if eq > 0 {
                                let ench: Vec<i32> = p
                                    .equipment_enchants
                                    .get(slot)
                                    .map(|v| v.iter().map(|&e| e as i32).collect())
                                    .unwrap_or_default();
                                ctx.sprite_renderer
                                    .render_item_sprite_with_enchants_native(ui, eq, &ench, 20.0);
                            } else {
                                ui.allocate_exact_size(
                                    egui::vec2(20.0, 20.0),
                                    egui::Sense::hover(),
                                );
                            }
                        }
                        ctx.sprite_renderer.set_hide_owned_rarity(false);
                    });
                    track(gear_resp.response.rect, &mut row_rect);

                    // Tracking-accuracy badge.
                    let (label, color) = provenance_badge(p.provenance);
                    let badge = ui
                        .label(RichText::new(label).color(color).small())
                        .hover_tip(provenance_tooltip(p.provenance));
                    track(badge.rect, &mut row_rect);

                    // Paint the local-player row highlight: a semi-transparent
                    // accent band spanning the full row width, matching the
                    // filter-chip accent used elsewhere. Deferred until the whole
                    // table is laid out so the band can stretch to the table's
                    // right edge.
                    if let Some(row_rect) = row_rect {
                        table_right = table_right.max(row_rect.right());
                        if let Some(bg_idx) = bg_idx {
                            local_bands.push((bg_idx, row_rect));
                        }
                    }
                    ui.end_row();
                }

                for (bg_idx, row_rect) in local_bands {
                    let pad = ui.spacing().item_spacing.y / 2.0;
                    let pad_x = 4.0;
                    let accent = ctx.shadcn.colors().primary;
                    let fill =
                        Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 40);
                    // Span the full table width (not the row's own last cell) so
                    // the band ends where every other row ends.
                    let band = egui::Rect::from_min_max(
                        egui::pos2(row_rect.left() - pad_x, row_rect.top() - pad),
                        egui::pos2(table_right + pad_x, row_rect.bottom() + pad),
                    );
                    ui.painter()
                        .set(bg_idx, egui::Shape::rect_filled(band, 4.0, fill));
                }

                // Footer: reconcile the damage columns to the boss HP so it's
                // clear RealmHound caps at HP (excludes overkill) rather than
                // arbitrarily lowering numbers.
                if self.show_dmg {
                    if reconcile && unattributed > 0 {
                        ui.label("");
                        ui.label(RichText::new("Unattributed").italics().color(Color32::GRAY))
                            .hover_tip(
                                "Boss HP that was removed but couldn't be attributed \
                             to an observed player (out-of-range or untracked \
                             sources).",
                            );
                        ui.label(RichText::new(format!("{}", unattributed)).color(Color32::GRAY));
                        let pct = (unattributed as f64 / denom as f64) * 100.0;
                        ui.label(RichText::new(format!("{:.1}%", pct)).color(Color32::GRAY));
                        if self.show_hits {
                            ui.label("");
                        }
                        if self.show_damage_taken {
                            ui.label("");
                        }
                        if self.show_damage_blocked {
                            ui.label("");
                        }
                        if show_guarded {
                            ui.label("");
                        }
                        ui.label("");
                        ui.label("");
                        ui.end_row();
                    }

                    ui.label("");
                    ui.label(RichText::new("Total").strong());
                    ui.label(RichText::new(format!("{}", grand_total)).strong());
                    let pct_label = if pct_base > 0 {
                        format!("{:.1}%", (grand_total as f64 / denom as f64) * 100.0)
                    } else {
                        "-".to_string()
                    };
                    let total_tip = if reconcile {
                        "Damage is capped at the boss's HP; overkill beyond its \
                         HP isn't counted. Players plus Unattributed reconcile to \
                         the boss's starting HP."
                    } else {
                        "Sum of tracked damage across the shown players."
                    };
                    ui.label(RichText::new(pct_label).strong())
                        .hover_tip(total_tip);
                    if self.show_hits {
                        ui.label(RichText::new(format!("{}", total_hits)).strong());
                    }
                    if self.show_damage_taken {
                        match total_taken {
                            Some(t) => {
                                ui.label(RichText::new(format!("{}", t)).strong());
                            }
                            None => {
                                ui.label(RichText::new("-").strong().color(Color32::GRAY));
                            }
                        }
                    }
                    if self.show_damage_blocked {
                        match total_blocked {
                            Some(t) => {
                                ui.label(RichText::new(format!("{}", t)).strong());
                            }
                            None => {
                                ui.label(RichText::new("-").strong().color(Color32::GRAY));
                            }
                        }
                    }
                    if show_guarded {
                        match total_guarded {
                            Some(t) => {
                                ui.label(RichText::new(format!("{}", t)).strong());
                            }
                            None => {
                                ui.label(RichText::new("-").strong().color(Color32::GRAY));
                            }
                        }
                    }
                    ui.label("");
                    ui.label("");
                    ui.end_row();
                }
            });
        col1_x
    }

    /// Render a grouped multi-phase encounter: aggregated roster + Phases.
    fn render_encounter_detail(&mut self, ui: &mut egui::Ui, ctx: &mut PanelContext, run_id: &str) {
        // Hydrate on demand when the opened run changes.
        if self.encounter.as_ref().map(|e| e.run_id.as_str()) != Some(run_id) {
            if let Some(db) = ctx.combat_database {
                self.encounter = db.encounter_detail(run_id).ok().flatten();
            }
            // Default-expand the main (last real boss) section so its breakdown
            // is visible immediately on open.
            if let Some(enc) = &self.encounter {
                let am = get_asset_manager();
                let anchor = enc
                    .phases
                    .iter()
                    .find(|p| p.boss_object_type == enc.anchor_object_type)
                    .or_else(|| {
                        enc.phases
                            .iter()
                            .filter(|p| !am.is_treasure_crate(p.boss_object_type))
                            .last()
                    })
                    .or_else(|| enc.phases.last());
                if let Some(anchor) = anchor {
                    self.expanded_phases.insert(anchor.id);
                }
            }
        }

        let shadcn = ctx.shadcn;
        let (has_prev, has_next) = self.selection_neighbours();
        let mut go_back = false;
        let mut go_prev = false;
        let mut go_next = false;
        shadcn.header_band(ui, shadcn.secondary_header_fill(), |ui| {
            shadcn.band_row(ui, |ui| {
                if shadcn.btn(ui, "← Back").clicked() {
                    go_back = true;
                }
                if ui
                    .add_enabled(has_prev, egui::Button::new("<< Previous"))
                    .clicked()
                {
                    go_prev = true;
                }
                if ui
                    .add_enabled(has_next, egui::Button::new("Next >>"))
                    .clicked()
                {
                    go_next = true;
                }
                ui.checkbox(&mut self.show_dmg, "DMG");
                ui.checkbox(&mut self.show_hits, "Hits landed");
                ui.checkbox(&mut self.show_damage_taken, "Damage taken");
                ui.checkbox(&mut self.show_damage_blocked, "Damage blocked");
            });
        });
        if go_back {
            self.selected = None;
            self.encounter = None;
            return;
        }
        if go_prev {
            self.navigate_selection(-1);
            return;
        }
        if go_next {
            self.navigate_selection(1);
            return;
        }

        let Some(enc) = self.encounter.clone() else {
            empty_state(ui, "Encounter unavailable", None);
            return;
        };

        let mut player_click: Option<(String, i32, u32, u32)> = None;

        // When the Treasure crates category is filtered out, crate phases folded
        // into a dungeon card must not appear inside it either. An
        // empty filter set means "show all"; NO_GROUPS_KEY (hide all) also hides
        // crates.
        let hide_crates = !self.filter_groups.is_empty()
            && !self
                .filter_groups
                .contains(BossGroup::TreasureCrate.as_str());
        let mut visible_phases: Vec<&FightRecord> = enc
            .phases
            .iter()
            .filter(|p| !(hide_crates && get_asset_manager().is_treasure_crate(p.boss_object_type)))
            .collect();

        // Display order: final boss at the top, earlier bosses below in
        // reverse-kill order. Crates done after the main boss are the latest
        // phases, so they land above the main boss (folded). Sort by kill time
        // descending.
        visible_phases.sort_by(|a, b| b.ended_at.cmp(&a.ended_at));

        // The main dungeon boss: the run anchor (last real boss), else the last
        // non-crate phase. Its section auto-expands and shows the "Main" badge.
        let am = get_asset_manager();
        let anchor_id: Option<i64> = enc
            .phases
            .iter()
            .find(|p| p.boss_object_type == enc.anchor_object_type)
            .or_else(|| {
                enc.phases
                    .iter()
                    .filter(|p| !am.is_treasure_crate(p.boss_object_type))
                    .last()
            })
            .or_else(|| enc.phases.last())
            .map(|p| p.id);

        // Anchor sprite for the header icon: the main boss above.
        let icon_type = enc
            .phases
            .iter()
            .find(|p| Some(p.id) == anchor_id)
            .map(|p| p.boss_object_type)
            .unwrap_or(enc.anchor_object_type);

        // Header title = the main boss's name (falls back to the curated
        // encounter name). This keeps a boss name visible on multi-boss cards.
        let header_title = enc
            .phases
            .iter()
            .find(|p| Some(p.id) == anchor_id)
            .map(|p| p.boss_name.clone())
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| enc.display_name.clone());

        // Headline-only encounters (e.g. Towering Perfection) always show the
        // curated anchor's picture and name, even when only escaping segments
        // were damaged and the anchor phase itself never appeared as a row. The
        // stored run encounter id can be a "dungeon_run" sentinel, so resolve the
        // real encounter from the member phase types.
        let (icon_type, header_title) = enc
            .phases
            .iter()
            .find_map(|p| realmhound_core::assets::encounter_for_boss_type(p.boss_object_type))
            .filter(|e| realmhound_core::assets::encounter_headline_only(e.id))
            .map(|e| (e.anchor_type, e.display_name.to_string()))
            .unwrap_or((icon_type, header_title));

        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(6.0);
                // --- Header: stylized boss HP bar + meta ---
                let portal_map = get_dungeon_portal_map();
                let portal_id = portal_map.get_portal_id(&enc.dungeon).unwrap_or(0);

                // The HP bar reflects the main boss identified above.
                let (anchor_start, anchor_max) = enc
                    .phases
                    .iter()
                    .find(|p| Some(p.id) == anchor_id)
                    .map(|p| (p.boss_start_hp as i64, p.boss_max_hp as i64))
                    .unwrap_or((0, 0));

                let bar_inset = self.draw_boss_hp_bar(
                    ui,
                    ctx.sprite_renderer,
                    portal_id,
                    icon_type,
                    anchor_start,
                    anchor_max,
                );

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_space(bar_inset);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.label(RichText::new(&header_title).size(18.0).strong());
                            let (state, color) = if enc.killed {
                                ("Completed", Color32::from_rgb(120, 200, 120))
                            } else {
                                ("Escaped", Color32::GRAY)
                            };
                            ui.label(RichText::new(state).color(color).small());
                        });
                        // location · date time (no dot between date and time so the
                        // start time doesn't read as a duration), then the two
                        // durations on their own labeled lines.
                        let datetime_s = match Local.timestamp_millis_opt(enc.started_at).single() {
                            Some(dt) => dt.format("%Y-%m-%d %H:%M").to_string(),
                            None => "-".to_string(),
                        };
                        ui.label(
                            RichText::new(format!(
                                "{}  ·  {}",
                                portal_map.normalize_dungeon_name(&enc.dungeon),
                                datetime_s,
                            ))
                            .color(Color32::GRAY),
                        );
                        ui.label(
                            RichText::new(format!(
                                "Active combat time: {}",
                                fmt_duration(enc.active_duration_ms())
                            ))
                            .color(Color32::GRAY),
                        )
                        .hover_tip(
                            "Sum of each big boss's fight duration - active combat only, \
                         excluding add-on rows and travel/idle time between bosses.",
                        );
                        ui.label(
                            RichText::new(format!(
                                "Total dungeon time: {}",
                                fmt_duration(enc.total_dungeon_ms())
                            ))
                            .color(Color32::GRAY),
                        )
                        .hover_tip(
                            "Wall-clock time from entering the dungeon to the final \
                         boss death, including minion clear and travel between bosses.",
                        );
                        let enc_close_calls = enc.total_close_calls();
                        if enc_close_calls > 0 {
                            let cc_char_id = enc
                                .phases
                                .iter()
                                .find_map(|p| (p.local_char_id != 0).then_some(p.local_char_id))
                                .unwrap_or(0);
                            // After death the live cache drops the character, so fall
                            // back to the local player's stored roster appearance so
                            // the icon keeps its skin/dyes instead of disappearing.
                            let cc_vis = char_visuals(ctx, cc_char_id).or_else(|| {
                                enc.roster.iter().find(|p| p.is_local).and_then(|p| {
                                    let icon_id = if p.skin_id > 0 {
                                        p.skin_id
                                    } else {
                                        p.object_type
                                    };
                                    (icon_id != 0).then_some((icon_id, p.tex1, p.tex2))
                                })
                            });
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                ui.label(
                                    RichText::new(format!("Close calls: {}", enc_close_calls))
                                        .color(Color32::GRAY),
                                );
                                if let Some((icon, t1, t2)) = cc_vis {
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(20.0, 20.0),
                                        egui::Sense::hover(),
                                    );
                                    ctx.sprite_renderer
                                        .draw_dyed_outlined_character_sprite_colored_outline(
                                            ui,
                                            icon,
                                            rect,
                                            8,
                                            t1,
                                            t2,
                                            Some(Color32::from_rgba_unmultiplied(255, 0, 0, 128)),
                                            Color32::from_rgb(255, 0, 0),
                                        );
                                }
                            })
                            .response
                            .hover_tip(
                                "Times you dropped to 20% HP or below during this run \
                             (deaths included).",
                            );
                        }
                        if visible_phases.len() > 1 {
                            ui.label(
                                RichText::new(format!("{} phases grouped", visible_phases.len()))
                                    .color(Color32::from_rgb(180, 150, 90))
                                    .small(),
                            );
                        }
                        // Participants: finished / total. "Finished" means the player
                        // was still present when the fight ended (did not nexus, move
                        // out of range, or die).
                        let finished = enc
                            .roster
                            .iter()
                            .filter(|p| matches!(p.end_status, ParticipantEndStatus::Present))
                            .count();
                        let total = enc.roster.len();
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            ui.label(RichText::new("Participants:").color(Color32::GRAY).small());
                            ui.label(
                                RichText::new(format!("{}", finished))
                                    .color(Color32::GRAY)
                                    .small(),
                            )
                            .hover_tip(
                                "Players who finished the fight (were still present \
                             when it ended).",
                            );
                            ui.label(RichText::new("/").color(Color32::GRAY).small());
                            ui.label(
                                RichText::new(format!("{}", total))
                                    .color(Color32::GRAY)
                                    .small(),
                            )
                            .hover_tip("Players who participated in the fight.");
                        });
                    });
                });

                ui.add_space(8.0);
                shadcn.full_width_separator(ui);
                ui.add_space(6.0);

                for phase in &visible_phases {
                    let expanded = self.expanded_phases.contains(&phase.id);
                    let arrow = if expanded { "▼" } else { "▶" };
                    // A killed grouped encounter means every constituent phase was
                    // cleared; transition bosses (e.g. Cult Hideout cultists) lock at
                    // a threshold and never trip the per-object kill rule, so surface
                    // them as Completed here rather than Escaped.
                    let (state, scolor) = if phase.killed || enc.killed {
                        ("Completed", Color32::from_rgb(120, 200, 120))
                    } else {
                        ("Escaped", Color32::GRAY)
                    };
                    let header = format!(
                        "{arrow}  {name}   HP {start}/{max}   {dur}",
                        name = match phase.aux_member_count {
                            Some(count) if count > 0 => format!("{} x{count}", phase.boss_name),
                            _ => phase.boss_name.clone(),
                        },
                        start = phase.boss_start_hp,
                        max = phase.boss_max_hp,
                        dur = fmt_duration(phase.duration_ms()),
                    );
                    ui.horizontal(|ui| {
                        if phase.boss_object_type != 0 {
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover());
                            ctx.sprite_renderer.draw_outlined_sprite_in_rect(
                                ui,
                                phase.boss_object_type,
                                rect,
                            );
                        }
                        if ui.add(egui::Button::new(header).frame(false)).clicked() {
                            if expanded {
                                self.expanded_phases.remove(&phase.id);
                            } else {
                                self.expanded_phases.insert(phase.id);
                            }
                        }
                        ui.label(RichText::new(state).color(scolor).small());
                        if Some(phase.id) == anchor_id
                            && !am.is_treasure_crate(phase.boss_object_type)
                        {
                            ui.label(
                                RichText::new("Main")
                                    .color(Color32::from_rgb(0xff, 0xc1, 0x00))
                                    .small()
                                    .strong(),
                            )
                            .hover_tip("Main dungeon boss (shown in the HP bar above).");
                        }
                    });
                    if expanded {
                        ui.add_space(2.0);
                        let loot_x = self.render_participant_table(
                            ui,
                            ctx,
                            &phase.participants,
                            &format!("phase_table_{}", phase.id),
                            phase.boss_start_hp as i64,
                            phase.killed,
                            !enc.killed,
                            phase.boss_object_type == O3_BOSS_TYPE,
                            &mut player_click,
                        );
                        self.render_boss_loot(
                            ui,
                            ctx,
                            &FightSelection::Encounter(run_id.to_string()),
                            Some(phase.boss_object_type),
                            loot_x,
                        );
                        ui.add_space(6.0);
                    }
                }
            });
        if let Some((name, icon, tex1, tex2)) = player_click {
            self.apply_player_click(name, icon, tex1, tex2);
        }
    }
}

impl Panel for CombatHistoryPanel {
    fn show(&mut self, ui: &mut egui::Ui, ctx: &mut PanelContext) -> Vec<AppAction> {
        let Some(db) = ctx.combat_database else {
            empty_state(
                ui,
                "Combat history unavailable",
                Some("The combat database could not be opened yet."),
            );
            return Vec::new();
        };

        self.maybe_refresh(db, ctx.loot_database);

        match self.selected.clone() {
            Some(FightSelection::Single(id)) => self.render_detail(ui, ctx, id),
            Some(FightSelection::Encounter(run_id)) => {
                self.render_encounter_detail(ui, ctx, &run_id)
            }
            None => self.render_list(ui, ctx),
        }
        Vec::new()
    }
}

/// Assign each unique loot-drop id to exactly one fight: the temporally closest
/// kill. `windows[i]` is fight `i`'s `(started_at, ended_at)` in ms; each
/// `candidates` entry is `(fight_index, drop_id, drop_timestamp)`. A drop inside
/// a fight's `[start, end]` has distance 0, otherwise the distance is the gap to
/// the nearer edge. Ties prefer the more recent kill (larger `ended_at`), since a
/// bag registers at or just after the kill that dropped it. Returns a map of
/// `drop_id -> chosen fight index`.
///
/// This stops one bag being shown on every neighbouring card when several
/// same-encounter kills (e.g. back-to-back Skull Shrines) have overlapping loot
/// windows.
fn assign_drops_exclusive(
    windows: &[(i64, i64)],
    candidates: &[(usize, i64, i64)],
) -> HashMap<i64, usize> {
    fn distance((start, end): (i64, i64), ts: i64) -> i64 {
        if ts < start {
            start - ts
        } else if ts > end {
            ts - end
        } else {
            0
        }
    }
    // id -> (fight index, distance, ended_at)
    let mut best: HashMap<i64, (usize, i64, i64)> = HashMap::new();
    for &(idx, id, ts) in candidates {
        let Some(&window) = windows.get(idx) else {
            continue;
        };
        let dist = distance(window, ts);
        let ended = window.1;
        match best.get(&id) {
            Some(&(_, best_dist, best_ended)) => {
                if dist < best_dist || (dist == best_dist && ended > best_ended) {
                    best.insert(id, (idx, dist, ended));
                }
            }
            None => {
                best.insert(id, (idx, dist, ended));
            }
        }
    }
    best.into_iter()
        .map(|(id, (idx, _, _))| (id, idx))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::assign_drops_exclusive;

    #[test]
    fn bag_after_third_kill_links_only_to_that_fight() {
        // Three back-to-back Skull Shrine kills (~1 min apart, ~9s each). A single
        // white bag drops just after the third kill; with 1 min pre / 10 min post
        // windows all three fights list it as a candidate.
        let windows = [(0, 9_000), (60_000, 69_000), (120_000, 129_000)];
        let bag_ts = 150_000; // ~21s after the third kill ends
        let candidates = [
            (0usize, 42i64, bag_ts),
            (1usize, 42i64, bag_ts),
            (2usize, 42i64, bag_ts),
        ];
        let chosen = assign_drops_exclusive(&windows, &candidates);
        assert_eq!(chosen.get(&42), Some(&2));
    }

    #[test]
    fn bag_inside_a_fight_window_links_to_that_fight() {
        let windows = [(0, 9_000), (60_000, 69_000), (120_000, 129_000)];
        let bag_ts = 65_000; // inside fight 1
        let candidates = [
            (0usize, 7i64, bag_ts),
            (1usize, 7i64, bag_ts),
            (2usize, 7i64, bag_ts),
        ];
        let chosen = assign_drops_exclusive(&windows, &candidates);
        assert_eq!(chosen.get(&7), Some(&1));
    }

    #[test]
    fn distinct_bags_keep_their_own_closest_fight() {
        let windows = [(0, 9_000), (60_000, 69_000)];
        let candidates = [
            (0usize, 1i64, 10_000), // just after fight 0
            (1usize, 1i64, 10_000),
            (0usize, 2i64, 70_000), // just after fight 1
            (1usize, 2i64, 70_000),
        ];
        let chosen = assign_drops_exclusive(&windows, &candidates);
        assert_eq!(chosen.get(&1), Some(&0));
        assert_eq!(chosen.get(&2), Some(&1));
    }

    #[test]
    fn equal_distance_prefers_more_recent_kill() {
        // Bag exactly between two kills; distances tie -> the later kill wins.
        let windows = [(0, 10_000), (30_000, 40_000)];
        let bag_ts = 20_000; // 10s after fight 0 end, 10s before fight 1 start
        let candidates = [(0usize, 5i64, bag_ts), (1usize, 5i64, bag_ts)];
        let chosen = assign_drops_exclusive(&windows, &candidates);
        assert_eq!(chosen.get(&5), Some(&1));
    }
}

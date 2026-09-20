//! Loot panel - displays loot drops with filters and history.
//!

use std::collections::HashMap;
use std::path::PathBuf;

use crate::ui_ext::HoverTooltipExt;
use chrono::{Duration, Local, NaiveDate, TimeZone, Utc};
use eframe::egui::{self, Color32, RichText, ScrollArea};
use realmhound_core::{
    assets::get_dungeon_portal_map,
    combat::{CombatDatabase, FightSelection},
    loot::{LootBagType, LootDatabase, LootDropRecord, LootItemRecord, SourceFilters},
};

use crate::panels::character_card::{format_fame, render_fame_icon};
use crate::panels::loot_filters::{render_filter_bar, LootFilters, PropertyFilters};
use crate::panels::{AppAction, Panel, PanelContext};
use crate::rendering::SpriteRenderer;

/// Date range filter for history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateFilter {
    Today,
    ThisWeek,
    ThisMonth,
    AllTime,
    /// User-defined inclusive date range (UTC days).
    Custom {
        start: NaiveDate,
        end: NaiveDate,
    },
}

impl Default for DateFilter {
    fn default() -> Self {
        DateFilter::AllTime
    }
}

impl DateFilter {
    /// Get the inclusive (start, end) timestamps for this filter, in milliseconds.
    pub fn range_millis(&self) -> (i64, i64) {
        let now = Utc::now();
        let end_now = now.timestamp_millis();
        match self {
            DateFilter::Today => {
                let start_of_day = now.date_naive().and_hms_opt(0, 0, 0).unwrap();
                (start_of_day.and_utc().timestamp_millis(), end_now)
            }
            DateFilter::ThisWeek => ((now - Duration::days(7)).timestamp_millis(), end_now),
            DateFilter::ThisMonth => ((now - Duration::days(30)).timestamp_millis(), end_now),
            DateFilter::AllTime => (0, end_now),
            DateFilter::Custom { start, end } => {
                // Guard against an inverted range by swapping if needed.
                let (lo, hi) = if start <= end {
                    (*start, *end)
                } else {
                    (*end, *start)
                };
                let start_ms = lo
                    .and_hms_opt(0, 0, 0)
                    .unwrap()
                    .and_utc()
                    .timestamp_millis();
                let end_ms = hi
                    .and_hms_opt(23, 59, 59)
                    .unwrap()
                    .and_utc()
                    .timestamp_millis()
                    + 999;
                (start_ms, end_ms)
            }
        }
    }
}

/// Click action from history row icons.
#[derive(Debug, Clone)]
pub enum HistoryClickAction {
    /// Filter by mob (type ID, name)
    FilterMob(i32, String),
    /// Filter by dungeon (name)
    FilterDungeon(String),
    /// Filter by character (char ID, name, icon_id)
    FilterChar(i32, String, i32),
    /// Filter by item (item ID, name)
    FilterItem(i32, String),
    /// Open the linked fight for this drop ID.
    OpenFight(i64),
}

/// Cached autocomplete data from database.
#[derive(Debug, Clone, Default)]
struct AutocompleteCache {
    /// All distinct dungeons
    dungeons: Vec<String>,
    /// All distinct mobs (type_id, name)
    mobs: Vec<(i32, String)>,
    /// All distinct items (item_id, name)
    items: Vec<(i32, String)>,
    /// Whether cache has been loaded
    loaded: bool,
}

/// Search result entry for keyboard navigation in autocomplete.
#[derive(Clone)]
enum SearchResult {
    Dungeon {
        name: String,
        icon_id: i32,
        display_name: String,
    },
    Enemy {
        mob_type: i32,
        name: String,
    },
    Item {
        item_id: i32,
        name: String,
    },
}

/// Loot panel state.
pub struct LootPanel {
    /// Bag type filters (display filters)
    filters: LootFilters,
    /// Item-property display filters (UT / ST / Shiny)
    prop_filters: PropertyFilters,
    /// Source filters for history (mob, dungeon, character)
    source_filters: SourceFilters,
    /// History: date range filter
    history_date_filter: DateFilter,
    /// History: cached drops from DB
    history_drops: Vec<LootDropRecord>,
    /// Drop id to correlated fight/encounter selection.
    fight_links: HashMap<i64, FightSelection>,
    /// Fight-link cache needs recomputing after history changes.
    fight_links_dirty: bool,
    /// Last observed combat fight count, so new fights refresh back-links.
    last_fight_count: i64,
    /// Pending cross-panel fight open action.
    pending_open_fight: Option<FightSelection>,
    /// History: current page offset
    history_offset: i64,
    /// History: whether more results are available
    history_has_more: bool,
    /// History: total count of matching drops (for display)
    history_total_count: Option<i64>,
    /// History: needs refresh
    history_needs_refresh: bool,
    /// History: cached white bag count (updated on load)
    history_white_count: usize,
    /// History: cached orange bag count (updated on load)
    history_orange_count: usize,
    /// History: cached red bag count (updated on load)
    history_red_count: usize,
    /// Search: text input
    search_text: String,
    /// Search: autocomplete cache
    autocomplete_cache: AutocompleteCache,
    /// Search: whether popup is showing
    search_popup_open: bool,
    /// Search: keyboard-selected index in results (None = no selection)
    search_selected_index: Option<usize>,
    /// Search: previous search text (to detect changes and reset selection)
    search_prev_text: String,
    /// When set, the history list scrolls back to the top on the next render
    /// (used to reset scroll position when the tab is (re)activated).
    reset_scroll_pending: bool,
    /// Drop ids checked for batch deletion.
    selected_for_delete: std::collections::HashSet<i64>,
    /// Drop ids queued for deletion, awaiting confirmation.
    pending_delete: Option<Vec<i64>>,
    /// Last observed vertical scroll offset of the history list, so PageUp/
    /// PageDown can compute a target offset relative to the current position.
    history_scroll_offset: f32,
    /// Loot history database path for the short-lived delete writer. Injected so
    /// the panel resolves no path itself.
    loot_db_path: PathBuf,
    /// Paired combat history database path, threaded to the delete writer so its
    /// schema initialization keeps the same Unknown-source backfill behavior.
    combat_db_path: PathBuf,
}

impl Default for LootPanel {
    fn default() -> Self {
        let filters = LootFilters::default();
        let mut source_filters = SourceFilters::default();
        source_filters.set_bag_types(filters.visible_bag_type_ids());

        Self {
            filters,
            prop_filters: PropertyFilters::default(),
            source_filters,
            history_date_filter: DateFilter::AllTime,
            history_drops: Vec::new(),
            fight_links: HashMap::new(),
            fight_links_dirty: true,
            last_fight_count: -1,
            pending_open_fight: None,
            history_offset: 0,
            history_has_more: false,
            history_total_count: None,
            history_needs_refresh: true,
            history_white_count: 0,
            history_orange_count: 0,
            history_red_count: 0,
            search_text: String::new(),
            autocomplete_cache: AutocompleteCache::default(),
            search_popup_open: false,
            search_selected_index: None,
            search_prev_text: String::new(),
            reset_scroll_pending: false,
            selected_for_delete: std::collections::HashSet::new(),
            pending_delete: None,
            history_scroll_offset: 0.0,
            loot_db_path: PathBuf::new(),
            combat_db_path: PathBuf::new(),
        }
    }
}

impl LootPanel {
    /// Create a new loot panel with settings loaded from LootSettings.
    pub fn new_with_settings(settings: &realmhound_core::settings::LootSettings) -> Self {
        let filters = LootFilters {
            show_brown: settings.visible_bags.get(&0).copied().unwrap_or(true),
            show_pink: settings.visible_bags.get(&1).copied().unwrap_or(true),
            show_purple: settings.visible_bags.get(&2).copied().unwrap_or(true),
            show_teal: settings.visible_bags.get(&3).copied().unwrap_or(true),
            show_blue: settings.visible_bags.get(&4).copied().unwrap_or(true),
            show_gold: settings.visible_bags.get(&5).copied().unwrap_or(true),
            show_orange: settings.visible_bags.get(&6).copied().unwrap_or(true),
            show_white: settings.visible_bags.get(&7).copied().unwrap_or(true),
            show_red: settings.visible_bags.get(&8).copied().unwrap_or(true),
            show_egg: settings.visible_bags.get(&9).copied().unwrap_or(true),
        };

        let mut source_filters = SourceFilters::default();
        source_filters.set_bag_types(filters.visible_bag_type_ids());

        Self {
            filters,
            prop_filters: PropertyFilters::default(),
            source_filters,
            history_date_filter: DateFilter::AllTime,
            history_drops: Vec::new(),
            fight_links: HashMap::new(),
            fight_links_dirty: true,
            last_fight_count: -1,
            pending_open_fight: None,
            history_offset: 0,
            history_has_more: false,
            history_total_count: None,
            history_needs_refresh: true,
            history_white_count: 0,
            history_orange_count: 0,
            history_red_count: 0,
            search_text: String::new(),
            autocomplete_cache: AutocompleteCache::default(),
            search_popup_open: false,
            search_selected_index: None,
            search_prev_text: String::new(),
            reset_scroll_pending: false,
            selected_for_delete: std::collections::HashSet::new(),
            pending_delete: None,
            history_scroll_offset: 0.0,
            loot_db_path: PathBuf::new(),
            combat_db_path: PathBuf::new(),
        }
    }

    /// Bind the loot/combat database paths used by the short-lived delete writer.
    pub fn set_database_paths(&mut self, loot_db_path: PathBuf, combat_db_path: PathBuf) {
        self.loot_db_path = loot_db_path;
        self.combat_db_path = combat_db_path;
    }

    /// Get a reference to the current filters (for saving).
    pub fn filters(&self) -> &LootFilters {
        &self.filters
    }

    /// Reset the history list back to the top on the next render. Called when
    /// the Loot History tab is (re)activated so it always opens on the most
    /// recent drops instead of inheriting a stale scroll position.
    pub fn reset_scroll(&mut self) {
        self.reset_scroll_pending = true;
    }

    /// Render the loot panel.
    /// Returns true if filters were changed and settings should be saved.
    pub fn render<F, G>(
        &mut self,
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        get_label: F,
        get_appearance: G,
        shadcn: &crate::shadcn_ui::Shadcn,
    ) -> bool
    where
        F: Fn(i32) -> Option<String>,
        G: Fn(i32) -> Option<(i32, u32, u32)>,
    {
        // Snapshot filters before UI
        let filters_before = self.filters.clone();

        // Filter toggles in header
        shadcn.header_band(ui, shadcn.secondary_header_fill(), |ui| {
            shadcn.band_row(ui, |ui| {
                render_filter_bar(
                    ui,
                    sprite_renderer,
                    &mut self.filters,
                    &mut self.prop_filters,
                    shadcn,
                );
            });
        });

        // Render history directly
        self.render_history(ui, sprite_renderer, &get_label, &get_appearance, shadcn);

        // Batch-delete confirmation + execution.
        self.render_delete_confirm(ui, shadcn);

        // Check if bag filters changed - if so, sync to source_filters and refresh
        let filters_changed = self.filters != filters_before;
        if filters_changed {
            self.source_filters
                .set_bag_types(self.filters.visible_bag_type_ids());
            self.reset_history();
        }

        filters_changed
    }

    /// Render a small bag-count stat: a 16px bag sprite followed by the count.
    /// Always rendered (even when the count is 0) so the stats block keeps a
    /// stable width.
    fn render_bag_stat(
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        bag_id: i32,
        count: usize,
        color: Color32,
    ) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
        sprite_renderer.draw_outlined_sprite_in_rect(ui, bag_id, rect);
        ui.add_space(2.0);
        // Fixed-width count column (fits 2 digits) so the block width stays
        // stable as counts grow while scrolling/loading more.
        ui.allocate_ui_with_layout(
            egui::vec2(18.0, 16.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(RichText::new(count.to_string()).color(color).size(12.0));
            },
        );
        ui.add_space(4.0);
    }

    /// Render bag icon by raw ID (for history where bag type might be unknown).
    fn render_bag_icon_by_id(
        ui: &mut egui::Ui,
        bag_id: i32,
        bag_type: Option<LootBagType>,
        sprite_renderer: &mut SpriteRenderer,
        size: f32,
    ) {
        // Allocate space for the icon (same size as item tiles)
        let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());

        // Try to render sprite with contour outline
        if !sprite_renderer.draw_outlined_sprite_in_rect(ui, bag_id, rect) {
            // Fallback to emoji based on bag type (or "?" for unknown)
            let icon = match bag_type {
                Some(LootBagType::White) | Some(LootBagType::BoostedWhite) => "⬜",
                Some(LootBagType::Orange) | Some(LootBagType::BoostedOrange) => "🟧",
                Some(LootBagType::Red) | Some(LootBagType::BoostedRed) => "🟥",
                Some(LootBagType::Gold) | Some(LootBagType::BoostedGold) => "🟨",
                Some(LootBagType::Egg) | Some(LootBagType::BoostedEgg) => "🥚",
                Some(LootBagType::Blue) | Some(LootBagType::BoostedBlue) => "🟦",
                Some(LootBagType::Teal) | Some(LootBagType::BoostedTeal) => "🩵",
                Some(LootBagType::Purple) | Some(LootBagType::BoostedPurple) => "🟪",
                Some(LootBagType::Pink) | Some(LootBagType::BoostedPink) => "🩷",
                Some(LootBagType::Brown) | Some(LootBagType::BoostedBrown) => "🟫",
                Some(LootBagType::Soulbound) => "🔒",
                None => "❓",
            };
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                icon,
                egui::FontId::proportional(size * 0.63),
                Color32::WHITE,
            );
        }

        let tooltip = match bag_type {
            Some(bt) => bt.name().to_string(),
            None => format!("Unknown bag (ID: {})", bag_id),
        };
        response.hover_tip(tooltip);
    }

    /// Render dungeon portal sprite (clickable version for history).
    /// Returns Some(dungeon_name) if clicked.
    fn render_dungeon_sprite_clickable(
        ui: &mut egui::Ui,
        dungeon: &str,
        sprite_renderer: &mut SpriteRenderer,
    ) -> Option<String> {
        if Self::render_dungeon_sprite_impl(ui, dungeon, sprite_renderer, true) {
            Some(dungeon.to_string())
        } else {
            None
        }
    }

    /// Internal: render dungeon sprite, returns true if clicked.
    fn render_dungeon_sprite_impl(
        ui: &mut egui::Ui,
        dungeon: &str,
        sprite_renderer: &mut SpriteRenderer,
        clickable: bool,
    ) -> bool {
        let sense = if clickable {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(egui::vec2(38.0, 38.0), sense);

        if !ui.is_rect_visible(rect) {
            return false;
        }

        // Get the dungeon portal mapping
        let portal_map = get_dungeon_portal_map();
        let display_name = portal_map.normalize_dungeon_name(dungeon);
        let portal_id = portal_map.get_portal_id(dungeon);

        // Try to draw portal sprite with contour outline
        let sprite_drawn = if let Some(id) = portal_id {
            sprite_renderer.draw_outlined_sprite_in_rect(ui, id, rect)
        } else {
            false
        };

        // Fallback if no sprite
        if !sprite_drawn {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "🚪",
                egui::FontId::proportional(22.0),
                Color32::from_rgb(150, 180, 200),
            );
        }

        // Hover highlight for clickable
        if clickable && response.hovered() {
            ui.painter().rect_stroke(
                rect,
                3.0,
                egui::Stroke::new(2.0_f32, Color32::from_rgb(100, 150, 200)),
                egui::StrokeKind::Outside,
            );
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        // Tooltip shows dungeon name with click hint
        let tooltip = if clickable {
            format!("{}\n(Click to filter)", display_name)
        } else {
            display_name
        };
        let response = response.hover_tip(tooltip);

        response.clicked()
    }

    /// Render mob sprite (clickable version for history).
    /// Returns Some((mob_type, mob_name)) if clicked.
    fn render_mob_sprite_clickable(
        ui: &mut egui::Ui,
        mob_type: i32,
        mob_name: &str,
        sprite_renderer: &mut SpriteRenderer,
    ) -> Option<(i32, String)> {
        if Self::render_mob_sprite_impl(ui, mob_type, mob_name, sprite_renderer, true) {
            Some((mob_type, mob_name.to_string()))
        } else {
            None
        }
    }

    /// Internal: render mob sprite, returns true if clicked.
    fn render_mob_sprite_impl(
        ui: &mut egui::Ui,
        mob_type: i32,
        mob_name: &str,
        sprite_renderer: &mut SpriteRenderer,
        clickable: bool,
    ) -> bool {
        // Never render an environmental structure (wall/gate/pillar) as a drop
        // source, even if a legacy row stored one; fall back to the "?" marker.
        let valid_source = mob_type > 0
            && realmhound_core::assets::get_asset_manager().is_valid_drop_source(mob_type);
        let sense = if clickable {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(egui::vec2(38.0, 38.0), sense);

        // Only draw if visible
        if !ui.is_rect_visible(rect) {
            return false;
        }

        // Try to draw mob sprite with contour outline
        let sprite_drawn = if valid_source {
            sprite_renderer.draw_outlined_sprite_in_rect(ui, mob_type, rect)
        } else {
            false
        };

        // If no sprite was drawn, show "?" overlay
        if !sprite_drawn {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "?",
                egui::FontId::monospace(24.0),
                Color32::from_rgb(180, 150, 200),
            );
        }

        // Hover highlight for clickable
        if clickable && response.hovered() {
            ui.painter().rect_stroke(
                rect,
                3.0,
                egui::Stroke::new(2.0_f32, Color32::from_rgb(150, 100, 200)),
                egui::StrokeKind::Outside,
            );
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        // Show mob name in tooltip with click hint
        let name = if !valid_source || mob_name.is_empty() {
            "Unknown"
        } else {
            mob_name
        };
        let tooltip = if clickable {
            format!("{}\n(Click to filter)", name)
        } else {
            name.to_string()
        };
        let response = response.hover_tip(tooltip);

        response.clicked()
    }

    /// Render character sprite (clickable version for history).
    /// Returns Some((char_id, display_name, icon_id)) if clicked.
    /// display_name will be the custom label if set, otherwise the in-game name.
    fn render_character_sprite_clickable(
        ui: &mut egui::Ui,
        icon_id: i32,
        char_id: i32,
        char_name: &str,
        custom_label: Option<&str>,
        exalt_bonus: i32,
        is_seasonal: bool,
        tex1: u32,
        tex2: u32,
        sprite_renderer: &mut SpriteRenderer,
    ) -> Option<(i32, String, i32)> {
        if Self::render_character_sprite_impl(
            ui,
            icon_id,
            char_id,
            char_name,
            custom_label,
            exalt_bonus,
            is_seasonal,
            tex1,
            tex2,
            sprite_renderer,
            true,
        ) {
            // Use custom label if set, otherwise fall back to in-game name
            let display_name = custom_label
                .map(|s| s.to_string())
                .unwrap_or_else(|| char_name.to_string());
            Some((char_id, display_name, icon_id))
        } else {
            None
        }
    }

    /// Internal: render character sprite, returns true if clicked.
    fn render_character_sprite_impl(
        ui: &mut egui::Ui,
        icon_id: i32,
        char_id: i32,
        char_name: &str,
        custom_label: Option<&str>,
        exalt_bonus: i32,
        is_seasonal: bool,
        tex1: u32,
        tex2: u32,
        sprite_renderer: &mut SpriteRenderer,
        clickable: bool,
    ) -> bool {
        let sense = if clickable {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(egui::vec2(38.0, 38.0), sense);

        // Only draw if visible
        if !ui.is_rect_visible(rect) {
            return false;
        }

        // Try to draw character sprite with dye compositing and contour outline, direction=6 (facing right)
        let sprite_drawn = if icon_id > 0 {
            sprite_renderer.draw_dyed_outlined_character_sprite(ui, icon_id, rect, 6, tex1, tex2)
        } else {
            false
        };

        // If no sprite was drawn, show character icon fallback
        if !sprite_drawn {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "👤",
                egui::FontId::proportional(22.0),
                Color32::from_rgb(180, 180, 200),
            );
        }

        // Hover highlight for clickable
        if clickable && response.hovered() {
            ui.painter().rect_stroke(
                rect,
                3.0,
                egui::Stroke::new(2.0_f32, Color32::from_rgb(100, 200, 150)),
                egui::StrokeKind::Outside,
            );
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        // Build tooltip with character info
        let mut tooltip = if !char_name.is_empty() {
            format!("{}", char_name)
        } else {
            format!("Char ID: {}", char_id)
        };
        if let Some(label) = custom_label {
            tooltip.push_str(&format!("\nLabel: {}", label));
        }
        if exalt_bonus > 0 {
            tooltip.push_str(&format!("\nExalt Bonus: {}%", exalt_bonus));
        }
        if is_seasonal {
            tooltip.push_str("\n🍂 Seasonal");
        }
        if clickable {
            tooltip.push_str("\n(Click to filter)");
        }

        let response = response.hover_tip(tooltip);

        response.clicked()
    }

    /// Format date for history display.
    fn format_date(timestamp_ms: i64) -> String {
        let dt = Utc
            .timestamp_millis_opt(timestamp_ms)
            .single()
            .map(|utc| utc.with_timezone(&Local));

        match dt {
            Some(local) => local.format("%Y-%m-%d %H:%M").to_string(),
            None => "????-??-?? ??:??".to_string(),
        }
    }

    /// Render the history browser.
    fn render_history<F, G>(
        &mut self,
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        get_label: &F,
        get_appearance: &G,
        shadcn: &crate::shadcn_ui::Shadcn,
    ) where
        F: Fn(i32) -> Option<String>,
        G: Fn(i32) -> Option<(i32, u32, u32)>,
    {
        // Date filter bar + search + source filters on same line
        let mut search_filter_applied = false;

        // Drop-count stats and selectable ids, computed before the bar so the
        // counts can render inline at the start of the filter row and the
        // "Select" toggle can act on every currently-visible row.
        let prop = self.prop_filters;
        let prop_active = prop.any();
        let visible_ids: Vec<i64> = self
            .history_drops
            .iter()
            .filter(|d| prop.matches_items(&d.items))
            .map(|d| d.id)
            .collect();
        let (stat_white, stat_orange, stat_red) = if prop_active {
            let mut w = 0usize;
            let mut o = 0usize;
            let mut r = 0usize;
            for d in self
                .history_drops
                .iter()
                .filter(|d| prop.matches_items(&d.items))
            {
                if let Some(b) = LootBagType::from_id(d.bag_type) {
                    if b.is_white() {
                        w += 1;
                    }
                    if b.is_orange() {
                        o += 1;
                    }
                    if b.is_red() {
                        r += 1;
                    }
                }
            }
            (w, o, r)
        } else {
            (
                self.history_white_count,
                self.history_orange_count,
                self.history_red_count,
            )
        };
        let stat_shown = if prop_active {
            visible_ids.len()
        } else {
            self.history_drops.len()
        };
        let raw_scanned = self.history_drops.len();
        let total_count = self.history_total_count;

        shadcn.header_band_stacked_divided(ui, shadcn.secondary_header_fill(), |ui| {
            shadcn.band_row(ui, |ui| {
                // Batch-select controls on the far left, so the "Select" checkbox
                // sits directly above the per-row checkboxes.
                let all_selected = !visible_ids.is_empty()
                    && visible_ids
                        .iter()
                        .all(|id| self.selected_for_delete.contains(id));
                let mut select_all = all_selected;
                if ui
                    .checkbox(&mut select_all, "Select")
                    .hover_tip("Select every visible drop")
                    .changed()
                {
                    if all_selected {
                        for id in &visible_ids {
                            self.selected_for_delete.remove(id);
                        }
                    } else {
                        for &id in &visible_ids {
                            self.selected_for_delete.insert(id);
                        }
                    }
                }
                ui.separator();

                // Fixed-width drop-count stats block so the toolbar doesn't jump as
                // the numbers change on search. Sized for a 7-digit total and a
                // 4-digit loaded count; W/O/R bag counts are always shown.
                ui.allocate_ui_with_layout(
                    egui::vec2(400.0, 22.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        if prop_active {
                            ui.label(format!(
                                "History drops: {} matching ({} scanned)",
                                stat_shown, raw_scanned
                            ));
                        } else if let Some(total) = total_count {
                            if total as usize > stat_shown {
                                ui.label(format!(
                                    "History drops: {} ({} loaded)",
                                    total, stat_shown
                                ));
                            } else {
                                ui.label(format!("History drops: {}", total));
                            }
                        } else {
                            ui.label(format!("History drops: {}", stat_shown));
                        }
                        ui.add_space(6.0);
                        // Bag-count stats: white / orange / red bag sprites + counts,
                        // always shown so the block width is stable.
                        Self::render_bag_stat(
                            ui,
                            sprite_renderer,
                            LootBagType::White.id(),
                            stat_white,
                            Color32::from_rgb(230, 230, 230),
                        );
                        Self::render_bag_stat(
                            ui,
                            sprite_renderer,
                            LootBagType::Orange.id(),
                            stat_orange,
                            Color32::from_rgb(255, 165, 0),
                        );
                        Self::render_bag_stat(
                            ui,
                            sprite_renderer,
                            LootBagType::Red.id(),
                            stat_red,
                            Color32::from_rgb(255, 80, 80),
                        );
                    },
                );
                ui.separator();

                ui.label("Date Range:");

                let old_filter = self.history_date_filter;
                let mut date_key = Some(
                    match self.history_date_filter {
                        DateFilter::Today => "today",
                        DateFilter::ThisWeek => "week",
                        DateFilter::ThisMonth => "month",
                        DateFilter::AllTime => "all",
                        DateFilter::Custom { .. } => "custom",
                    }
                    .to_string(),
                );
                shadcn.sel(
                    ui,
                    "history_date_filter",
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
                if let Some(key) = &date_key {
                    let new_filter = match key.as_str() {
                        "today" => DateFilter::Today,
                        "week" => DateFilter::ThisWeek,
                        "month" => DateFilter::ThisMonth,
                        "all" => DateFilter::AllTime,
                        "custom" => {
                            if matches!(self.history_date_filter, DateFilter::Custom { .. }) {
                                self.history_date_filter
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
                    self.history_date_filter = new_filter;
                }

                // Inline calendar pickers when a custom range is active
                if let DateFilter::Custom { mut start, mut end } = self.history_date_filter {
                    shadcn.date_range_picker(ui, "loot_hist_range", &mut start, &mut end);
                    self.history_date_filter = DateFilter::Custom { start, end };
                }

                // If filter (or a custom date) changed, mark for refresh
                if old_filter != self.history_date_filter {
                    self.reset_history();
                }

                ui.separator();

                if shadcn
                    .btn(ui, "🔄")
                    .hover_tip("Refresh the page to see the latest drops")
                    .clicked()
                {
                    self.reset_history();
                }

                ui.separator();

                // Search bar inline
                if self.render_search_bar(ui, sprite_renderer, shadcn) {
                    search_filter_applied = true;
                }

                // Active source filters (on same line)
                if !self.source_filters.is_empty() {
                    ui.separator();

                    // Mob filter pill
                    if let (Some(mob_type), Some(mob_name)) = (
                        self.source_filters.mob_type,
                        &self.source_filters.mob_name.clone(),
                    ) {
                        if sprite_renderer.render_filter_pill(
                            ui,
                            mob_type,
                            mob_name,
                            Color32::from_rgb(60, 40, 80),
                        ) {
                            self.source_filters.clear_mob();
                            self.reset_history();
                        }
                    }

                    // Dungeon filter pill
                    if let Some(dungeon) = &self.source_filters.dungeon.clone() {
                        let portal_map = get_dungeon_portal_map();
                        let icon_id = portal_map.get_portal_id(dungeon).unwrap_or(0);
                        let display_name = portal_map.normalize_dungeon_name(dungeon);
                        if sprite_renderer.render_filter_pill(
                            ui,
                            icon_id,
                            &display_name,
                            Color32::from_rgb(40, 50, 70),
                        ) {
                            self.source_filters.clear_dungeon();
                            self.reset_history();
                        }
                    }

                    // Character filter pill
                    if let (Some(char_id), Some(icon_id), Some(char_name)) = (
                        self.source_filters.char_id,
                        self.source_filters.char_icon_id,
                        &self.source_filters.char_name.clone(),
                    ) {
                        let (icon, tex1, tex2) = get_appearance(char_id).unwrap_or((icon_id, 0, 0));
                        if sprite_renderer.render_filter_pill_character(
                            ui,
                            icon,
                            tex1,
                            tex2,
                            char_name,
                            Color32::from_rgb(40, 60, 50),
                        ) {
                            self.source_filters.clear_char();
                            self.reset_history();
                        }
                    }

                    // Item filter pill
                    if let (Some(item_id), Some(item_name)) = (
                        self.source_filters.item_id,
                        &self.source_filters.item_name.clone(),
                    ) {
                        if sprite_renderer.render_filter_pill(
                            ui,
                            item_id,
                            item_name,
                            Color32::from_rgb(70, 50, 40),
                        ) {
                            self.source_filters.clear_item();
                            self.reset_history();
                        }
                    }

                    if shadcn
                        .btn(ui, "Clear")
                        .hover_tip("Clear all filters")
                        .clicked()
                    {
                        self.source_filters.clear();
                        self.search_text.clear();
                        self.reset_history();
                    }
                }

                // Batch-delete action lives at the end of the toolbar (to the right
                // of the search bar), matching Combat History.
                let selected_n = self.selected_for_delete.len();
                if selected_n > 0 {
                    ui.separator();
                    if shadcn
                        .btn(ui, format!("Delete selected ({selected_n})"))
                        .clicked()
                    {
                        self.pending_delete =
                            Some(self.selected_for_delete.iter().cloned().collect());
                    }
                }
            });
        });

        // Apply search filter if needed (must be outside horizontal due to borrow)
        if search_filter_applied {
            self.reset_history();
        }

        // Item-property display filter (UT / ST / Shiny): keep only bags that
        // contain at least one matching item. Whole bags (all items) are shown.
        // Applied here over loaded rows; raw `history_drops` is left intact so
        // DB pagination (offset = history_drops.len()) is unaffected. Reuses the
        // `prop`/`prop_active` computed above for the inline stats.
        let display_indices: Vec<usize> = self
            .history_drops
            .iter()
            .enumerate()
            .filter(|(_, d)| prop.matches_items(&d.items))
            .map(|(i, _)| i)
            .collect();

        // If nothing is visible and there is nothing more to load, show empty state.
        if display_indices.is_empty() && !self.history_needs_refresh && !self.history_has_more {
            ui.vertical_centered(|ui| {
                ui.add_space(50.0);
                if self.source_filters.is_empty() && !prop_active {
                    ui.label(
                        RichText::new("No historical drops")
                            .size(18.0)
                            .color(Color32::GRAY),
                    );
                    ui.add_space(10.0);
                    ui.label("Use 'Refresh' to load drops from the database.");
                } else {
                    ui.label(
                        RichText::new("No drops match filters")
                            .size(18.0)
                            .color(Color32::GRAY),
                    );
                    ui.add_space(10.0);
                    if !self.source_filters.is_empty() && shadcn.btn(ui, "Clear Filters").clicked()
                    {
                        self.source_filters.clear();
                        self.reset_history();
                    }
                }
            });
            return;
        }

        // Collect click actions from rows
        let mut click_action: Option<HistoryClickAction> = None;
        let mut should_load_more = false;
        // Row whose selection checkbox was toggled this frame (applied after scroll).
        let mut toggle_id: Option<i64> = None;

        // Row height: 42.0 content + 2.0 spacing = 44.0 per row
        const ROW_HEIGHT: f32 = 44.0;
        let total_rows = display_indices.len() + if self.history_has_more { 1 } else { 0 };

        // PageUp/PageDown scroll a viewport-sized step relative to the last known
        // offset. Only acted on when the pointer is over this panel so it does
        // not fight other scroll areas.
        let mut key_scroll_target: Option<f32> = None;
        if ui.rect_contains_pointer(ui.max_rect()) {
            let (page_up, page_down, home, end) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::PageUp),
                    i.key_pressed(egui::Key::PageDown),
                    i.key_pressed(egui::Key::Home),
                    i.key_pressed(egui::Key::End),
                )
            });
            if home {
                key_scroll_target = Some(0.0);
            } else if end {
                // Exact bottom offset. Must stay within range: the virtualized
                // show_rows below derives its row indices from the offset, so an
                // out-of-range value would panic on the row slice.
                let max_offset = (total_rows as f32 * ROW_HEIGHT - ui.available_height()).max(0.0);
                key_scroll_target = Some(max_offset);
            } else if page_up || page_down {
                let page = (ui.available_height() - ROW_HEIGHT).max(ROW_HEIGHT);
                let delta = if page_down { page } else { -page };
                key_scroll_target = Some((self.history_scroll_offset + delta).max(0.0));
            }
        }

        // Virtualized scrollable drop list - only renders visible rows
        let mut history_area = ScrollArea::vertical()
            .id_salt("loot_history_list")
            .auto_shrink([false, false]);
        if self.reset_scroll_pending {
            self.reset_scroll_pending = false;
            history_area = history_area.vertical_scroll_offset(0.0);
        } else if let Some(target) = key_scroll_target {
            history_area = history_area.vertical_scroll_offset(target);
        }
        let scroll_output = history_area.show_rows(ui, ROW_HEIGHT, total_rows, |ui, row_range| {
            for row_idx in row_range {
                if row_idx < display_indices.len() {
                    let drop = &self.history_drops[display_indices[row_idx]];
                    let id = drop.id;
                    let mut checked = self.selected_for_delete.contains(&id);
                    let was_checked = checked;
                    let has_fight_link = self.fight_links.contains_key(&id);

                    if let Some(action) = Self::render_loot_card(
                        ui,
                        drop,
                        sprite_renderer,
                        shadcn,
                        &get_label,
                        &get_appearance,
                        Some(&mut checked),
                        true,
                        has_fight_link,
                        0.0,
                    ) {
                        click_action = Some(action);
                    }
                    if checked != was_checked {
                        toggle_id = Some(id);
                    }
                } else if self.history_has_more {
                    // Loading indicator row
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Scroll to load more...");
                    });
                    ui.add_space(10.0);
                }
            }
        });

        // Remember the current scroll offset for the next frame's PageUp/Down.
        self.history_scroll_offset = scroll_output.state.offset.y;

        // Check if user has scrolled near the bottom (infinite scroll trigger)
        if self.history_has_more && !self.history_needs_refresh {
            let content_height = scroll_output.content_size.y;
            let visible_height = scroll_output.inner_rect.height();
            let scroll_offset = scroll_output.state.offset.y;

            // Trigger load when within 100 pixels of bottom
            let distance_from_bottom = content_height - (scroll_offset + visible_height);
            if distance_from_bottom < 100.0 {
                should_load_more = true;
            }

            // When a property filter is active, a loaded page may yield few/no
            // matches and never fill the viewport, so the scroll trigger never
            // fires. Keep pulling raw pages until the viewport fills or the data
            // is exhausted. Gated by `!history_needs_refresh` => one load at a time.
            if prop_active && content_height <= visible_height + 1.0 {
                should_load_more = true;
            }
        }

        // Trigger load more if needed (offset = number of raw items already loaded)
        if should_load_more {
            self.history_offset = self.history_drops.len() as i64;
            self.history_needs_refresh = true;
        }

        // Apply row selection toggle collected during the scroll closure.
        if let Some(id) = toggle_id {
            if !self.selected_for_delete.remove(&id) {
                self.selected_for_delete.insert(id);
            }
        }

        // Process click action after scroll area (to avoid borrow issues)
        if let Some(action) = click_action {
            match action {
                HistoryClickAction::FilterMob(mob_type, mob_name) => {
                    self.set_mob_filter(mob_type, mob_name);
                }
                HistoryClickAction::FilterDungeon(dungeon) => {
                    self.set_dungeon_filter(dungeon);
                }
                HistoryClickAction::FilterChar(char_id, char_name, icon_id) => {
                    self.set_char_filter(char_id, char_name, icon_id);
                }
                HistoryClickAction::FilterItem(item_id, item_name) => {
                    self.set_item_filter(item_id, item_name);
                }
                HistoryClickAction::OpenFight(drop_id) => {
                    if let Some(selection) = self.fight_links.get(&drop_id).cloned() {
                        self.pending_open_fight = Some(selection);
                    }
                }
            }
        }
    }

    /// Render a single history drop row: [Timestamp] [Fame]f [BagIcon] [MobSprite] [Items...]
    /// Returns Some(action) if user clicked on a filterable icon.
    pub(crate) fn render_history_drop_row<F, G>(
        ui: &mut egui::Ui,
        drop: &LootDropRecord,
        sprite_renderer: &mut SpriteRenderer,
        get_label: F,
        get_appearance: G,
        clickable: bool,
        has_fight_link: bool,
    ) -> Option<HistoryClickAction>
    where
        F: Fn(i32) -> Option<String>,
        G: Fn(i32) -> Option<(i32, u32, u32)>,
    {
        let row_height = 42.0;
        let bag_type = LootBagType::from_id(drop.bag_type);

        // Bag type filtering is now done at the database level

        let mut click_action: Option<HistoryClickAction> = None;

        // `ui` is already a single-line, vertically-centered layout bounded to
        // the card's content rect, so render directly into it. Wrapping in an
        // extra horizontal/min-height layout re-anchored the content and pushed
        // the sprites off-center, so it is intentionally avoided here.

        // Date/time
        ui.label(
            RichText::new(Self::format_date(drop.timestamp))
                .monospace()
                .size(12.0),
        );

        ui.add_space(4.0);

        // Fame at drop time: orange fame sprite followed by the value, in a
        // fixed-width column so the bag and every later column stay aligned
        // across rows regardless of the fame value. Sized to fit a 7-digit
        // fame (e.g. "9,999,999") without pushing the rest of the row.
        const FAME_COL_WIDTH: f32 = 92.0;
        ui.allocate_ui_with_layout(
            egui::vec2(FAME_COL_WIDTH, row_height),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_width(FAME_COL_WIDTH);
                ui.spacing_mut().item_spacing.x = 2.0;
                render_fame_icon(ui, sprite_renderer, 13.0, false);
                ui.label(
                    RichText::new(format_fame(drop.fame as i64))
                        .monospace()
                        .size(11.0)
                        .color(Color32::from_rgb(255, 200, 100)),
                );
            },
        );

        ui.add_space(4.0);

        // Draw bag sprite (use raw ID for history since bag_type might be None)
        Self::render_bag_icon_by_id(ui, drop.bag_type, bag_type, sprite_renderer, 38.0);

        ui.add_space(4.0);

        // Character/skin sprite (shows who got the loot) - CLICKABLE
        let custom_label = get_label(drop.char_id);
        let (icon, tex1, tex2) =
            get_appearance(drop.char_id).unwrap_or((drop.icon_id(), drop.tex1, drop.tex2));
        if clickable {
            if let Some((char_id, char_name, icon_id)) = Self::render_character_sprite_clickable(
                ui,
                icon,
                drop.char_id,
                &drop.char_name,
                custom_label.as_deref(),
                drop.exalt_bonus,
                drop.is_seasonal,
                tex1,
                tex2,
                sprite_renderer,
            ) {
                click_action = Some(HistoryClickAction::FilterChar(char_id, char_name, icon_id));
            }
        } else {
            Self::render_character_sprite_impl(
                ui,
                icon,
                drop.char_id,
                &drop.char_name,
                custom_label.as_deref(),
                drop.exalt_bonus,
                drop.is_seasonal,
                tex1,
                tex2,
                sprite_renderer,
                false,
            );
        }

        ui.add_space(4.0);

        // Dungeon portal sprite - CLICKABLE
        if clickable {
            if let Some(dungeon) =
                Self::render_dungeon_sprite_clickable(ui, &drop.dungeon, sprite_renderer)
            {
                click_action = Some(HistoryClickAction::FilterDungeon(dungeon));
            }
        } else {
            Self::render_dungeon_sprite_impl(ui, &drop.dungeon, sprite_renderer, false);
        }

        ui.add_space(4.0);

        // Mob sprite (with tooltip for mob name) - CLICKABLE
        if clickable {
            if let Some((mob_type, mob_name)) = Self::render_mob_sprite_clickable(
                ui,
                drop.mob_type,
                &drop.mob_name,
                sprite_renderer,
            ) {
                click_action = Some(HistoryClickAction::FilterMob(mob_type, mob_name));
            }
        } else {
            Self::render_mob_sprite_impl(ui, drop.mob_type, &drop.mob_name, sprite_renderer, false);
        }

        ui.add_space(8.0);

        // Items - CLICKABLE
        for item in &drop.items {
            if clickable {
                if let Some((item_id, item_name)) =
                    Self::render_history_item_clickable(ui, item, sprite_renderer)
                {
                    click_action = Some(HistoryClickAction::FilterItem(item_id, item_name));
                }
            } else {
                sprite_renderer.render_item_tile(
                    ui,
                    item.item_id,
                    Some(&item.item_name),
                    &item.parsed_enchant_ids,
                );
            }
        }

        if has_fight_link {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(4.0);
                if ui
                    .button(RichText::new("⚔ View fight details").size(12.0))
                    .clicked()
                {
                    click_action = Some(HistoryClickAction::OpenFight(drop.id));
                }
            });
        }

        click_action
    }

    /// Render a compact loot row for the Combat History detail: bag sprite plus
    /// its item tiles only, dropping the date, fame, character, dungeon, and mob
    /// columns used by the full history row.
    /// Render a compact bag: one bag sprite followed by all its items at gear
    /// size. `items` may span several same-tier drops merged into one bag (an
    /// 8-slot overflow of a single drop is shown as a single bag).
    pub(crate) fn render_compact_bag(
        ui: &mut egui::Ui,
        bag_type_id: i32,
        items: &[&LootItemRecord],
        sprite_renderer: &mut SpriteRenderer,
    ) {
        const COMPACT_SIZE: f32 = 20.0;
        let bag_type = LootBagType::from_id(bag_type_id);
        Self::render_bag_icon_by_id(ui, bag_type_id, bag_type, sprite_renderer, COMPACT_SIZE);
        ui.add_space(4.0);
        for item in items {
            sprite_renderer.render_item_sprite_with_enchants_native(
                ui,
                item.item_id,
                &item.parsed_enchant_ids,
                COMPACT_SIZE,
            );
        }
    }

    /// Render a full loot card (background, optional select checkbox, drop row,
    /// hover border) shared by Loot History and the Live Feed. Pass
    /// `selection = Some(&mut checked)` to show the delete checkbox (History) or
    /// `None` to omit it (Live Feed). `clickable` enables click-to-filter on the
    /// sprites (History only). `left_bleed` extends the card's left edge outward
    /// so its border lines up with sibling rows that draw a `StrokeKind::Outside`
    /// outline (Live Feed dungeon/boss rows); pass `0.0` for Loot History.
    /// Returns the click-to-filter action, if any.
    pub(crate) fn render_loot_card<F, G>(
        ui: &mut egui::Ui,
        drop: &LootDropRecord,
        sprite_renderer: &mut SpriteRenderer,
        shadcn: &crate::shadcn_ui::Shadcn,
        get_label: F,
        get_appearance: G,
        selection: Option<&mut bool>,
        clickable: bool,
        has_fight_link: bool,
        left_bleed: f32,
    ) -> Option<HistoryClickAction>
    where
        F: Fn(i32) -> Option<String>,
        G: Fn(i32) -> Option<(i32, u32, u32)>,
    {
        const ROW_HEIGHT: f32 = 44.0;
        let full_width = ui.available_width();
        let (alloc_rect, _) = ui.allocate_exact_size(
            egui::vec2(full_width, ROW_HEIGHT - 2.0),
            egui::Sense::hover(),
        );
        // Content stays anchored to the allocated rect; only the drawn box may
        // extend left so its border aligns with outlined sibling rows.
        let card_rect =
            egui::Rect::from_min_max(alloc_rect.min - egui::vec2(left_bleed, 0.0), alloc_rect.max);
        let hovered = ui.rect_contains_pointer(card_rect);
        ui.painter()
            .rect_filled(card_rect, 6.0, shadcn.secondary_header_fill());
        let content_rect = egui::Rect::from_min_max(
            alloc_rect.min + egui::vec2(8.0, 0.0),
            alloc_rect.max - egui::vec2(8.0, 0.0),
        );
        let mut cui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(content_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let cui = &mut cui;
        if let Some(selected) = selection {
            cui.checkbox(selected, "");
            cui.add_space(4.0);
        }
        let action = Self::render_history_drop_row(
            cui,
            drop,
            sprite_renderer,
            get_label,
            get_appearance,
            clickable,
            has_fight_link,
        );
        if hovered {
            ui.painter().rect_stroke(
                card_rect,
                6.0,
                egui::Stroke::new(1.5_f32, Color32::from_rgb(90, 130, 170)),
                egui::StrokeKind::Inside,
            );
        }
        action
    }

    /// Render a history item (from LootItemRecord) - clickable to filter by item.
    /// Returns Some((item_id, item_name)) if clicked.
    fn render_history_item_clickable(
        ui: &mut egui::Ui,
        item: &realmhound_core::loot::LootItemRecord,
        sprite_renderer: &mut SpriteRenderer,
    ) -> Option<(i32, String)> {
        // Use pre-parsed enchant IDs (parsed once on DB load, not every frame)
        let response = sprite_renderer.render_item_tile_clickable(
            ui,
            item.item_id,
            Some(&item.item_name),
            &item.parsed_enchant_ids,
        );

        if response.clicked() {
            Some((item.item_id, item.item_name.clone()))
        } else {
            None
        }
    }

    /// Load history drops from database.
    pub fn load_history(&mut self, db: &LootDatabase) {
        if !self.history_needs_refresh {
            return;
        }

        let (start, end) = self.history_date_filter.range_millis();
        let limit = 100i64;

        // When loading first page, also get total count for display
        if self.history_offset == 0 {
            match db.count_drops_filtered(start, end, &self.source_filters) {
                Ok(count) => {
                    self.history_total_count = Some(count);
                }
                Err(e) => {
                    eprintln!("Failed to count history: {}", e);
                    self.history_total_count = None;
                }
            }
        }

        // Use filtered query with source filters
        match db.get_drops_filtered(start, end, &self.source_filters, self.history_offset, limit) {
            Ok(drops) => {
                self.history_has_more = drops.len() == limit as usize;
                if self.history_offset == 0 {
                    self.history_drops = drops;
                } else {
                    self.history_drops.extend(drops);
                }
                self.history_needs_refresh = false;
                self.fight_links_dirty = true;

                // Update cached bag type counts
                self.history_white_count = self
                    .history_drops
                    .iter()
                    .filter(|d| {
                        LootBagType::from_id(d.bag_type)
                            .map(|b| b.is_white())
                            .unwrap_or(false)
                    })
                    .count();
                self.history_orange_count = self
                    .history_drops
                    .iter()
                    .filter(|d| {
                        LootBagType::from_id(d.bag_type)
                            .map(|b| b.is_orange())
                            .unwrap_or(false)
                    })
                    .count();
                self.history_red_count = self
                    .history_drops
                    .iter()
                    .filter(|d| {
                        LootBagType::from_id(d.bag_type)
                            .map(|b| b.is_red())
                            .unwrap_or(false)
                    })
                    .count();
            }
            Err(e) => {
                eprintln!("Failed to load history: {}", e);
            }
        }
    }

    fn refresh_fight_links(&mut self, combat_db: &CombatDatabase) {
        if !self.fight_links_dirty {
            return;
        }

        self.fight_links.clear();
        for drop in &self.history_drops {
            match combat_db.find_killed_fight(drop.map_seed, drop.mob_type, drop.timestamp) {
                Ok(Some(selection)) => {
                    self.fight_links.insert(drop.id, selection);
                }
                Ok(None) => {}
                Err(e) => tracing::warn!("[LOOT] find_killed_fight failed: {}", e),
            }
        }
        self.fight_links_dirty = false;
    }

    /// Reset history state for a fresh load (when filters change).
    fn reset_history(&mut self) {
        self.history_offset = 0;
        self.history_drops.clear();
        self.fight_links.clear();
        self.fight_links_dirty = true;
        self.history_total_count = None;
        self.history_needs_refresh = true;
        self.history_white_count = 0;
        self.history_orange_count = 0;
        self.history_red_count = 0;
    }

    /// Confirm modal + execution for batch-deleting selected drops.
    fn render_delete_confirm(&mut self, ui: &mut egui::Ui, shadcn: &crate::shadcn_ui::Shadcn) {
        let Some(ids) = self.pending_delete.clone() else {
            return;
        };

        let mut do_confirm = false;
        let mut do_cancel = false;
        egui::Window::new("Delete selected drops")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ui.ctx(), |ui| {
                ui.set_width(300.0);
                ui.add_space(4.0);
                ui.label(format!(
                    "Permanently delete {} selected drop(s)?",
                    ids.len()
                ));
                ui.add_space(14.0);
                let gap = 8.0;
                let bw = ((ui.available_width() - gap) / 2.0).max(80.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    if shadcn.button_min_width(ui, "Cancel", bw).clicked() {
                        do_cancel = true;
                    }
                    if shadcn
                        .button_destructive_min_width(ui, "Delete", true, bw)
                        .clicked()
                    {
                        do_confirm = true;
                    }
                });
            });

        if do_cancel {
            self.pending_delete = None;
            return;
        }
        if !do_confirm {
            return;
        }
        self.pending_delete = None;

        match LootDatabase::open_writer(&self.loot_db_path, Some(&self.combat_db_path)) {
            Ok(mut writer) => match writer.delete_drops(&ids) {
                Ok(_) => {
                    self.selected_for_delete.clear();
                    self.reset_history();
                    self.invalidate_autocomplete_cache();
                }
                Err(e) => tracing::warn!("[LOOT] batch delete failed: {}", e),
            },
            Err(e) => tracing::warn!("[LOOT] open writer for batch delete failed: {}", e),
        }
    }

    /// Set mob filter and trigger refresh.
    pub fn set_mob_filter(&mut self, mob_type: i32, mob_name: String) {
        self.source_filters.set_mob(mob_type, mob_name);
        self.reset_history();
    }

    /// Set dungeon filter and trigger refresh.
    pub fn set_dungeon_filter(&mut self, dungeon: String) {
        self.source_filters.set_dungeon(dungeon);
        self.reset_history();
    }

    /// Set character filter and trigger refresh.
    pub fn set_char_filter(&mut self, char_id: i32, char_name: String, icon_id: i32) {
        self.source_filters.set_char(char_id, char_name, icon_id);
        self.reset_history();
    }

    /// Set character filter exclusively - clears all other source filters first.
    /// Used when navigating from the Characters panel to view loot for a specific character.
    pub fn set_char_filter_exclusive(&mut self, char_id: i32, char_name: String, icon_id: i32) {
        // Clear all source filters (preserves bag_types)
        self.source_filters.clear();
        // Set only the character filter
        self.source_filters.set_char(char_id, char_name, icon_id);
        self.reset_history();
    }

    /// Set item filter and trigger refresh.
    pub fn set_item_filter(&mut self, item_id: i32, item_name: String) {
        self.source_filters.set_item(item_id, item_name);
        self.reset_history();
    }

    /// Load autocomplete cache from database (call once when opening search).
    pub fn load_autocomplete_cache(&mut self, db: &LootDatabase) {
        if self.autocomplete_cache.loaded {
            return;
        }

        // Load dungeons
        if let Ok(dungeons) = db.get_distinct_dungeons() {
            self.autocomplete_cache.dungeons = dungeons;
        }

        // Load mobs
        if let Ok(mobs) = db.get_distinct_mobs() {
            self.autocomplete_cache.mobs = mobs;
        }

        // Load items
        if let Ok(items) = db.get_distinct_items() {
            self.autocomplete_cache.items = items;
        }

        self.autocomplete_cache.loaded = true;
    }

    /// Invalidate autocomplete cache (call after new drops are recorded).
    pub fn invalidate_autocomplete_cache(&mut self) {
        self.autocomplete_cache.loaded = false;
    }

    /// Mark the history view as needing a refresh (call after new drops are
    /// recorded so the next render re-queries the database).
    pub fn mark_history_dirty(&mut self) {
        self.history_needs_refresh = true;
    }

    /// Render search bar with autocomplete suggestions.
    /// Returns true if a filter was applied.
    fn render_search_bar(
        &mut self,
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        shadcn: &crate::shadcn_ui::Shadcn,
    ) -> bool {
        let mut filter_applied = false;

        // Reset selection when search text changes
        if self.search_text != self.search_prev_text {
            self.search_selected_index = None;
            self.search_prev_text = self.search_text.clone();
        }

        ui.label("🔍");

        // Search text input
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.search_text)
                .id_source("loot_search_box")
                .desired_width(230.0)
                .hint_text("Search item, boss or dungeon..."),
        );

        // Live-filter the drops list by free text as the user types,
        // mirroring Combat History. Selecting a suggestion still applies a
        // precise source chip below.
        if response.changed() {
            let trimmed = self.search_text.trim();
            self.source_filters.search_text = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            };
            filter_applied = true;
        }

        // Check if Enter was pressed to submit (TextEdit loses focus on Enter)
        let enter_pressed = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

        // Open popup when focused and has text
        if response.has_focus() && !self.search_text.is_empty() {
            self.search_popup_open = true;
        }

        // Clear button
        if !self.search_text.is_empty() && shadcn.btn(ui, "✕").clicked() {
            self.search_text.clear();
            self.search_popup_open = false;
            self.search_selected_index = None;
            self.source_filters.search_text = None;
            filter_applied = true;
        }

        // Build flat list of search results for keyboard navigation
        let search_lower = self.search_text.to_lowercase();
        let mut results: Vec<SearchResult> = Vec::new();

        if !self.search_text.is_empty() {
            // Collect dungeon matches
            let portal_map = get_dungeon_portal_map();
            for dungeon in self
                .autocomplete_cache
                .dungeons
                .iter()
                .filter(|d| d.to_lowercase().contains(&search_lower))
                .take(5)
            {
                let icon_id = portal_map.get_portal_id(dungeon).unwrap_or(0);
                let display_name = portal_map.normalize_dungeon_name(dungeon);
                results.push(SearchResult::Dungeon {
                    name: dungeon.clone(),
                    icon_id,
                    display_name,
                });
            }

            // Collect enemy matches
            for (mob_type, mob_name) in self
                .autocomplete_cache
                .mobs
                .iter()
                .filter(|(_, name)| name.to_lowercase().contains(&search_lower))
                .take(5)
            {
                results.push(SearchResult::Enemy {
                    mob_type: *mob_type,
                    name: mob_name.clone(),
                });
            }

            // Collect item matches
            for (item_id, item_name) in self
                .autocomplete_cache
                .items
                .iter()
                .filter(|(_, name)| name.to_lowercase().contains(&search_lower))
                .take(10)
            {
                results.push(SearchResult::Item {
                    item_id: *item_id,
                    name: item_name.clone(),
                });
            }
        }

        let total_results = results.len();

        // Handle Enter key when TextEdit loses focus (singleline submits on Enter)
        if enter_pressed && self.search_popup_open && total_results > 0 {
            // Apply selected result, or first result if none selected
            let idx = self.search_selected_index.unwrap_or(0);
            if let Some(result) = results.get(idx) {
                match result {
                    SearchResult::Dungeon { name, .. } => {
                        self.source_filters.set_dungeon(name.clone());
                    }
                    SearchResult::Enemy { mob_type, name } => {
                        self.source_filters.set_mob(*mob_type, name.clone());
                    }
                    SearchResult::Item { item_id, name } => {
                        self.source_filters.set_item(*item_id, name.clone());
                    }
                }
                self.search_text.clear();
                self.search_popup_open = false;
                self.search_selected_index = None;
                self.source_filters.search_text = None;
                filter_applied = true;
            }
        }

        // Handle keyboard navigation when popup is open and text field has focus
        if self.search_popup_open && response.has_focus() && total_results > 0 {
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

        // Render autocomplete popup below the text input
        if self.search_popup_open && !self.search_text.is_empty() {
            let popup_id = ui.make_persistent_id("search_popup");
            let popup_pos = response.rect.left_bottom() + egui::vec2(0.0, 2.0);

            egui::Area::new(popup_id)
                .fixed_pos(popup_pos)
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_min_width(250.0);
                        ui.set_max_height(300.0);

                        egui::ScrollArea::vertical()
                            .max_height(280.0)
                            .show(ui, |ui| {
                                if results.is_empty() {
                                    ui.label(
                                        RichText::new("No matches found")
                                            .italics()
                                            .color(Color32::GRAY),
                                    );
                                } else {
                                    let mut current_idx = 0usize;
                                    let mut last_category: Option<&str> = None;

                                    for result in &results {
                                        // Show category header when switching types
                                        let category = match result {
                                            SearchResult::Dungeon { .. } => "Dungeons",
                                            SearchResult::Enemy { .. } => "Enemies",
                                            SearchResult::Item { .. } => "Items",
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
                                            self.search_selected_index == Some(current_idx);

                                        let (icon_id, display_text) = match result {
                                            SearchResult::Dungeon {
                                                icon_id,
                                                display_name,
                                                ..
                                            } => (*icon_id, display_name.as_str()),
                                            SearchResult::Enemy { mob_type, name } => {
                                                (*mob_type, name.as_str())
                                            }
                                            SearchResult::Item { item_id, name } => {
                                                (*item_id, name.as_str())
                                            }
                                        };

                                        ui.horizontal(|ui| {
                                            // Small sprite
                                            let (rect, _) = ui.allocate_exact_size(
                                                egui::vec2(20.0, 20.0),
                                                egui::Sense::hover(),
                                            );
                                            if icon_id > 0 {
                                                // Fit oversized boss/portal art (e.g. Ancient
                                                // Kaiju, Oryx the Mad God 3) to the row box,
                                                // matching the active filter pill. Gear
                                                // items keep crisp integer scaling.
                                                if matches!(result, SearchResult::Item { .. }) {
                                                    if !sprite_renderer
                                                        .draw_outlined_sprite_in_rect(
                                                            ui, icon_id, rect,
                                                        )
                                                    {
                                                        sprite_renderer
                                                            .draw_sprite_in_rect(ui, icon_id, rect);
                                                    }
                                                } else {
                                                    sprite_renderer
                                                        .draw_outlined_sprite_in_rect_filled(
                                                            ui, icon_id, rect,
                                                        );
                                                }
                                            }

                                            // Selectable label with highlight for keyboard selection
                                            let label_response = ui.add(
                                                egui::Button::new(display_text)
                                                    .selected(is_selected),
                                            );

                                            // Scroll to selected item when using keyboard navigation
                                            if is_selected {
                                                label_response
                                                    .scroll_to_me(Some(egui::Align::Center));
                                            }

                                            if label_response.clicked() {
                                                match result {
                                                    SearchResult::Dungeon { name, .. } => {
                                                        self.source_filters
                                                            .set_dungeon(name.clone());
                                                    }
                                                    SearchResult::Enemy { mob_type, name } => {
                                                        self.source_filters
                                                            .set_mob(*mob_type, name.clone());
                                                    }
                                                    SearchResult::Item { item_id, name } => {
                                                        self.source_filters
                                                            .set_item(*item_id, name.clone());
                                                    }
                                                }
                                                self.search_text.clear();
                                                self.search_popup_open = false;
                                                self.search_selected_index = None;
                                                self.source_filters.search_text = None;
                                                filter_applied = true;
                                            }
                                        });

                                        current_idx += 1;
                                    }
                                }
                            });
                    });
                });

            // Close popup when clicking outside
            if ui.input(|i| i.pointer.any_click()) && !response.has_focus() {
                self.search_popup_open = false;
                self.search_selected_index = None;
            }
        }

        filter_applied
    }
}

impl Panel for LootPanel {
    fn show(&mut self, ui: &mut egui::Ui, ctx: &mut PanelContext) -> Vec<AppAction> {
        // Preload history and autocomplete cache if database is available
        if let Some(db) = ctx.loot_database {
            self.load_history(db);
            self.load_autocomplete_cache(db);
        }
        if let Some(db) = ctx.combat_database {
            // New/deleted fights can create or remove back-links, so recompute
            // when the fight count changes even if the drop list is unchanged.
            if let Ok(count) = db.fight_count() {
                if count != self.last_fight_count {
                    self.fight_links_dirty = true;
                    self.last_fight_count = count;
                }
            }
            self.refresh_fight_links(db);
        }

        let labels = ctx.labels;
        let account_data = ctx.account_data;
        let get_label = |char_id: i32| -> Option<String> { labels.get(&char_id).cloned() };
        let get_appearance = |char_id: i32| -> Option<(i32, u32, u32)> {
            crate::panels::character_appearance(account_data, char_id)
        };
        let mut actions = Vec::new();
        if self.render(
            ui,
            ctx.sprite_renderer,
            get_label,
            get_appearance,
            ctx.shadcn,
        ) {
            actions.push(AppAction::SaveLootSettings);
        }
        if let Some(selection) = self.pending_open_fight.take() {
            actions.push(AppAction::OpenFight { selection });
        }
        actions
    }
}

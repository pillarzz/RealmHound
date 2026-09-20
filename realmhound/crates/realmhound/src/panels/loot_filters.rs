//! Shared loot filter state and filter-bar UI.
//!
//! Used by both the Loot History tab (`panels::loot`) and the Live Feed tab
//! (`panels::live_feed`) so the bag-type / item-property filters stay in one
//! place instead of being duplicated per panel.

use crate::ui_ext::HoverTooltipExt;
use eframe::egui::{self, Color32, RichText};
use realmhound_core::{
    assets::get_asset_manager,
    loot::{LootBagType, LootItemRecord, ProcessedLootItem},
    settings::LiveFeedFilterSettings,
};

use crate::rendering::SpriteRenderer;
use crate::shadcn_ui::Shadcn;

/// Bag type filter settings.
///
/// All bag types can be tracked. High-tier bags (White, Orange, Red, Gold,
/// Blue) are always tracked. Low-tier bags (Brown, Soulbound, Pink, Purple,
/// Egg, Teal) are only tracked when they contain items with valuable
/// enchantments (Unique, Awakened, or High Loot Bonus).
#[derive(Debug, Clone, PartialEq)]
pub struct LootFilters {
    pub show_white: bool,
    pub show_orange: bool,
    pub show_red: bool,
    pub show_gold: bool,
    pub show_blue: bool,
    // Conditional bags (only tracked with valuable enchants)
    pub show_egg: bool,
    pub show_teal: bool,
    pub show_purple: bool,
    pub show_pink: bool,
    pub show_brown: bool,
}

impl Default for LootFilters {
    fn default() -> Self {
        // Show all tracked bags by default
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
        }
    }
}

impl LootFilters {
    /// Get all visible bag type IDs for database filtering.
    pub fn visible_bag_type_ids(&self) -> Vec<i32> {
        let mut ids = Vec::new();
        if self.show_white {
            ids.push(LootBagType::White as i32);
            ids.push(LootBagType::BoostedWhite as i32);
        }
        if self.show_orange {
            ids.push(LootBagType::Orange as i32);
            ids.push(LootBagType::BoostedOrange as i32);
        }
        if self.show_red {
            ids.push(LootBagType::Red as i32);
            ids.push(LootBagType::BoostedRed as i32);
        }
        if self.show_gold {
            ids.push(LootBagType::Gold as i32);
            ids.push(LootBagType::BoostedGold as i32);
        }
        if self.show_blue {
            ids.push(LootBagType::Blue as i32);
            ids.push(LootBagType::BoostedBlue as i32);
        }
        if self.show_egg {
            ids.push(LootBagType::Egg as i32);
            ids.push(LootBagType::BoostedEgg as i32);
        }
        if self.show_teal {
            ids.push(LootBagType::Teal as i32);
            ids.push(LootBagType::BoostedTeal as i32);
        }
        if self.show_purple {
            ids.push(LootBagType::Purple as i32);
            ids.push(LootBagType::BoostedPurple as i32);
        }
        if self.show_pink {
            ids.push(LootBagType::Pink as i32);
            ids.push(LootBagType::BoostedPink as i32);
        }
        if self.show_brown {
            ids.push(LootBagType::Brown as i32);
            ids.push(LootBagType::BoostedBrown as i32);
            ids.push(LootBagType::Soulbound as i32);
        }
        ids
    }

    /// Whether a bag of this type passes the current bag-type filter.
    ///
    /// Mirrors [`visible_bag_type_ids`], so boosted variants and Soulbound are
    /// handled exactly as the database-level filter does.
    pub fn is_visible(&self, bag_type: LootBagType) -> bool {
        self.visible_bag_type_ids().contains(&bag_type.id())
    }

    /// Toggle all filters on or off.
    pub fn set_all(&mut self, enabled: bool) {
        self.show_white = enabled;
        self.show_orange = enabled;
        self.show_red = enabled;
        self.show_gold = enabled;
        self.show_blue = enabled;
        self.show_egg = enabled;
        self.show_teal = enabled;
        self.show_purple = enabled;
        self.show_pink = enabled;
        self.show_brown = enabled;
    }

    /// Build bag-type filters from persisted Live Feed settings.
    pub fn from_live_feed_settings(s: &LiveFeedFilterSettings) -> Self {
        Self {
            show_white: s.show_white,
            show_orange: s.show_orange,
            show_red: s.show_red,
            show_gold: s.show_gold,
            show_blue: s.show_blue,
            show_egg: s.show_egg,
            show_teal: s.show_teal,
            show_purple: s.show_purple,
            show_pink: s.show_pink,
            show_brown: s.show_brown,
        }
    }

    /// Copy bag-type filters into persisted Live Feed settings.
    pub fn write_to_live_feed_settings(&self, s: &mut LiveFeedFilterSettings) {
        s.show_white = self.show_white;
        s.show_orange = self.show_orange;
        s.show_red = self.show_red;
        s.show_gold = self.show_gold;
        s.show_blue = self.show_blue;
        s.show_egg = self.show_egg;
        s.show_teal = self.show_teal;
        s.show_purple = self.show_purple;
        s.show_pink = self.show_pink;
        s.show_brown = self.show_brown;
    }
}

/// An item that can be matched against [`PropertyFilters`].
///
/// Abstracts over the two in-app item representations so the property filter
/// logic is written once: stored history rows ([`LootItemRecord`]) and live
/// feed items ([`ProcessedLootItem`]).
pub(crate) trait FilterItem {
    fn item_id(&self) -> i32;
    /// Enchant count used to derive rarity. Must match the displayed overlay.
    fn enchant_count(&self) -> usize;
}

impl FilterItem for LootItemRecord {
    fn item_id(&self) -> i32 {
        self.item_id
    }
    fn enchant_count(&self) -> usize {
        self.parsed_enchant_ids.len()
    }
}

impl FilterItem for ProcessedLootItem {
    fn item_id(&self) -> i32 {
        self.item_id
    }
    fn enchant_count(&self) -> usize {
        self.enchant_ids.len()
    }
}

/// Item-property display filters (UT / ST / Shiny + rarity).
///
/// Applied at display time over a bag's items. A bag is shown when it contains
/// at least one item that satisfies BOTH active subgroups (AND between groups),
/// where each subgroup is OR within itself:
///   * type subgroup: UT / ST / Shiny
///   * rarity subgroup (by enchant count): None(0) / Uncommon(1) / Rare(2) /
///     Legendary(3) / Divine(4+)
/// A subgroup imposes no constraint when it matches everything: the type group
/// when no button is enabled, the rarity group when all (or no) buttons are
/// enabled. Rarity defaults to all-enabled, i.e. no rarity constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropertyFilters {
    /// Show items that are Untiered (UT).
    pub ut: bool,
    /// Show items that are Set Tier (ST).
    pub st: bool,
    /// Show items that are shiny.
    pub shiny: bool,
    /// Rarity: 0-enchant items.
    pub r_none: bool,
    /// Rarity: 1-enchant (Uncommon) items.
    pub r_uncommon: bool,
    /// Rarity: 2-enchant (Rare) items.
    pub r_rare: bool,
    /// Rarity: 3-enchant (Legendary) items.
    pub r_legendary: bool,
    /// Rarity: 4+-enchant (Divine) items.
    pub r_divine: bool,
}

impl Default for PropertyFilters {
    fn default() -> Self {
        // Type filters off; all rarity buttons on (= no rarity constraint).
        Self {
            ut: false,
            st: false,
            shiny: false,
            r_none: true,
            r_uncommon: true,
            r_rare: true,
            r_legendary: true,
            r_divine: true,
        }
    }
}

impl PropertyFilters {
    /// Whether any item-type filter (UT/ST/Shiny) is enabled.
    fn type_active(&self) -> bool {
        self.ut || self.st || self.shiny
    }

    /// Whether all rarity buttons are enabled (equivalent to no rarity filter).
    fn rarity_all(&self) -> bool {
        self.r_none && self.r_uncommon && self.r_rare && self.r_legendary && self.r_divine
    }

    /// Whether at least one rarity button is enabled.
    fn rarity_any(&self) -> bool {
        self.r_none || self.r_uncommon || self.r_rare || self.r_legendary || self.r_divine
    }

    /// Whether the rarity group actually narrows results: a strict, non-empty
    /// subset of rarities is selected. All-on and all-off both impose no
    /// constraint (show everything).
    fn rarity_constraining(&self) -> bool {
        self.rarity_any() && !self.rarity_all()
    }

    /// Whether any property filter actually narrows results.
    pub(crate) fn any(&self) -> bool {
        self.type_active() || self.rarity_constraining()
    }

    /// Build property filters from persisted Live Feed settings.
    pub fn from_live_feed_settings(s: &LiveFeedFilterSettings) -> Self {
        Self {
            ut: s.ut,
            st: s.st,
            shiny: s.shiny,
            r_none: s.r_none,
            r_uncommon: s.r_uncommon,
            r_rare: s.r_rare,
            r_legendary: s.r_legendary,
            r_divine: s.r_divine,
        }
    }

    /// Copy property filters into persisted Live Feed settings.
    pub fn write_to_live_feed_settings(&self, s: &mut LiveFeedFilterSettings) {
        s.ut = self.ut;
        s.st = self.st;
        s.shiny = self.shiny;
        s.r_none = self.r_none;
        s.r_uncommon = self.r_uncommon;
        s.r_rare = self.r_rare;
        s.r_legendary = self.r_legendary;
        s.r_divine = self.r_divine;
    }

    /// Returns true if a bag with these items should be displayed under the
    /// current filters. When no filter narrows results, every bag matches.
    /// Otherwise the bag is shown if at least one item satisfies every active
    /// subgroup (AND between groups, OR within each group).
    pub(crate) fn matches_items<I: FilterItem>(&self, items: &[I]) -> bool {
        if !self.any() {
            return true;
        }
        let am = get_asset_manager();
        let type_active = self.type_active();
        let rarity_constraining = self.rarity_constraining();
        items.iter().any(|item| {
            let id = item.item_id();
            let type_ok = !type_active
                || (self.ut && am.is_ut(id))
                || (self.shiny && am.is_shiny(id))
                || (self.st && am.get_object(id).map(|o| o.is_st()).unwrap_or(false));
            if !type_ok {
                return false;
            }
            if !rarity_constraining {
                return true;
            }
            // Rarity is derived from enchant count, matching the displayed overlay.
            match item.enchant_count() {
                0 => self.r_none,
                1 => self.r_uncommon,
                2 => self.r_rare,
                3 => self.r_legendary,
                _ => self.r_divine,
            }
        })
    }
}

/// Render the loot filter bar: bag-type toggles, All/None, and the item-property
/// (UT/ST/Shiny + rarity) toggles. Mutates the filters in place; callers detect
/// changes by snapshotting before/after.
pub(crate) fn render_filter_bar(
    ui: &mut egui::Ui,
    sprite_renderer: &mut SpriteRenderer,
    filters: &mut LootFilters,
    prop: &mut PropertyFilters,
    shadcn: &Shadcn,
) {
    ui.label("Filters:");

    // Always-tracked bags (high tier)
    filter_button_bag(
        ui,
        sprite_renderer,
        LootBagType::White,
        &mut filters.show_white,
    );
    filter_button_bag(
        ui,
        sprite_renderer,
        LootBagType::Orange,
        &mut filters.show_orange,
    );
    filter_button_bag(ui, sprite_renderer, LootBagType::Red, &mut filters.show_red);
    filter_button_bag(
        ui,
        sprite_renderer,
        LootBagType::Gold,
        &mut filters.show_gold,
    );
    filter_button_bag(
        ui,
        sprite_renderer,
        LootBagType::Blue,
        &mut filters.show_blue,
    );

    ui.separator();

    // Conditional bags (tracked only with valuable enchants)
    filter_button_bag(ui, sprite_renderer, LootBagType::Egg, &mut filters.show_egg);
    filter_button_bag(
        ui,
        sprite_renderer,
        LootBagType::Teal,
        &mut filters.show_teal,
    );
    filter_button_bag(
        ui,
        sprite_renderer,
        LootBagType::Purple,
        &mut filters.show_purple,
    );
    filter_button_bag(
        ui,
        sprite_renderer,
        LootBagType::Pink,
        &mut filters.show_pink,
    );
    filter_button_bag(
        ui,
        sprite_renderer,
        LootBagType::Brown,
        &mut filters.show_brown,
    );

    ui.separator();

    if shadcn
        .btn_small(ui, RichText::new("All").small())
        .hover_tip("Show all bag types")
        .clicked()
    {
        filters.set_all(true);
    }
    if shadcn
        .btn_small(ui, RichText::new("None").small())
        .hover_tip("Hide all bag types")
        .clicked()
    {
        filters.set_all(false);
    }

    ui.separator();

    // Item-property filters (UT / ST / Shiny). Show bags containing a matching item.
    prop_button(
        ui,
        shadcn,
        "UT",
        &mut prop.ut,
        "Only show bags containing an Untiered (UT) item",
    );
    prop_button(
        ui,
        shadcn,
        "ST",
        &mut prop.st,
        "Only show bags containing a Set Tier (ST) item",
    );
    sprite_renderer.shiny_toggle(
        ui,
        shadcn,
        &mut prop.shiny,
        "Only show bags containing a shiny item",
    );

    ui.separator();

    // Rarity filters (by enchant count). Combine with the type filters above
    // as AND (a single item must satisfy both groups); multiple rarities are OR.
    rarity_button(
        ui,
        sprite_renderer,
        None,
        "0",
        &mut prop.r_none,
        "Only show bags with a 0-enchant (None) item",
    );
    rarity_button(
        ui,
        sprite_renderer,
        Some(0),
        "1",
        &mut prop.r_uncommon,
        "Only show bags with a 1-enchant (Uncommon) item",
    );
    rarity_button(
        ui,
        sprite_renderer,
        Some(1),
        "2",
        &mut prop.r_rare,
        "Only show bags with a 2-enchant (Rare) item",
    );
    rarity_button(
        ui,
        sprite_renderer,
        Some(2),
        "3",
        &mut prop.r_legendary,
        "Only show bags with a 3-enchant (Legendary) item",
    );
    rarity_button(
        ui,
        sprite_renderer,
        Some(3),
        "4",
        &mut prop.r_divine,
        "Only show bags with a 4+-enchant (Divine) item",
    );
}

/// Render a single item-property toggle button (UT / ST / Shiny).
fn prop_button(
    ui: &mut egui::Ui,
    shadcn: &crate::shadcn_ui::Shadcn,
    label: &str,
    enabled: &mut bool,
    tooltip: &str,
) {
    shadcn
        .tgl(ui, enabled, RichText::new(label).small())
        .hover_tip(tooltip);
}

/// Render a single rarity toggle button using the enchant-tier sprite.
///
/// `tier_idx` is the enchant overlay index (0=Uncommon..3=Divine), or `None`
/// for the 0-enchant ("None") rarity which has no sprite. `fallback_label` is
/// drawn when the sprite is unavailable (e.g. overlays not yet loaded, or the
/// "None" rarity).
fn rarity_button(
    ui: &mut egui::Ui,
    sprite_renderer: &mut SpriteRenderer,
    tier_idx: Option<usize>,
    fallback_label: &str,
    enabled: &mut bool,
    tooltip: &str,
) {
    let size = egui::vec2(18.0, 18.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let drawn = match tier_idx {
            Some(idx) => sprite_renderer.draw_enchant_tier_in_rect(ui, idx, rect),
            None => false,
        };

        if !drawn {
            // Fallback: show enchant-count label on a small chip.
            ui.painter()
                .rect_filled(rect, 2.0, Color32::from_rgb(55, 55, 60));
            let text_color = if *enabled {
                Color32::WHITE
            } else {
                Color32::from_rgb(120, 120, 120)
            };
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                fallback_label,
                egui::FontId::proportional(12.0),
                text_color,
            );
        }

        // Draw dimming overlay when disabled (expand rect to fully cover sprite)
        if !*enabled {
            let expanded_rect = rect.expand(2.0);
            ui.painter().rect_filled(
                expanded_rect,
                2.0,
                Color32::from_rgba_unmultiplied(0, 0, 0, 180),
            );
        }

        // Draw border when enabled
        if *enabled {
            ui.painter().rect_stroke(
                rect,
                2.0,
                egui::Stroke::new(1.0_f32, Color32::WHITE),
                egui::StrokeKind::Outside,
            );
        }
    }

    if response.hover_tip(tooltip).clicked() {
        *enabled = !*enabled;
    }
}

/// Helper to render a bag sprite filter toggle button.
fn filter_button_bag(
    ui: &mut egui::Ui,
    sprite_renderer: &mut SpriteRenderer,
    bag_type: LootBagType,
    enabled: &mut bool,
) {
    let size = egui::vec2(18.0, 18.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        // Try to render the bag sprite with tint for disabled state
        let drawn = sprite_renderer.draw_sprite_in_rect(ui, bag_type.id(), rect);

        if !drawn {
            // Fallback: draw a placeholder square
            let fill = if *enabled {
                Color32::from_rgb(100, 100, 100)
            } else {
                Color32::from_rgb(40, 40, 40)
            };
            ui.painter().rect_filled(rect, 2.0, fill);
        }

        // Draw dimming overlay when disabled (expand rect to fully cover sprite)
        if !*enabled {
            let expanded_rect = rect.expand(2.0);
            ui.painter().rect_filled(
                expanded_rect,
                2.0,
                Color32::from_rgba_unmultiplied(0, 0, 0, 180),
            );
        }

        // Draw border when enabled
        if *enabled {
            ui.painter().rect_stroke(
                rect,
                2.0,
                egui::Stroke::new(1.0_f32, Color32::WHITE),
                egui::StrokeKind::Outside,
            );
        }
    }

    // Tooltip indicates this filter includes boosted variants
    let tooltip = if bag_type.is_boosted() {
        format!("{} bags", bag_type.name())
    } else {
        format!("{} bags (includes loot boosted)", bag_type.name())
    };
    if response.hover_tip(tooltip).clicked() {
        *enabled = !*enabled;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use realmhound_core::loot::{LootBagType, ProcessedLootItem};

    #[test]
    fn test_filter_includes_boosted_variants() {
        let mut filters = LootFilters::default();

        filters.show_white = true;
        let ids = filters.visible_bag_type_ids();
        assert!(ids.contains(&(LootBagType::White as i32)));
        assert!(ids.contains(&(LootBagType::BoostedWhite as i32)));

        filters.show_white = false;
        let ids = filters.visible_bag_type_ids();
        assert!(!ids.contains(&(LootBagType::White as i32)));
        assert!(!ids.contains(&(LootBagType::BoostedWhite as i32)));
    }

    #[test]
    fn test_all_boosted_variants_follow_base_filter() {
        let mut filters = LootFilters::default();
        filters.set_all(false);

        let ids = filters.visible_bag_type_ids();
        assert!(!ids.contains(&(LootBagType::BoostedWhite as i32)));
        assert!(!ids.contains(&(LootBagType::BoostedOrange as i32)));
        assert!(!ids.contains(&(LootBagType::BoostedRed as i32)));
        assert!(!ids.contains(&(LootBagType::BoostedGold as i32)));
        assert!(!ids.contains(&(LootBagType::BoostedBlue as i32)));
        assert!(!ids.contains(&(LootBagType::BoostedEgg as i32)));
        assert!(!ids.contains(&(LootBagType::BoostedTeal as i32)));
        assert!(!ids.contains(&(LootBagType::BoostedPurple as i32)));
        assert!(!ids.contains(&(LootBagType::BoostedPink as i32)));
        assert!(!ids.contains(&(LootBagType::BoostedBrown as i32)));

        filters.set_all(true);

        let ids = filters.visible_bag_type_ids();
        assert!(ids.contains(&(LootBagType::BoostedWhite as i32)));
        assert!(ids.contains(&(LootBagType::BoostedOrange as i32)));
        assert!(ids.contains(&(LootBagType::BoostedRed as i32)));
        assert!(ids.contains(&(LootBagType::BoostedGold as i32)));
        assert!(ids.contains(&(LootBagType::BoostedBlue as i32)));
        assert!(ids.contains(&(LootBagType::BoostedEgg as i32)));
        assert!(ids.contains(&(LootBagType::BoostedTeal as i32)));
        assert!(ids.contains(&(LootBagType::BoostedPurple as i32)));
        assert!(ids.contains(&(LootBagType::BoostedPink as i32)));
        assert!(ids.contains(&(LootBagType::BoostedBrown as i32)));
    }

    #[test]
    fn test_is_visible_matches_visible_ids() {
        let mut filters = LootFilters::default();
        // Brown toggle also governs the Soulbound bag.
        filters.set_all(false);
        filters.show_brown = true;
        assert!(filters.is_visible(LootBagType::Brown));
        assert!(filters.is_visible(LootBagType::BoostedBrown));
        assert!(filters.is_visible(LootBagType::Soulbound));
        assert!(!filters.is_visible(LootBagType::White));

        filters.show_brown = false;
        filters.show_white = true;
        assert!(!filters.is_visible(LootBagType::Soulbound));
        assert!(filters.is_visible(LootBagType::White));
        assert!(filters.is_visible(LootBagType::BoostedWhite));
    }

    /// Build a live-feed item with `n` enchantments (rarity derives from count).
    fn item_with_enchants(item_id: i32, n: usize) -> ProcessedLootItem {
        ProcessedLootItem {
            slot: 0,
            item_id,
            item_name: String::new(),
            enchant_ids: (0..n as i32).collect(),
        }
    }

    /// Build a rarity-only filter (type filters off) selecting exactly the given tiers.
    fn rarity_only(none: bool, unc: bool, rare: bool, leg: bool, div: bool) -> PropertyFilters {
        PropertyFilters {
            ut: false,
            st: false,
            shiny: false,
            r_none: none,
            r_uncommon: unc,
            r_rare: rare,
            r_legendary: leg,
            r_divine: div,
        }
    }

    #[test]
    fn test_property_filters_default_is_no_constraint() {
        let prop = PropertyFilters::default();
        assert!(prop.rarity_all());
        assert!(!prop.any());
        assert!(prop.matches_items::<ProcessedLootItem>(&[]));
        assert!(prop.matches_items(&[item_with_enchants(1, 3)]));
    }

    #[test]
    fn test_property_filters_all_on_equals_all_off() {
        let all_on = rarity_only(true, true, true, true, true);
        let all_off = rarity_only(false, false, false, false, false);
        assert!(!all_on.any());
        assert!(!all_off.any());
        let items = vec![item_with_enchants(1, 2)];
        assert!(all_on.matches_items(&items));
        assert!(all_off.matches_items(&items));
    }

    #[test]
    fn test_rarity_filter_maps_enchant_count() {
        let prop = rarity_only(false, false, true, false, false); // Rare = 2 enchants
        assert!(prop.matches_items(&[item_with_enchants(1, 2)]));
        assert!(!prop.matches_items(&[item_with_enchants(1, 1)]));
        assert!(!prop.matches_items(&[item_with_enchants(1, 3)]));

        let prop = rarity_only(false, false, false, false, true);
        assert!(prop.matches_items(&[item_with_enchants(1, 4)]));
        assert!(prop.matches_items(&[item_with_enchants(1, 7)]));
        assert!(!prop.matches_items(&[item_with_enchants(1, 3)]));

        let prop = rarity_only(true, false, false, false, false);
        assert!(prop.matches_items(&[item_with_enchants(1, 0)]));
        assert!(!prop.matches_items(&[item_with_enchants(1, 1)]));
    }

    #[test]
    fn test_rarity_filter_or_within_group() {
        let prop = rarity_only(true, false, false, false, true); // None + Divine
        assert!(prop.matches_items(&[item_with_enchants(1, 0)]));
        assert!(prop.matches_items(&[item_with_enchants(1, 5)]));
        assert!(!prop.matches_items(&[
            item_with_enchants(1, 1),
            item_with_enchants(2, 2),
            item_with_enchants(3, 3),
        ]));
        assert!(prop.matches_items(&[item_with_enchants(1, 1), item_with_enchants(2, 4),]));
    }
}

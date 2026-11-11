//! Live Loot overlay body.
//!
//! Tallies the loot bags that have dropped this session, in the same
//! colour order the Loot tab uses, and lists the drops the Live Feed's toggles
//! let through as a compact row: bag, dungeon and source icons followed by the
//! bag's full contents, as in the Live Feed and Loot History.
//! Boosted bags are counted against their base
//! colour -- a boosted white is still a white -- which is what makes the
//! counters match how players actually talk about a run.

use std::collections::{HashMap, VecDeque};

use eframe::egui::{self, Color32, RichText};
use realmhound_core::loot::{LootBagType, LootDropRecord, ProcessedLootDrop};

use crate::panels::live_feed::LootEntry;
use crate::panels::loot::LootPanel;
use crate::panels::loot_filters::{LootFilters, PropertyFilters};
use crate::rendering::SpriteRenderer;

use super::{
    overlay_tooltip, overlay_tooltip_with, row_text, scrollbar_hit, HitRegions, OverlayContent,
};

/// Side of a bag sprite in the counter strip.
const BAG_ICON: f32 = 18.0;
/// Side of every item tile in a drop row.
const ROW_ICON: f32 = 26.0;
/// Side of the bag, dungeon and source icons. These fill their rect, while an
/// item sprite sits inset in its tile, so they are drawn smaller to match.
const SOURCE_ICON: f32 = 20.0;
/// Hard cap on retained drops. All of them are listed; the scroll area is what
/// bounds how many are on screen at once.
const MAX_RECENT: usize = 100;

/// Display order, each entry pairing a base bag with its boosted variant. Best
/// bags first so the ones worth seeing are never scrolled off.
const BAG_ORDER: &[(LootBagType, LootBagType)] = &[
    (LootBagType::White, LootBagType::BoostedWhite),
    (LootBagType::Orange, LootBagType::BoostedOrange),
    (LootBagType::Red, LootBagType::BoostedRed),
    (LootBagType::Gold, LootBagType::BoostedGold),
    (LootBagType::Egg, LootBagType::BoostedEgg),
    (LootBagType::Blue, LootBagType::BoostedBlue),
    (LootBagType::Teal, LootBagType::BoostedTeal),
    (LootBagType::Purple, LootBagType::BoostedPurple),
    (LootBagType::Pink, LootBagType::BoostedPink),
    (LootBagType::Soulbound, LootBagType::Soulbound),
    (LootBagType::Brown, LootBagType::BoostedBrown),
];

/// One retained drop.
struct RecentDrop {
    bag_type: LootBagType,
    record: LootDropRecord,
}

#[derive(Default)]
pub(super) struct LiveLootOverlay {
    /// Base bag id -> count, boosted variants folded into their base.
    counts: HashMap<i32, u32>,
    recent: VecDeque<RecentDrop>,
    /// Set by the title-bar reset button and consumed on the next render.
    reset_requested: bool,
    /// Bumped whenever `recent` changes, so the cache below can tell it is stale
    /// without comparing the drops themselves.
    revision: u64,
    cache: Option<RowCache>,
}

/// The drop list as rendered, kept across frames.
///
/// Building it runs [`PropertyFilters::matches_items`], which takes the
/// asset manager's lock and does a map lookup per item. Redoing that for every
/// retained drop on every repaint is what this exists to avoid.
struct RowCache {
    bags: LootFilters,
    properties: PropertyFilters,
    revision: u64,
    /// Indices into `recent` of the drops the Live Feed would show.
    rows: Vec<usize>,
}

/// Borrows everything one frame of the Live Loot overlay needs.
pub(super) struct LiveLootContent<'a> {
    pub overlay: &'a mut LiveLootOverlay,
    /// The Live Feed's bag toggles, which the overlay shares rather than
    /// duplicating as a second place to configure the same thing.
    pub filters: &'a LootFilters,
    /// The Live Feed's UT/ST/shiny and rarity toggles, which decide whether a
    /// bag is listed.
    pub properties: &'a PropertyFilters,
}

impl OverlayContent for LiveLootContent<'_> {
    fn title_extras(&mut self, ui: &mut egui::Ui) {
        self.overlay.title_extras(ui);
    }

    fn body(&mut self, ui: &mut egui::Ui, sprites: &mut SpriteRenderer, hits: &mut HitRegions) {
        self.overlay
            .body(ui, sprites, self.filters, self.properties, hits);
    }
}

impl LiveLootOverlay {
    pub(super) fn push(&mut self, drop: &ProcessedLootDrop) {
        let base = base_bag(drop.bag_type);
        *self.counts.entry(base.id()).or_insert(0) += 1;

        self.recent.push_front(RecentDrop {
            bag_type: drop.bag_type,
            record: LootEntry::from_drop(drop).to_drop_record(),
        });
        self.recent.truncate(MAX_RECENT);
        self.revision = self.revision.wrapping_add(1);
    }

    pub(super) fn reset(&mut self) {
        self.counts.clear();
        self.recent.clear();
        self.revision = self.revision.wrapping_add(1);
    }

    /// Title-bar button unique to this overlay: clear the session tallies.
    fn title_extras(&mut self, ui: &mut egui::Ui) {
        let response = ui.add(
            egui::Button::new(
                RichText::new(super::RESET_GLYPH)
                    .size(11.0)
                    .color(Color32::from_gray(150)),
            )
            .frame(false)
            .min_size(egui::vec2(16.0, 0.0)),
        );
        overlay_tooltip(ui, &response, "Reset the session loot tally");
        if response.clicked() {
            self.reset_requested = true;
        }
    }

    pub(super) fn body(
        &mut self,
        ui: &mut egui::Ui,
        sprites: &mut SpriteRenderer,
        filters: &LootFilters,
        properties: &PropertyFilters,
        hits: &mut HitRegions,
    ) {
        if std::mem::take(&mut self.reset_requested) {
            self.reset();
        }

        let visible: Vec<(LootBagType, u32)> = BAG_ORDER
            .iter()
            .filter(|(base, _)| filters.is_visible(*base))
            .filter_map(|(base, _)| {
                self.counts
                    .get(&base.id())
                    .copied()
                    .filter(|&count| count > 0)
                    .map(|count| (*base, count))
            })
            .collect();

        // A hidden bag colour, or a bag with no item passing the item filters,
        // only adds to its counter, exactly as in the Live Feed.
        // Every retained drop is offered, not a fixed handful -- the scroll area
        // is what bounds the list, so a taller window simply shows more.
        self.refresh_rows(filters, properties);
        let Some(cache) = self.cache.as_ref() else {
            return;
        };

        // The counters are the overlay's headline and stay pinned above the
        // list: scrolling the history must never push the tally off screen.
        if !visible.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for (bag, count) in &visible {
                    counter(ui, sprites, *bag, *count);
                }
            });
            ui.add_space(4.0);
            super::divider(ui);
        }

        let output = super::scroll_rows(ui, "loot_history", |ui| {
            if cache.rows.is_empty() {
                // Nothing counted at all reads as "nothing has dropped";
                // bags counted but no rows means the filters hid them.
                let message = if visible.is_empty() {
                    "No drops yet"
                } else {
                    "No items match your filters"
                };
                ui.label(row_text(message, Color32::from_gray(120)));
                return;
            }
            for index in &cache.rows {
                if let Some(drop) = self.recent.get(*index) {
                    LootPanel::render_compact_drop_row(
                        ui,
                        &drop.record,
                        sprites,
                        SOURCE_ICON,
                        |ui, sprites| {
                            for item in &drop.record.items {
                                // The full item breakdown dwarfs the overlay and
                                // cannot leave it, so only the name is shown.
                                let (response, name) = sprites.render_item_tile_plain(
                                    ui,
                                    item.item_id,
                                    Some(&item.item_name),
                                    &item.parsed_enchant_ids,
                                    ROW_ICON,
                                );
                                overlay_tooltip(ui, &response, &name);
                            }
                        },
                    );
                }
            }
        });
        if let Some(bar) = scrollbar_hit(&output) {
            hits.add(bar);
        }
    }

    /// Rebuild [`RowCache`] when the retained drops or either filter set moved.
    fn refresh_rows(&mut self, filters: &LootFilters, properties: &PropertyFilters) {
        let fresh = self.cache.as_ref().is_some_and(|cache| {
            cache.revision == self.revision
                && cache.properties == *properties
                && cache.bags == *filters
        });
        if fresh {
            return;
        }
        let rows = self
            .recent
            .iter()
            .enumerate()
            .filter(|(_, drop)| {
                filters.is_visible(drop.bag_type) && properties.matches_items(&drop.record.items)
            })
            .map(|(index, _)| index)
            .collect();
        self.cache = Some(RowCache {
            bags: filters.clone(),
            properties: *properties,
            revision: self.revision,
            rows,
        });
    }
}

/// A bag sprite with its count, e.g. the white-bag icon followed by `x3`.
fn counter(ui: &mut egui::Ui, sprites: &mut SpriteRenderer, bag: LootBagType, count: u32) {
    let response = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            bag_sprite(ui, sprites, bag.id(), bag, BAG_ICON);
            ui.label(row_text(
                format!("x{count}"),
                Color32::from_rgb(225, 225, 230),
            ));
        })
        .response;
    overlay_tooltip_with(ui, &response, || format!("{}: {count}", bag.name()));
}

/// Draw a bag sprite, falling back to a flat colour chip when the asset is
/// missing so the counter never silently renders as empty space.
fn bag_sprite(
    ui: &mut egui::Ui,
    sprites: &mut SpriteRenderer,
    bag_id: i32,
    bag: LootBagType,
    size: f32,
) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    if sprites.draw_outlined_sprite_in_rect(ui, bag_id, rect) {
        return;
    }
    ui.painter()
        .rect_filled(rect.shrink(2.0), 2.0, fallback_color(bag));
}

/// Fold a boosted bag into the base colour it shares.
fn base_bag(bag: LootBagType) -> LootBagType {
    match bag {
        LootBagType::BoostedWhite => LootBagType::White,
        LootBagType::BoostedOrange => LootBagType::Orange,
        LootBagType::BoostedRed => LootBagType::Red,
        LootBagType::BoostedGold => LootBagType::Gold,
        LootBagType::BoostedEgg => LootBagType::Egg,
        LootBagType::BoostedBlue => LootBagType::Blue,
        LootBagType::BoostedTeal => LootBagType::Teal,
        LootBagType::BoostedPurple => LootBagType::Purple,
        LootBagType::BoostedPink => LootBagType::Pink,
        LootBagType::BoostedBrown => LootBagType::Brown,
        // Soulbound has no boosted variant and is its own colour.
        other => other,
    }
}

fn fallback_color(bag: LootBagType) -> Color32 {
    match base_bag(bag) {
        LootBagType::White => Color32::from_rgb(235, 235, 235),
        LootBagType::Orange => Color32::from_rgb(235, 150, 50),
        LootBagType::Red => Color32::from_rgb(200, 60, 60),
        LootBagType::Gold => Color32::from_rgb(220, 190, 60),
        LootBagType::Egg => Color32::from_rgb(220, 200, 170),
        LootBagType::Blue => Color32::from_rgb(70, 110, 220),
        LootBagType::Teal => Color32::from_rgb(60, 190, 190),
        LootBagType::Purple => Color32::from_rgb(150, 80, 200),
        LootBagType::Pink => Color32::from_rgb(230, 130, 190),
        LootBagType::Brown => Color32::from_rgb(140, 100, 60),
        LootBagType::Soulbound => Color32::from_rgb(190, 110, 220),
        _ => Color32::from_gray(120),
    }
}

#[cfg(test)]
mod tests {
    use super::{base_bag, LiveLootOverlay, LootFilters, PropertyFilters, BAG_ORDER};
    use realmhound_core::loot::{
        LootBagType, PlayerSnapshot, ProcessedLootDrop, ProcessedLootItem,
    };

    fn drop_of(bag: LootBagType, item: &str) -> ProcessedLootDrop {
        ProcessedLootDrop {
            timestamp: 0,
            player: PlayerSnapshot {
                char_id: 1,
                object_id: 1,
                class_id: 0x0300,
                name: "Tester".to_string(),
                skin_id: 0,
                tex1: 0,
                tex2: 0,
                fame: 0,
                is_seasonal: false,
                exalt_bonus: 0,
                loot_drop_active: false,
                loot_tier_active: false,
                dungeon: "Test Dungeon".to_string(),
                map_seed: 0,
            },
            bag_type: bag,
            mob_type: 0,
            mob_name: "Test Boss".to_string(),
            fabricated_attribution: false,
            items: vec![ProcessedLootItem {
                slot: 0,
                item_id: 1,
                item_name: item.to_string(),
                enchant_ids: Vec::new(),
            }],
        }
    }

    #[test]
    fn boosted_bags_fold_into_their_base_colour() {
        assert_eq!(base_bag(LootBagType::BoostedWhite), LootBagType::White);
        assert_eq!(base_bag(LootBagType::BoostedBrown), LootBagType::Brown);
        // Soulbound has no boosted twin and must stay itself.
        assert_eq!(base_bag(LootBagType::Soulbound), LootBagType::Soulbound);
    }

    #[test]
    fn every_ordered_pair_folds_to_its_base() {
        for (base, boosted) in BAG_ORDER {
            assert_eq!(base_bag(*boosted), *base);
            assert_eq!(base_bag(*base), *base);
        }
    }

    #[test]
    fn counters_follow_the_live_feed_bag_toggles() {
        // The overlay has no bag setting of its own; turning a colour off in
        // the Live Feed must hide it here too, for the base and boosted bag
        // alike.
        let mut filters = LootFilters::default();
        assert!(filters.is_visible(LootBagType::White));
        assert!(filters.is_visible(LootBagType::BoostedWhite));

        filters.show_white = false;
        assert!(!filters.is_visible(LootBagType::White));
        assert!(!filters.is_visible(LootBagType::BoostedWhite));
        // Other colours are untouched.
        assert!(filters.is_visible(LootBagType::Red));
    }

    #[test]
    fn drops_are_tallied_even_while_their_colour_is_hidden() {
        // Counting is independent of the filter so re-enabling a colour in the
        // Live Feed reveals the drops that already happened.
        let mut overlay = LiveLootOverlay::default();
        overlay.push(&drop_of(LootBagType::Brown, "Trash"));
        assert_eq!(overlay.counts.get(&LootBagType::Brown.id()), Some(&1));
    }

    #[test]
    fn a_boosted_drop_increments_the_base_counter() {
        let mut overlay = LiveLootOverlay::default();
        overlay.push(&drop_of(LootBagType::White, "Sword"));
        overlay.push(&drop_of(LootBagType::BoostedWhite, "Robe"));
        assert_eq!(overlay.counts.get(&LootBagType::White.id()), Some(&2));
        assert_eq!(overlay.counts.get(&LootBagType::BoostedWhite.id()), None);
    }

    #[test]
    fn recent_drops_are_newest_first_and_bounded() {
        let mut overlay = LiveLootOverlay::default();
        for index in 0..super::MAX_RECENT + 10 {
            overlay.push(&drop_of(LootBagType::Blue, &format!("Item {index}")));
        }
        assert_eq!(overlay.recent.len(), super::MAX_RECENT);
        let newest = overlay.recent.front().expect("a retained drop");
        assert_eq!(
            newest.record.items[0].item_name,
            format!("Item {}", super::MAX_RECENT + 9)
        );
    }

    #[test]
    fn bags_are_listed_whole_when_any_item_passes_the_live_feed_filters() {
        let mut drop = drop_of(LootBagType::White, "Plain");
        drop.items.push(ProcessedLootItem {
            slot: 1,
            item_id: 2,
            item_name: "Enchanted".to_string(),
            enchant_ids: vec![10, 11],
        });
        let mut overlay = LiveLootOverlay::default();
        overlay.push(&drop);
        overlay.push(&drop_of(LootBagType::White, "Other plain"));

        // Rarity narrowed to 2-enchant items only.
        let properties = PropertyFilters {
            r_none: false,
            r_uncommon: false,
            r_rare: true,
            r_legendary: false,
            r_divine: false,
            ..PropertyFilters::default()
        };
        overlay.refresh_rows(&LootFilters::default(), &properties);
        let rows = &overlay.cache.as_ref().expect("a cache").rows;
        assert_eq!(rows, &vec![1], "only the bag holding the enchanted item");
    }

    #[test]
    fn an_overflow_bag_dropped_alongside_the_first_gets_its_own_row() {
        // A boss with more than 8 items drops a second bag at the same instant.
        let mut first = drop_of(LootBagType::White, "Marble Seal");
        first.items.extend((1..8).map(|slot| ProcessedLootItem {
            slot,
            item_id: 100 + slot,
            item_name: format!("Potion {slot}"),
            enchant_ids: Vec::new(),
        }));
        let overflow = drop_of(LootBagType::White, "Overflow item");
        let mut overlay = LiveLootOverlay::default();
        overlay.push(&first);
        overlay.push(&overflow);

        overlay.refresh_rows(&LootFilters::default(), &PropertyFilters::default());
        let rows = &overlay.cache.as_ref().expect("a cache").rows;
        assert_eq!(rows, &vec![0, 1]);
        assert_eq!(overlay.counts.get(&LootBagType::White.id()), Some(&2));
    }

    #[test]
    fn many_rows_stay_inside_the_window_with_a_visible_scrollbar() {
        let _assets = crate::test_support::asset_manager_guard();
        let mut overlay = LiveLootOverlay::default();
        for index in 0..40 {
            overlay.push(&drop_of(LootBagType::White, &format!("Item {index}")));
        }
        let mut sprites = crate::rendering::SpriteRenderer::new();
        let ctx = eframe::egui::Context::default();
        // Mirror the app: dark theme with handle and track in the same grey, and
        // the solid bars the overlay window switches on.
        ctx.style_mut(|style| {
            let grey = eframe::egui::Color32::from_rgb(39, 39, 42);
            style.visuals.extreme_bg_color = grey;
            style.visuals.widgets.inactive.bg_fill = grey;
            style.spacing.scroll = eframe::egui::style::ScrollStyle::solid();
        });
        let screen = eframe::egui::Rect::from_min_size(
            eframe::egui::Pos2::ZERO,
            eframe::egui::vec2(320.0, 180.0),
        );
        let mut used = eframe::egui::Rect::NOTHING;
        let mut shapes = Vec::new();
        for _ in 0..3 {
            let input = eframe::egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            shapes = ctx
                .run(input, |ctx| {
                    eframe::egui::CentralPanel::default().show(ctx, |ui| {
                        let mut hits = super::HitRegions::default();
                        overlay.body(
                            ui,
                            &mut sprites,
                            &LootFilters::default(),
                            &PropertyFilters::default(),
                            &mut hits,
                        );
                        used = ui.min_rect();
                    });
                })
                .shapes;
        }
        assert!(
            used.bottom() <= screen.bottom() + 1.0,
            "rows grew past the window: {used:?}"
        );
        let handle_painted = shapes.iter().any(|clipped| {
            matches!(&clipped.shape, eframe::egui::Shape::Rect(rect) if rect.fill == super::super::SCROLL_HANDLE)
        });
        assert!(handle_painted, "no visible scrollbar handle was painted");
    }

    #[test]
    fn reset_clears_counts_and_history() {
        let mut overlay = LiveLootOverlay::default();
        overlay.push(&drop_of(LootBagType::Red, "Ring"));
        overlay.reset();
        assert!(overlay.counts.is_empty());
        assert!(overlay.recent.is_empty());
    }

    #[test]
    fn the_row_cache_follows_new_drops_and_filter_changes() {
        let bags = LootFilters::default();
        let mut properties = PropertyFilters::default();
        let mut overlay = LiveLootOverlay::default();
        overlay.push(&drop_of(LootBagType::Red, "Ring"));
        overlay.refresh_rows(&bags, &properties);
        assert_eq!(overlay.cache.as_ref().map(|c| c.rows.len()), Some(1));

        // A new drop invalidates the cache.
        overlay.push(&drop_of(LootBagType::Red, "Wand"));
        overlay.refresh_rows(&bags, &properties);
        assert_eq!(overlay.cache.as_ref().map(|c| c.rows.len()), Some(2));

        // So does narrowing the rarity filter past what the drops carry.
        properties.r_none = false;
        overlay.refresh_rows(&bags, &properties);
        assert_eq!(overlay.cache.as_ref().map(|c| c.rows.len()), Some(0));
    }
}

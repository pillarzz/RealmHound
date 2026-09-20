//! Shared pet card widget for rendering pet inventory cards.
//!
//! Used by the Treasury tab to display compact pet cards: pet skin sprite +
//! pet name header, followed by an 8-slot inventory in a 2x4 grid.

use eframe::egui::{self, RichText};
use realmhound_core::assets::get_asset_manager;
use realmhound_core::vault::CachedPet;
use std::collections::HashSet;

use crate::panels::character_card::{CARD_CONTENT_WIDTH, CARD_PADDING, SLOT_SIZE, SLOT_SPACING};
use crate::rendering::SpriteRenderer;
use crate::ui_colors::DIM_OVERLAY;

// ---------------------------------------------------------------------------
// PetCardWidget
// ---------------------------------------------------------------------------

/// Widget for rendering a compact pet inventory card.
///
/// Shows a minimal header (pet skin sprite + pet name) followed by an 8-slot
/// inventory grid in 2 rows of 4. Same card frame style as character cards.
pub struct PetCardWidget;

impl PetCardWidget {
    /// Render a compact pet card: skin + name header, then 2x4 inventory grid.
    ///
    /// Draws the full card including frame. Call this directly -- no wrapping
    /// frame is needed from the caller.
    ///
    /// When `highlight_items` is non-empty, slots matching ANY of those ids get a
    /// colored border.
    pub fn render(
        ui: &mut egui::Ui,
        pet: &CachedPet,
        highlight_items: &HashSet<i32>,
        clickable: bool,
        click_sink: &std::cell::Cell<Option<i32>>,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        let fill = crate::ui_colors::card_fill(ui.visuals());
        let stroke_color = crate::ui_colors::card_stroke(ui.visuals());

        ui.push_id(pet.instance_id, |ui| {
            egui::Frame::NONE
                .fill(fill)
                .stroke(egui::Stroke::new(1.0_f32, stroke_color))
                .corner_radius(8.0)
                .inner_margin(CARD_PADDING)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(SLOT_SPACING, SLOT_SPACING);
                    ui.set_min_width(CARD_CONTENT_WIDTH);
                    ui.set_max_width(CARD_CONTENT_WIDTH);

                    ui.vertical(|ui| {
                        // Compact header: pet sprite + name. The stored `name` is
                        // the stale original-hatch family; a re-skinned pet's real
                        // name is the skin/type sprite's name, so resolve it the
                        // same way the sprite does, falling back to the stored name.
                        ui.horizontal(|ui| {
                            let sprite_id = if pet.skin > 0 { pet.skin } else { pet.pet_type };
                            if sprite_id > 0 {
                                sprite_renderer.render_item_tile(ui, sprite_id, None, &[]);
                                ui.add_space(4.0);
                            }
                            let pet_name = get_asset_manager()
                                .object_name(sprite_id)
                                .filter(|n| !n.is_empty())
                                .unwrap_or_else(|| pet.name.clone());
                            ui.label(RichText::new(&pet_name).strong());
                        });

                        ui.add_space(4.0);
                        ui.separator();
                        ui.add_space(4.0);

                        // 8-slot inventory in 2 rows of 4
                        Self::render_pet_inventory_grid(
                            ui,
                            pet,
                            highlight_items,
                            clickable,
                            click_sink,
                            sprite_renderer,
                        );
                    });
                });
        });
    }

    /// Render the 2x4 pet inventory grid.
    fn render_pet_inventory_grid(
        ui: &mut egui::Ui,
        pet: &CachedPet,
        highlight_items: &HashSet<i32>,
        clickable: bool,
        click_sink: &std::cell::Cell<Option<i32>>,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        // First row: slots 0-3
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = SLOT_SPACING;
            for i in 0..4 {
                Self::render_pet_item_slot(
                    ui,
                    pet,
                    i,
                    highlight_items,
                    clickable,
                    click_sink,
                    sprite_renderer,
                );
            }
        });

        // Second row: slots 4-7
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = SLOT_SPACING;
            for i in 4..8 {
                Self::render_pet_item_slot(
                    ui,
                    pet,
                    i,
                    highlight_items,
                    clickable,
                    click_sink,
                    sprite_renderer,
                );
            }
        });
    }

    /// Render a single pet inventory slot.
    ///
    /// Uses `CachedPetItem` which has `item_id`, `unique_id`, and `stack_count`.
    /// Renders sprite from item_id, shows stack count badge when stack_count > 1.
    /// Tooltip shows item name.
    fn render_pet_item_slot(
        ui: &mut egui::Ui,
        pet: &CachedPet,
        slot_index: usize,
        highlight_items: &HashSet<i32>,
        clickable: bool,
        click_sink: &std::cell::Cell<Option<i32>>,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        if let Some(item) = pet.inventory.get(slot_index) {
            if item.item_id < 0 {
                // Empty slot
                sprite_renderer.render_empty_slot(ui, SLOT_SIZE);
            } else {
                let should_highlight = !highlight_items.is_empty() && {
                    let am = realmhound_core::assets::get_asset_manager();
                    highlight_items
                        .iter()
                        .any(|&id| am.items_match(item.item_id, id))
                };
                let search_active = !highlight_items.is_empty();

                // Render item with stack count support
                let raw_name = sprite_renderer
                    .item_name(item.item_id)
                    .unwrap_or_else(|| format!("Item 0x{:04X}", item.item_id));
                let (item_name, name_stack) = SpriteRenderer::extract_stack_from_name(&raw_name);
                let is_shiny = sprite_renderer.is_shiny(item.item_id);

                let effective_stack = if item.stack_count > 1 {
                    item.stack_count
                } else {
                    name_stack
                };

                // Pet items have no enchants
                let response = sprite_renderer.render_item_slot_ex(
                    ui,
                    item.item_id,
                    &item_name,
                    &[],
                    is_shiny,
                    SLOT_SIZE,
                    effective_stack,
                    should_highlight,
                    clickable,
                );

                if clickable && response.clicked() {
                    click_sink.set(Some(item.item_id));
                }

                if search_active && !should_highlight {
                    ui.painter().rect_filled(response.rect, 4.0, DIM_OVERLAY);
                }
            }
        } else {
            // Slot doesn't exist in inventory data
            sprite_renderer.render_empty_slot(ui, SLOT_SIZE);
        }
    }
}

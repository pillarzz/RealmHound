//! Shared character card widget for rendering character inventory cards.
//!
//! Used by both the Characters tab (full mode with level, fame, badges) and
//! the Treasury tab (compact mode with skin + name only). Extracts shared
//! rendering logic so both tabs stay visually consistent.

use crate::ui_ext::HoverTooltipExt;
use eframe::egui::{self, Color32, RichText, Sense, Vec2};
use realmhound_core::vault::{CachedCharacter, CharacterItem};
use std::collections::HashSet;

use crate::rendering::SpriteRenderer;
use crate::tab_icons::TabIconSprite;
use crate::ui_colors::{DIM_OVERLAY, REGULAR_COLOR, SEASONAL_COLOR};

// ---------------------------------------------------------------------------
// Shared constants
// ---------------------------------------------------------------------------

/// Slot size 38 matches loot panel's TILE_SIZE for consistent item rendering.
/// Inner rect = 38 - 6 shrink = 32×32 pixels.
pub const SLOT_SIZE: f32 = 38.0;
pub const SLOT_SPACING: f32 = 4.0;

/// Content width = 4 slots * 38 + 3 gaps * 4 = 152 + 12 = 164
pub const CARD_CONTENT_WIDTH: f32 = 164.0;
pub const CARD_PADDING: f32 = 8.0;
pub const CARD_SPACING: f32 = 4.0;

/// Icon size for graveyard card icons (killer, grave, fame), matching the
/// loot-history filter pill icon size (16x16).
const GRAVEYARD_ICON_SIZE: f32 = 16.0;

/// Object ID for the "SuperFame" sprite used as the fame icon (orange).
pub const FAME_SPRITE_ID: i32 = 24102;

/// Object ID for the "Gold Shop Icon Object" sprite used as the gold coin icon.
pub const GOLD_SPRITE_ID: i32 = 0x67FF;

/// Draw a small red "CRUCIBLE" pill, roughly half the size of the Missions-tab
/// badge. Used inline on the character info line to mark crucible characters.
fn draw_crucible_pill(ui: &mut egui::Ui) {
    const TEXT: &str = "CRUCIBLE";
    let font = egui::FontId::proportional(8.0);
    let galley = ui
        .painter()
        .layout_no_wrap(TEXT.to_string(), font.clone(), Color32::WHITE);
    let size = galley.size() + egui::vec2(6.0, 2.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_filled(rect, 2.0, Color32::from_rgb(0xdc, 0x21, 0x15));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        TEXT,
        font,
        Color32::WHITE,
    );
}

/// Format a fame value with a comma every three digits (e.g. `280087` ->
/// `280,087`).
pub fn format_fame(value: i64) -> String {
    let negative = value < 0;
    let digits = value.unsigned_abs().to_string();
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if negative {
        format!("-{}", out)
    } else {
        out
    }
}

/// Format a loot-drop-boost countdown. Under one hour it reads `Mm Ss`
/// (minutes + seconds); at or above one hour it rolls to `Hh Mm`.
pub fn format_loot_boost(remaining_secs: u64) -> String {
    if remaining_secs >= 3600 {
        let h = remaining_secs / 3600;
        let m = (remaining_secs % 3600) / 60;
        format!("{}h {}m", h, m)
    } else {
        let m = remaining_secs / 60;
        let s = remaining_secs % 60;
        format!("{}m {}s", m, s)
    }
}

/// Draw the fame icon (sprite 24102) into a `size`×`size` cell. When `faded` is
/// true the icon is drawn semi-transparent (for potential/estimated values).
/// Adds trailing spacing when the icon is drawn.
pub fn render_fame_icon(
    ui: &mut egui::Ui,
    sprite_renderer: &mut SpriteRenderer,
    size: f32,
    faded: bool,
) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    if ui.is_rect_visible(rect) {
        let drawn = if faded {
            sprite_renderer.draw_sprite_in_rect_tinted(
                ui,
                FAME_SPRITE_ID,
                rect,
                Color32::from_white_alpha(110),
            )
        } else {
            sprite_renderer.draw_sprite_in_rect(ui, FAME_SPRITE_ID, rect)
        };
        if drawn {
            ui.add_space(3.0);
        }
    }
}

// ---------------------------------------------------------------------------
// CharacterCardWidget
// ---------------------------------------------------------------------------

/// Shared widget for rendering a single character card.
///
/// When `compact` is true, the header shows only the skin sprite and display
/// name (no level, maxed count, fame, LIVE/DEAD badges, or death info).
/// When `compact` is false, the full header is rendered (used by Characters tab).
/// How the "stats maxed" section renders each stat value (toolbar).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatDisplay {
    /// Don't render the stats-maxed section at all.
    #[default]
    None,
    /// Show the current stat value.
    Current,
    /// Show potions left to max (`cap - current`, clamped at 0).
    LeftToMax,
    /// Show current value with left-to-max in parentheses: `value (left)`.
    CurrentLeftToMax,
}

impl StatDisplay {
    /// Serialize to the settings string key.
    pub fn as_key(self) -> &'static str {
        match self {
            StatDisplay::None => "none",
            StatDisplay::Current => "current",
            StatDisplay::LeftToMax => "left",
            StatDisplay::CurrentLeftToMax => "current_left",
        }
    }

    /// Parse from a settings string key (defaults to `None`).
    pub fn from_key(key: &str) -> Self {
        match key {
            "current" => StatDisplay::Current,
            "left" => StatDisplay::LeftToMax,
            "current_left" => StatDisplay::CurrentLeftToMax,
            _ => StatDisplay::None,
        }
    }

    /// Short label for the toolbar dropdown.
    pub fn label(self) -> &'static str {
        match self {
            StatDisplay::None => "None",
            StatDisplay::Current => "Current",
            StatDisplay::LeftToMax => "Left to max",
            StatDisplay::CurrentLeftToMax => "Current (Left to max)",
        }
    }
}

pub struct CharacterCardWidget {
    /// Whether to render a compact header (skin + name only).
    pub compact: bool,
    /// When set, item slots matching ANY of these ids get a highlight border.
    pub highlight_items: HashSet<i32>,
    /// Graveyard mode: render a grave sprite, show base + dead fame, and
    /// suppress the "💀 DEAD" badge (every card in the Graveyard tab is dead).
    pub is_graveyard: bool,
    /// Grave sprite to render in graveyard mode (matching maxed stats at death).
    pub grave_sprite: Option<TabIconSprite>,
    /// When true, non-empty item slots respond to clicks (for click-to-filter).
    pub clickable: bool,
    /// Stats-maxed section display mode.
    pub stat_display: StatDisplay,
    /// Equipped pet `(name, skin object id)` to render in the header line.
    pub pet: Option<(String, i32)>,
    /// Remaining loot-drop-boost seconds (server-driven, already paused by the
    /// game in safe areas/loading). Only rendered on the live card; `None`
    /// hides the boost line.
    pub loot_boost_secs: Option<u32>,
    /// Whether to render the main inventory section.
    pub show_inventory: bool,
    /// Whether to render the backpack section.
    pub show_backpack: bool,
    /// Whether to render the backpack-extender section.
    pub show_extender: bool,
    /// Whether to render the potion-belt section.
    pub show_belt: bool,
    /// Records the id of a clicked item slot during a render pass.
    clicked: std::cell::Cell<Option<i32>>,
}

impl CharacterCardWidget {
    /// Create a new widget in full mode (all header details shown).
    pub fn full() -> Self {
        Self {
            compact: false,
            highlight_items: HashSet::new(),
            is_graveyard: false,
            grave_sprite: None,
            clickable: false,
            stat_display: StatDisplay::None,
            pet: None,
            loot_boost_secs: None,
            show_inventory: true,
            show_backpack: true,
            show_extender: true,
            show_belt: true,
            clicked: std::cell::Cell::new(None),
        }
    }

    /// Create a new widget in compact mode (skin + name only).
    pub fn compact() -> Self {
        Self {
            compact: true,
            highlight_items: HashSet::new(),
            is_graveyard: false,
            grave_sprite: None,
            clickable: false,
            stat_display: StatDisplay::None,
            pet: None,
            loot_boost_secs: None,
            show_inventory: true,
            show_backpack: true,
            show_extender: true,
            show_belt: true,
            clicked: std::cell::Cell::new(None),
        }
    }

    /// Create a full-mode widget for the Graveyard tab.
    ///
    /// Renders the matching grave sprite and suppresses the DEAD badge/red
    /// styling, since every card in the Graveyard tab is already dead.
    pub fn full_graveyard(grave_sprite: TabIconSprite) -> Self {
        Self {
            compact: false,
            highlight_items: HashSet::new(),
            is_graveyard: true,
            grave_sprite: Some(grave_sprite),
            clickable: false,
            stat_display: StatDisplay::None,
            pet: None,
            loot_boost_secs: None,
            show_inventory: true,
            show_backpack: true,
            show_extender: true,
            show_belt: true,
            clicked: std::cell::Cell::new(None),
        }
    }

    /// Set multiple item ids to highlight in all item slots.
    pub fn with_highlights(mut self, item_ids: &HashSet<i32>) -> Self {
        self.highlight_items = item_ids.clone();
        self
    }

    /// Make non-empty item slots clickable so a click can drive item filtering.
    pub fn clickable(mut self) -> Self {
        self.clickable = true;
        self
    }

    /// Set the stats-maxed section display mode.
    pub fn with_stat_display(mut self, mode: StatDisplay) -> Self {
        self.stat_display = mode;
        self
    }

    /// Set the equipped pet `(name, skin)` shown in the header line.
    pub fn with_pet(mut self, pet: Option<(String, i32)>) -> Self {
        self.pet = pet;
        self
    }

    /// Set the live loot-drop-boost remaining seconds shown on the live card.
    pub fn with_loot_boost(mut self, loot_boost_secs: Option<u32>) -> Self {
        self.loot_boost_secs = loot_boost_secs;
        self
    }

    /// Configure which inventory sections are shown (toggles).
    pub fn with_sections(
        mut self,
        inventory: bool,
        backpack: bool,
        extender: bool,
        belt: bool,
    ) -> Self {
        self.show_inventory = inventory;
        self.show_backpack = backpack;
        self.show_extender = extender;
        self.show_belt = belt;
        self
    }

    // -----------------------------------------------------------------------
    // Item slot rendering
    // -----------------------------------------------------------------------

    /// Render a single item slot with optional stack count.
    pub fn render_item_slot(
        &self,
        ui: &mut egui::Ui,
        item: &CharacterItem,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        if item.is_empty() {
            sprite_renderer.render_empty_slot(ui, SLOT_SIZE);
        } else {
            let search_active = !self.highlight_items.is_empty();
            let should_highlight = search_active && {
                let am = realmhound_core::assets::get_asset_manager();
                self.highlight_items
                    .iter()
                    .any(|&id| am.items_match(item.item_id, id))
            };

            let raw_item_name = sprite_renderer
                .item_name(item.item_id)
                .unwrap_or_else(|| format!("Item 0x{:04X}", item.item_id));

            // Extract stack count from item name (e.g., "Shard of the Advisor x32" -> 32)
            let (item_name, name_stack_count) =
                SpriteRenderer::extract_stack_from_name(&raw_item_name);
            let is_shiny = sprite_renderer.is_shiny(item.item_id);

            // Use explicit stack_count if set, otherwise use name-based stack count
            let effective_stack_count = if item.stack_count > 1 {
                item.stack_count
            } else {
                name_stack_count
            };

            // Build the enchant-slot buffer for the renderer: filled enchant
            // ids first, then empty (-1) and locked (-2) slots. This drives the
            // gem tier (by total slot count) and the tooltip, so an item with
            // slots but no enchants still renders as slotted with "Empty".
            let mut enchant_buf: [i32; 4] = [0; 4];
            let mut slot_count = 0;
            let mut push_slot = |value: i32, count: &mut usize| {
                if *count < 4 {
                    enchant_buf[*count] = value;
                    *count += 1;
                }
            };
            for &e in item.enchant_ids.iter() {
                push_slot(e as i32, &mut slot_count);
            }
            for _ in 0..item.empty_enchant_slots {
                push_slot(-1, &mut slot_count);
            }
            for _ in 0..item.locked_enchant_slots {
                push_slot(-2, &mut slot_count);
            }
            let enchant_ids = &enchant_buf[..slot_count];

            let response = sprite_renderer.render_item_slot_ex(
                ui,
                item.item_id,
                &item_name,
                enchant_ids,
                is_shiny,
                SLOT_SIZE,
                effective_stack_count,
                should_highlight,
                self.clickable,
            );

            if self.clickable && response.clicked() {
                self.clicked.set(Some(item.item_id));
            }

            // Dim non-matching items so the highlighted match stands out.
            if search_active && !should_highlight {
                ui.painter().rect_filled(response.rect, 4.0, DIM_OVERLAY);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Equipment row (4 slots)
    // -----------------------------------------------------------------------

    /// Render equipment row (4 slots: Weapon, Ability, Armor, Ring).
    pub fn render_equipment_row(
        &self,
        ui: &mut egui::Ui,
        char: &CachedCharacter,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        ui.horizontal(|ui| {
            let items: Vec<_> = char.equipment.iter().take(4).collect();
            for (i, item) in items.iter().enumerate() {
                self.render_item_slot(ui, item, sprite_renderer);
                if i < items.len() - 1 {
                    ui.add_space(SLOT_SPACING);
                }
            }
        });
    }

    // -----------------------------------------------------------------------
    // Inventory row (8 slots in 2 rows of 4)
    // -----------------------------------------------------------------------

    /// Render an inventory section (8 slots in 2 rows of 4).
    /// If `available` is false, renders greyed-out unavailable slots.
    pub fn render_inventory_row(
        &self,
        ui: &mut egui::Ui,
        _label: &str,
        items: &[CharacterItem],
        available: bool,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        let empty = CharacterItem::empty();
        ui.vertical(|ui| {
            // First row: slots 1-4 (indices 0-3)
            ui.horizontal(|ui| {
                for i in 0..4 {
                    let item = items.get(i);
                    if available {
                        self.render_item_slot(ui, item.unwrap_or(&empty), sprite_renderer);
                    } else {
                        sprite_renderer.render_unavailable_slot(ui, SLOT_SIZE);
                    }
                    if i < 3 {
                        ui.add_space(SLOT_SPACING);
                    }
                }
            });

            // Second row: slots 5-8 (indices 4-7)
            ui.horizontal(|ui| {
                for i in 4..8 {
                    let item = items.get(i);
                    if available {
                        self.render_item_slot(ui, item.unwrap_or(&empty), sprite_renderer);
                    } else {
                        sprite_renderer.render_unavailable_slot(ui, SLOT_SIZE);
                    }
                    if i < 7 {
                        ui.add_space(SLOT_SPACING);
                    }
                }
            });
        });
    }

    // -----------------------------------------------------------------------
    // Belt row (2-3 quickslots)
    // -----------------------------------------------------------------------

    /// Render belt row (always 2 slots, plus optional 3rd if has_3_quickslots).
    pub fn render_belt_row(
        &self,
        ui: &mut egui::Ui,
        belt: &[CharacterItem],
        show_third_slot: bool,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        let empty = CharacterItem::empty();
        ui.horizontal(|ui| {
            // Always show 2 slots (base quick slots)
            for i in 0..2 {
                let item = belt.get(i).unwrap_or(&empty);
                self.render_item_slot(ui, item, sprite_renderer);
                ui.add_space(SLOT_SPACING);
            }
            // Show 3rd slot only if adventurer's belt equipped
            if show_third_slot {
                let item = belt.get(2).unwrap_or(&empty);
                self.render_item_slot(ui, item, sprite_renderer);
            }
        });
    }

    // -----------------------------------------------------------------------
    // Card header
    // -----------------------------------------------------------------------

    /// Render a single row: an optional sprite icon (loot-filter sized)
    /// followed by a text label, used for graveyard card detail rows.
    fn icon_label_row(
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        icon_id: Option<i32>,
        outlined: bool,
        label: RichText,
    ) {
        ui.horizontal(|ui| {
            if let Some(id) = icon_id {
                let (rect, _) =
                    ui.allocate_exact_size(Vec2::splat(GRAVEYARD_ICON_SIZE), Sense::hover());
                let drawn = if ui.is_rect_visible(rect) {
                    // Use the same outlined-sprite path as loot history for mob
                    // (killer) icons so they render identically.
                    if outlined {
                        sprite_renderer.draw_outlined_sprite_in_rect(ui, id, rect)
                    } else {
                        sprite_renderer.render_icon(ui, Some(TabIconSprite::ObjectId(id)), rect)
                    }
                } else {
                    false
                };
                if drawn {
                    ui.add_space(4.0);
                }
            }
            ui.label(label);
        });
    }

    /// Render the card header.
    ///
    /// In compact mode: skin sprite + display name only.
    /// In full mode: skin sprite + name + level + maxed + fame + LIVE/DEAD badges.
    pub fn render_card_header(
        &self,
        ui: &mut egui::Ui,
        char: &CachedCharacter,
        is_live: bool,
        custom_label: Option<&str>,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        ui.horizontal(|ui| {
            let skin_size = 32.0;
            // Left column: character skin sprite with the equipped pet directly
            // beneath it.
            ui.vertical(|ui| {
            // Character skin sprite.
            let (rect, _response) =
                ui.allocate_exact_size(Vec2::new(skin_size, skin_size), Sense::hover());

            if ui.is_rect_visible(rect) {
                let skin_id = if char.skin > 0 {
                    char.skin
                } else {
                    char.class_id as i32
                };

                if !sprite_renderer.draw_dyed_outlined_character_sprite(
                    ui, skin_id, rect, 6, char.tex1, char.tex2,
                ) {
                    // Fallback: draw class initial
                    let initial = char.class_name().chars().next().unwrap_or('?');
                    ui.painter()
                        .rect_filled(rect, 4.0, crate::ui_colors::sprite_fallback_fill(ui.visuals()));
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        initial,
                        egui::FontId::proportional(16.0),
                        Color32::WHITE,
                    );
                }
            }

            // Equipped pet sprite directly under the character icon.
            if !self.is_graveyard {
                if let Some((pet_name, pet_skin)) = &self.pet {
                    ui.add_space(2.0);
                    let (area, pet_resp) =
                        ui.allocate_exact_size(Vec2::new(skin_size, 18.0), Sense::hover());
                    let pet_rect =
                        egui::Rect::from_center_size(area.center(), Vec2::splat(18.0));
                    sprite_renderer.draw_outlined_sprite_in_rect(ui, *pet_skin, pet_rect);
                    pet_resp.hover_tip(pet_name);
                }
            }
            });

            ui.add_space(8.0);

            if self.compact {
                // Compact mode: name on line 1, crucible pill on line 2 (stacked
                // in a vertical so a long 20-char name can't stretch the card).
                // Small top/bottom padding keeps the two lines within the 32px
                // sprite box.
                let display_name = custom_label.unwrap_or(char.class_name());
                let name_color = if char.seasonal { SEASONAL_COLOR } else { REGULAR_COLOR };
                ui.vertical(|ui| {
                    ui.add_space(2.0);
                    ui.label(RichText::new(display_name).strong().size(14.0).color(name_color));
                    if char.crucible_active {
                        draw_crucible_pill(ui);
                        ui.add_space(2.0);
                    }
                });
            } else {
                // Full mode: name, badges, level, maxed, fame
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    // Label and badges
                    ui.horizontal(|ui| {
                        let display_name = custom_label.unwrap_or(char.class_name());
                        let name_color = if char.seasonal { SEASONAL_COLOR } else { REGULAR_COLOR };
                        ui.label(RichText::new(display_name).strong().size(14.0).color(name_color));

                        // A live character is flagged with a small green dot +
                        // LIVE right after the name. (The old lightning glyph was
                        // borrowed from Muledump, where it actually means an
                        // active loot boost, not "currently playing".)
                        if is_live {
                            ui.label(
                                RichText::new("\u{25cf} LIVE")
                                    .small()
                                    .color(Color32::from_rgb(100, 255, 100)),
                            );
                        }

                        if char.is_dead && !self.is_graveyard {
                            ui.label(
                                RichText::new("💀 DEAD")
                                    .small()
                                    .color(Color32::from_rgb(255, 80, 80)),
                            );
                        }
                    });

                    if !self.is_graveyard {
                        // Death info (if dead)
                        if char.is_dead {
                            ui.horizontal(|ui| {
                                let killed_by = char.killed_by.as_deref().unwrap_or("Unknown");
                                ui.label(
                                    RichText::new(format!("Killed by: {}", killed_by))
                                        .small()
                                        .color(Color32::from_rgb(255, 120, 120)),
                                );
                                if char.death_fame > 0 {
                                    render_fame_icon(ui, sprite_renderer, 13.0, false);
                                    ui.label(
                                        RichText::new(format_fame(char.death_fame as i64))
                                            .small()
                                            .color(Color32::from_rgb(255, 200, 100)),
                                    );
                                }
                            });
                        }

                        // Fame + crucible pill on their own row (row 2). Fame
                        // is always shown (0 when none) so the card height stays
                        // consistent and a long value can't stretch the card.
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            render_fame_icon(ui, sprite_renderer, 13.0, false);
                            ui.add_space(-2.0);
                            ui.label(
                                RichText::new(format_fame(char.fame))
                                    .size(12.5)
                                    .color(Color32::from_rgb(255, 200, 100)),
                            );
                            if char.crucible_active {
                                ui.add_space(6.0);
                                draw_crucible_pill(ui);
                            }
                        });

                        // Id, level, maxed - dot-separated (row 3).
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("#{} • Lv.{}", char.char_id, char.level))
                                    .small()
                                    .color(Color32::LIGHT_GRAY),
                            );

                            ui.label(RichText::new("•").small().color(Color32::GRAY));

                            let maxed_color = match char.maxed_count {
                                8 => Color32::from_rgb(255, 215, 0),
                                7 => Color32::from_rgb(200, 200, 255),
                                6 => Color32::from_rgb(150, 150, 255),
                                _ => Color32::LIGHT_GRAY,
                            };
                            ui.label(
                                RichText::new(format!("{}/8", char.maxed_count))
                                    .small()
                                    .color(maxed_color),
                            );
                        });

                        // Loot-drop boost row (row 4, classic ⚡). Always
                        // reserved -- even when empty -- so cards without an
                        // active boost keep the same height as boosted ones.
                        // The remaining seconds are server-driven, so the timer
                        // naturally freezes when the game pauses it
                        // (Nexus/vault/loading/offline). The panel pins the
                        // value to the boosted character, so its card keeps
                        // showing the last-known timer even when not live.
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            match self.loot_boost_secs {
                                Some(secs) => {
                                    ui.label(
                                        RichText::new(format!(
                                            "\u{26a1} {}",
                                            format_loot_boost(secs as u64)
                                        ))
                                        .small()
                                        .color(Color32::from_rgb(100, 255, 100)),
                                    )
                                    .on_hover_text(
                                        "Loot-drop boost remaining. Pauses in safe areas (Nexus, vault, etc.) and while loading.",
                                    );
                                    if is_live {
                                        ui.ctx().request_repaint_after(
                                            std::time::Duration::from_millis(500),
                                        );
                                    }
                                }
                                None => {
                                    // Reserve the line height with an empty
                                    // small label so the card stays as tall.
                                    ui.label(RichText::new(" ").small());
                                }
                            }
                        });
                    }
                });
            }
        });

        // Graveyard rows stacked full-width under the sprite + name row.
        if self.is_graveyard && !self.compact {
            ui.add_space(2.0);

            // Line: gravestone + level + maxed.
            ui.horizontal(|ui| {
                if let Some(grave) = self.grave_sprite {
                    let (grave_rect, _) =
                        ui.allocate_exact_size(Vec2::splat(GRAVEYARD_ICON_SIZE), Sense::hover());
                    if ui.is_rect_visible(grave_rect) {
                        sprite_renderer.render_icon(ui, Some(grave), grave_rect);
                    }
                    ui.add_space(4.0);
                }
                ui.label(
                    RichText::new(format!("Lv.{}", char.level))
                        .small()
                        .color(Color32::LIGHT_GRAY),
                );
                let maxed_color = match char.maxed_count {
                    8 => Color32::from_rgb(255, 215, 0),
                    7 => Color32::from_rgb(200, 200, 255),
                    6 => Color32::from_rgb(150, 150, 255),
                    _ => Color32::LIGHT_GRAY,
                };
                ui.label(
                    RichText::new(format!("{}/8", char.maxed_count))
                        .small()
                        .color(maxed_color),
                );
            });

            // Line: base fame.
            Self::icon_label_row(
                ui,
                sprite_renderer,
                Some(FAME_SPRITE_ID),
                false,
                RichText::new(format!("Base fame: {}", format_fame(char.fame)))
                    .small()
                    .color(Color32::from_rgb(255, 200, 100)),
            );

            // Line: death fame.
            Self::icon_label_row(
                ui,
                sprite_renderer,
                Some(FAME_SPRITE_ID),
                false,
                RichText::new(format!(
                    "Death fame: {}",
                    format_fame(char.death_fame as i64)
                ))
                .small()
                .color(Color32::from_rgb(255, 200, 100)),
            );

            // Line: killer.
            let killed_by = char.killed_by.as_deref().unwrap_or("Unknown");
            let killer_id =
                realmhound_core::assets::get_asset_manager().killer_sprite_id(killed_by);
            Self::icon_label_row(
                ui,
                sprite_renderer,
                killer_id,
                true,
                RichText::new(format!("Killed by: {}", killed_by))
                    .small()
                    .color(Color32::WHITE),
            );
        }
    }

    // -----------------------------------------------------------------------
    // Full card rendering
    // -----------------------------------------------------------------------

    /// Render the entire character card contents (header + inventory rows).
    ///
    /// This draws only the *inner content* of a card. The caller is
    /// responsible for the card frame, drag-and-drop, and context menus.
    pub fn render(
        &self,
        ui: &mut egui::Ui,
        char: &CachedCharacter,
        is_live: bool,
        custom_label: Option<&str>,
        sprite_renderer: &mut SpriteRenderer,
    ) -> Option<i32> {
        self.clicked.set(None);

        // Card text is never meant to be selectable; disabling it prevents egui
        // from highlighting labels (name, level, fame) while a card is dragged.
        ui.style_mut().interaction.selectable_labels = false;

        // Header row
        self.render_card_header(ui, char, is_live, custom_label, sprite_renderer);

        ui.add_space(2.0);
        ui.separator();
        ui.add_space(4.0);

        // Stats-maxed section - between header and equipment.
        self.render_stats_maxed(ui, char, sprite_renderer);

        // Equipment row (4 slots)
        self.render_equipment_row(ui, char, sprite_renderer);
        ui.add_space(4.0);

        // Main inventory (2 rows of 4)
        if self.show_inventory {
            self.render_inventory_row(ui, "Inventory", &char.inventory, true, sprite_renderer);
        }

        // Backpack - always show, greyed out if unavailable
        if self.show_backpack {
            ui.add_space(4.0);
            self.render_inventory_row(
                ui,
                "Backpack",
                &char.backpack,
                char.has_backpack,
                sprite_renderer,
            );
        }

        // Backpack extender - always show, greyed out if unavailable
        if self.show_extender {
            ui.add_space(4.0);
            self.render_inventory_row(
                ui,
                "Extender",
                &char.backpack_ext,
                char.has_extender,
                sprite_renderer,
            );
        }

        // Belt - always show 2 slots, 3rd only if has_3_quickslots
        if self.show_belt {
            ui.add_space(4.0);
            self.render_belt_row(ui, &char.belt, char.has_3_quickslots, sprite_renderer);
        }

        self.clicked.get()
    }

    /// Render the "stats maxed" section: 8 stats in two columns,
    /// bounded by top and bottom dividers. Maxed stats are yellow, unmaxed
    /// green. The value shown depends on the active [`StatDisplay`] mode.
    fn render_stats_maxed(
        &self,
        ui: &mut egui::Ui,
        char: &CachedCharacter,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        if self.stat_display == StatDisplay::None || self.compact {
            return;
        }

        let details = char.stat_details();
        let maxed = Color32::from_rgb(255, 215, 0);
        let unmaxed = Color32::from_rgb(100, 220, 100);
        let bracket = Color32::from_rgb(150, 150, 150);
        let display = self.stat_display;

        ui.add_space(2.0);
        // Two equal-width columns (via `ui.columns`) so each column's potion
        // icons line up vertically and there's equal empty space to the right of
        // both columns, regardless of number width.
        let icon_size = 16.0;
        let draw_line =
            |ui: &mut egui::Ui, sr: &mut SpriteRenderer, entry: (&'static str, i32, i32, bool)| {
                let (label, current, cap, is_maxed) = entry;
                let deficit = (cap - current).max(0);
                let left = if label == "HP" || label == "MP" {
                    (deficit + 4) / 5
                } else {
                    deficit
                };
                let value_color = if is_maxed { maxed } else { unmaxed };
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(icon_size), Sense::hover());
                    if let Some(id) = stat_potion_id(label) {
                        sr.draw_outlined_sprite_in_rect(ui, id, rect);
                    }
                    ui.add_space(4.0);
                    match display {
                        StatDisplay::Current => {
                            ui.label(
                                RichText::new(current.to_string())
                                    .size(14.0)
                                    .color(value_color),
                            );
                        }
                        StatDisplay::LeftToMax => {
                            ui.label(
                                RichText::new(left.to_string())
                                    .size(14.0)
                                    .color(value_color),
                            );
                        }
                        StatDisplay::CurrentLeftToMax => {
                            ui.label(
                                RichText::new(current.to_string())
                                    .size(14.0)
                                    .color(value_color),
                            );
                            ui.label(
                                RichText::new(format!(" ({})", left))
                                    .size(14.0)
                                    .color(bracket),
                            );
                        }
                        StatDisplay::None => {}
                    }
                });
            };
        ui.columns(2, |cols| {
            for row in 0..4 {
                draw_line(&mut cols[0], sprite_renderer, details[row * 2]);
                draw_line(&mut cols[1], sprite_renderer, details[row * 2 + 1]);
            }
        });
        ui.add_space(2.0);
        ui.separator();
        ui.add_space(4.0);
    }
}

/// Object id of the stat potion matching a stat label, resolved by name at
/// runtime (asset ids aren't fixed across builds). Used to draw a potion icon
/// next to each stat in the card's maxed-stats section.
pub(crate) fn stat_potion_id(label: &str) -> Option<i32> {
    let name = match label {
        "HP" => "Potion of Life",
        "MP" => "Potion of Mana",
        "ATT" => "Potion of Attack",
        "DEF" => "Potion of Defense",
        "SPD" => "Potion of Speed",
        "DEX" => "Potion of Dexterity",
        "VIT" => "Potion of Vitality",
        "WIS" => "Potion of Wisdom",
        _ => return None,
    };
    let mgr = realmhound_core::assets::get_asset_manager();
    mgr.object_id_for_name(name)
        .or_else(|| mgr.object_id_for_display_name(name))
}

#[cfg(test)]
mod tests {
    use super::format_loot_boost;

    #[test]
    fn loot_boost_uses_minutes_seconds_under_one_hour() {
        assert_eq!(format_loot_boost(0), "0m 0s");
        assert_eq!(format_loot_boost(5), "0m 5s");
        assert_eq!(format_loot_boost(65), "1m 5s");
        assert_eq!(format_loot_boost(3599), "59m 59s");
    }

    #[test]
    fn loot_boost_rolls_to_hours_minutes_at_one_hour() {
        assert_eq!(format_loot_boost(3600), "1h 0m");
        assert_eq!(format_loot_boost(3660), "1h 1m");
        assert_eq!(format_loot_boost(7325), "2h 2m");
    }
}

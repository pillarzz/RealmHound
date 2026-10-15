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

/// The lightning glyph and green the card uses for its loot-boost readout.
const LOOT_BOOST_GLYPH: &str = "\u{26a1}";
const LOOT_BOOST_COLOR: Color32 = Color32::from_rgb(100, 255, 100);
/// Width of the painted loot-boost readout, used only for its hover target: the
/// readout is painted, never laid out, so it cannot affect anything else.
/// The slot is the run from the readout's left edge to the id column.
const LOOT_SLOT_W: f32 = CARD_TEXT_INDENT - LOOT_BOOST_LEFT;
/// Distance from the card's content edge to its text column: the 32px sprite box
/// plus the gap after it. The loot readout is painted back at the content edge so
/// it sits under the sprite, and this is the only place that offset is used -- as
/// a paint coordinate, not as layout spacing.
const CARD_TEXT_INDENT: f32 = 40.0;
/// Offset of the duration text past the painted glyph.
const LOOT_GLYPH_W: f32 = 12.0;
/// The loot readout's left edge, this far left of the card's content edge, so the
/// pair sits under the sprite.
///
/// Anchoring it to the content edge rather than to the text column is what keeps
/// a long timer clear of the id line: the column is 40px in from that edge, which
/// leaves the readout 51px to grow into, and `⚡10:45:48` measures 48.6px. The
/// card's side padding is what the 11px of the offset sits in.
const LOOT_BOOST_LEFT: f32 = -11.0;

/// The name row's font size and item spacing. Pinned as constants because the
/// row's width cap is computed from them: the cap has to hold for the same
/// metrics the text is drawn with.
const NAME_FONT_SIZE: f32 = 13.0;
const NAME_ROW_SPACING: f32 = 4.0;

/// Width of `text` laid out as a single line at `font`. Used by the rows that
/// have to fit a fixed card width, so the cap is computed from the real font
/// rather than from a guess at how wide the text is.
fn text_width(ui: &egui::Ui, text: &str, font: egui::FontId) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_string(), font, Color32::WHITE)
        .size()
        .x
}

/// Draw a small pill, roughly half the size of the Missions-tab badge. The text
/// gets a 1px black outline so it stays legible over any card background.
fn draw_pill(ui: &mut egui::Ui, text: &str, background: Color32) {
    let font = egui::FontId::proportional(8.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE);
    let size = galley.size() + egui::vec2(6.0, 2.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, background);
    for offset in [
        egui::vec2(-1.0, 0.0),
        egui::vec2(1.0, 0.0),
        egui::vec2(0.0, -1.0),
        egui::vec2(0.0, 1.0),
    ] {
        painter.text(
            rect.center() + offset,
            egui::Align2::CENTER_CENTER,
            text,
            font.clone(),
            Color32::BLACK,
        );
    }
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        font,
        Color32::WHITE,
    );
}

/// Allocate a pill's exact footprint without drawing anything, so a card with no
/// challenges reserves precisely the row a card with them occupies.
fn draw_pill_invisible(ui: &mut egui::Ui, text: &str) {
    let font = egui::FontId::proportional(8.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font, Color32::WHITE);
    ui.allocate_exact_size(galley.size() + egui::vec2(6.0, 2.0), Sense::hover());
}

/// Crucible marker on the character card.
fn draw_crucible_pill(ui: &mut egui::Ui) {
    draw_pill(ui, "CRUCIBLE", Color32::from_rgb(0xdc, 0x21, 0x15));
}

/// Blood Ritual marker, in the ritual panel's own orange so it reads apart from
/// the crucible's red at a glance.
fn draw_blood_ritual_pill(ui: &mut egui::Ui) {
    draw_pill(ui, "BLOOD RITUAL", Color32::from_rgb(0xe7, 0x7b, 0x00));
}

/// The season-challenge pills a character carries, on one line.
///
/// With `reserve` set, a character carrying no challenge still draws an
/// *invisible* pill, so the row is the same height as one holding two. Going
/// through the same allocation is the only exact way to do it: a spacer is laid
/// out slightly differently (the vertical spacing between rows is applied to a
/// spacer and a pill differently), which left cards without challenges a couple
/// of pixels short in the grid. [#64]
fn draw_season_pills(ui: &mut egui::Ui, crucible: bool, blood_ritual: bool, reserve: bool) {
    if !crucible && !blood_ritual {
        if reserve {
            // The same row structure the pills use below, so the reserved row is
            // identical in height rather than merely close to it.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                draw_pill_invisible(ui, "CRUCIBLE");
            });
        }
        return;
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if crucible {
            draw_crucible_pill(ui);
        }
        if blood_ritual {
            draw_blood_ritual_pill(ui);
        }
    });
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
    // A clock reads as seconds-accurate without spending the card's narrow left
    // column on "m"/"s" units, and its width is fixed by the format: five
    // characters up to an hour, seven beyond it.
    let (h, m, s) = (
        remaining_secs / 3600,
        (remaining_secs % 3600) / 60,
        remaining_secs % 60,
    );
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{}:{:02}", m, s)
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
    /// Whether this character has a Blood Ritual in effect, resolved by the caller
    /// against the tiers the feed delivered. Passed in rather than read off the
    /// character so the pill is right regardless of refresh timing.
    blood_ritual_active: bool,
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
            blood_ritual_active: false,
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
            blood_ritual_active: false,
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
            blood_ritual_active: false,
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
    pub fn with_blood_ritual(mut self, blood_ritual_active: bool) -> Self {
        self.blood_ritual_active = blood_ritual_active;
        self
    }

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
                // Compact mode: name on line 1, the season-challenge pills on
                // line 2 (stacked in a vertical so a long 20-char name can't
                // stretch the card). Small top/bottom padding keeps the two lines
                // within the 32px sprite box.
                let display_name = custom_label.unwrap_or(char.class_name());
                let name_color = if char.seasonal { SEASONAL_COLOR } else { REGULAR_COLOR };
                ui.vertical(|ui| {
                    ui.add_space(2.0);
                    ui.label(RichText::new(display_name).strong().size(14.0).color(name_color));
                    if char.crucible_active || self.blood_ritual_active {
                        draw_season_pills(
                            ui,
                            char.crucible_active,
                            self.blood_ritual_active,
                            false,
                        );
                        ui.add_space(2.0);
                    }
                });
            } else {
                // Full mode: name, badges, level, maxed, fame
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    // Label and badges. The name is the widest text on the card
                    // and the only row whose content is not fixed, so it is capped
                    // to whatever the badges leave: a name too long for the card
                    // clips with an ellipsis instead of stretching this one card
                    // wider than its neighbours.
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = NAME_ROW_SPACING;
                        let display_name = custom_label.unwrap_or(char.class_name());
                        let name_color = if char.seasonal { SEASONAL_COLOR } else { REGULAR_COLOR };

                        // A live character is flagged with a small green dot +
                        // LIVE right after the name. (The old lightning glyph was
                        // borrowed from Muledump, where it actually means an
                        // active loot boost, not "currently playing".)
                        let mut badges: Vec<(&str, Color32)> = Vec::new();
                        if is_live {
                            badges.push(("\u{25cf} LIVE", Color32::from_rgb(100, 255, 100)));
                        }
                        if char.is_dead && !self.is_graveyard {
                            badges.push(("\u{1f480} DEAD", Color32::from_rgb(255, 80, 80)));
                        }

                        let small = {
                            let style = ui.style();
                            style
                                .text_styles
                                .get(&egui::TextStyle::Small)
                                .cloned()
                                .unwrap_or_else(|| egui::FontId::proportional(10.0))
                        };
                        let badges_w: f32 = badges
                            .iter()
                            .map(|(text, _)| {
                                text_width(ui, text, small.clone()) + NAME_ROW_SPACING
                            })
                            .sum();
                        let name_room =
                            (CARD_CONTENT_WIDTH - CARD_TEXT_INDENT - badges_w).max(0.0);

                        ui.scope(|ui| {
                            ui.set_max_width(name_room);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(display_name)
                                        .strong()
                                        .size(NAME_FONT_SIZE)
                                        .color(name_color),
                                )
                                .truncate(),
                            );
                        });

                        for (text, color) in badges {
                            ui.label(RichText::new(text).small().color(color));
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

                        // Identity line (row 2): id, level, maxed. Always present,
                        // so the row under the name is never blank -- the optional
                        // readouts all sit lower down instead.
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
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

                        // Fame on its own row (row 3). Always shown (0 when none)
                        // so the card height stays consistent and a long value
                        // can't stretch the card.
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            render_fame_icon(ui, sprite_renderer, 13.0, false);
                            ui.add_space(-2.0);
                            ui.label(
                                RichText::new(format_fame(char.fame))
                                    .size(12.5)
                                    .color(Color32::from_rgb(255, 200, 100)),
                            );
                        });

                        // Season-challenge pills and the loot readout share the last
                        // row (row 4). Both are optional, so pairing them keeps every
                        // card's name, identity line and fame in the same place, and a
                        // card carrying neither is blank at the bottom rather than in
                        // the middle. The pills reserve the row even when empty, so the
                        // height stays steady either way.
                        //
                        // The readout is painted rather than laid out: it belongs to
                        // the card's left column, under the pet sprite, while the pills
                        // sit in the text column. Laying it out would consume width here
                        // and push the pills across, so a running timer moved the row.
                        // Its font is fixed at 10px so the pair cannot set the row's
                        // height -- the pill alone does that.
                        //
                        // The remaining seconds are server-driven, so the timer
                        // naturally freezes when the game pauses it
                        // (Nexus/vault/loading/offline). The panel pins the value
                        // to the boosted character, so its card keeps showing the
                        // last-known timer even when not live.
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            // The loot boost reads from the card's left column, under the pet sprite,
                            // while the id line keeps the row's own x. It is painted rather than laid
                            // out for exactly that reason: laying it out would consume width here and
                            // push the id across, so a running timer moved the whole line.
                            if let Some(secs) = self.loot_boost_secs {
                                const HINT: &str = "Loot-drop boost remaining. Pauses in safe areas (Nexus, vault, etc.) and while loading.";
                                // 10px so the five-character clock fits the column the sprite occupies.
                                let font = egui::FontId::proportional(10.0);
                                let area = ui.available_rect_before_wrap();
                                // The row starts at the text column, one inset in
                                // from the content edge; the readout is painted
                                // back at that edge so it sits under the sprite.
                                let content_left =
                                    area.left() - CARD_TEXT_INDENT;
                                // Nudged off the column and the row's middle by eye
                                // (7px left, 2px down): the glyph's own bearings push
                                // the pair right of the column and high of the text's
                                // centre.
                                let left = content_left + LOOT_BOOST_LEFT;
                                let centre = area.top() + 9.0;
                                ui.painter().text(
                                    egui::pos2(left, centre),
                                    egui::Align2::LEFT_CENTER,
                                    LOOT_BOOST_GLYPH,
                                    font.clone(),
                                    LOOT_BOOST_COLOR,
                                );
                                ui.painter().text(
                                    egui::pos2(left + LOOT_GLYPH_W, centre),
                                    egui::Align2::LEFT_CENTER,
                                    format_loot_boost(secs as u64),
                                    font,
                                    LOOT_BOOST_COLOR,
                                );
                                // Painted text has no response of its own, so the hover hangs off the rect
                                // it was drawn in -- also paint-only, so it cannot shift the row either.
                                let hit = egui::Rect::from_min_size(
                                    egui::pos2(left, centre - 7.0),
                                    egui::vec2(LOOT_SLOT_W, 14.0),
                                );
                                ui.interact(hit, ui.id().with("loot_boost"), Sense::hover())
                                    .on_hover_text(HINT);
                                if is_live {
                                    ui.ctx()
                                        .request_repaint_after(std::time::Duration::from_millis(500));
                                }
                            }

                            draw_season_pills(
                                ui,
                                char.crucible_active,
                                self.blood_ritual_active,
                                true,
                            );
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
    use super::*;

    /// Run `f` inside a real egui pass, so text is measured with the fonts the
    /// card actually draws with rather than a guess at how wide it is.
    fn measure<R>(f: impl FnOnce(&mut egui::Ui) -> R) -> R {
        let ctx = egui::Context::default();
        let mut f = Some(f);
        let mut out = None;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                if let Some(f) = f.take() {
                    out = Some(f(ui));
                }
            });
        });
        out.expect("egui pass ran")
    }

    /// The small-text font the card's badges use.
    fn small_font(ui: &egui::Ui) -> egui::FontId {
        ui.style()
            .text_styles
            .get(&egui::TextStyle::Small)
            .cloned()
            .unwrap_or_else(|| egui::FontId::proportional(10.0))
    }

    #[test]
    fn loot_boost_uses_minutes_seconds_under_one_hour() {
        assert_eq!(format_loot_boost(0), "0:00");
        assert_eq!(format_loot_boost(5), "0:05");
        assert_eq!(format_loot_boost(65), "1:05");
        assert_eq!(format_loot_boost(3599), "59:59");
    }

    #[test]
    fn loot_boost_rolls_to_hours_minutes_at_one_hour() {
        assert_eq!(format_loot_boost(3600), "1:00:00");
        assert_eq!(format_loot_boost(3660), "1:01:00");
        assert_eq!(format_loot_boost(7325), "2:02:05");
    }

    /// The loot readout is painted between the card's content edge and the id
    /// column beside it, and a timer past ten hours is the longest the server
    /// sends. This is the slot that has to hold it: a boost of an hour or more
    /// (`1:45:48`, `10:45:48`) is what used to run into the id line.
    #[test]
    fn the_loot_readout_fits_beside_the_id_line() {
        let (slot, longest) = measure(|ui| {
            let longest = format!(
                "{}{}",
                LOOT_BOOST_GLYPH,
                format_loot_boost(10 * 3600 + 45 * 60 + 48)
            );
            (
                LOOT_SLOT_W,
                text_width(ui, &longest, egui::FontId::proportional(10.0)),
            )
        });

        assert!(
            longest < slot,
            "the longest readout ({longest:.1}px) does not fit the {slot:.1}px slot"
        );
    }

    /// The cap only helps if it holds in a real pass: the row is built the way the
    /// card builds it, and a name far too long for the card must still leave the
    /// row inside the card's width rather than stretching it.
    #[test]
    fn a_long_name_cannot_widen_the_name_row() {
        let row_w = measure(|ui| {
            ui.set_max_width(CARD_CONTENT_WIDTH);
            let badge = "\u{25cf} LIVE";
            let badges_w = text_width(ui, badge, small_font(ui)) + NAME_ROW_SPACING;
            let room = CARD_CONTENT_WIDTH - CARD_TEXT_INDENT - badges_w;
            let mut row_w = 0.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = NAME_ROW_SPACING;
                ui.scope(|ui| {
                    ui.set_max_width(room);
                    ui.add(
                        egui::Label::new(RichText::new("New Mystic NPE").size(NAME_FONT_SIZE))
                            .truncate(),
                    );
                });
                ui.label(RichText::new(badge).small());
                row_w = ui.min_rect().width();
            });
            row_w
        });

        let row_budget = CARD_CONTENT_WIDTH - CARD_TEXT_INDENT;
        assert!(
            row_w <= row_budget,
            "the row came to {row_w:.1}px of its {row_budget:.1}px budget"
        );
    }
}

//! Exaltations Panel - RealmEye-style account exaltation overview
//! broken down by class rather than by character.
//!
//! Shows an account-wide total-progress bar, a class x stat matrix (class-header
//! square buttons + per-stat headers), and a per-class detail page reusing the
//! in-game exalt menu from [`crate::panels::exalt_view`].

use crate::ui_ext::HoverTooltipExt;
use std::collections::HashMap;

use eframe::egui::{self, Color32, RichText, ScrollArea};
use realmhound_core::{
    api::{CharacterClass, ClassExaltation},
    assets::get_asset_manager,
    loot::class_ids,
    stats::exalt_proficiency,
};

use crate::panels::exalt_view::{exalt_colors, ExaltView};
use crate::panels::{empty_state, AppAction, Panel, PanelContext};
use crate::rendering::SpriteRenderer;

/// Per-stat metadata in display order (ATT, DEF, SPD, DEX, VIT, WIS, HP, MP).
/// `(short, potion_id, banner_idname)`. The banner id_name is looked up in
/// ObjectID.list to swap the potion icon for an exaltation banner once the stat
/// is fully exalted account-wide.
struct StatMeta {
    short: &'static str,
    potion_id: i32,
    banner_idname: &'static str,
}

const STATS: [StatMeta; 8] = [
    StatMeta {
        short: "ATT",
        potion_id: 2591,
        banner_idname: "ExaltationBanner_ATT",
    },
    StatMeta {
        short: "DEF",
        potion_id: 2592,
        banner_idname: "ExaltationBanner_DEF",
    },
    StatMeta {
        short: "SPD",
        potion_id: 2593,
        banner_idname: "ExaltationBanner_SPD",
    },
    StatMeta {
        short: "DEX",
        potion_id: 2636,
        banner_idname: "ExaltationBanner_DEX",
    },
    StatMeta {
        short: "VIT",
        potion_id: 2612,
        banner_idname: "ExaltationBanner_VIT",
    },
    StatMeta {
        short: "WIS",
        potion_id: 2613,
        banner_idname: "ExaltationBanner_WIS",
    },
    StatMeta {
        short: "HP",
        potion_id: 2793,
        banner_idname: "ExaltationBanner_LIFE",
    },
    StatMeta {
        short: "MP",
        potion_id: 2794,
        banner_idname: "ExaltationBanner_MANA",
    },
];

/// A dungeon that can exalt a stat, with an optional clarifying note and any
/// One inline piece of a dungeon note. When `icon` is set, its sprite is drawn
/// immediately before `text` (so a boss/item icon precedes its name).
struct NoteSeg {
    icon: Option<&'static str>,
    text: &'static str,
}

const fn seg(text: &'static str) -> NoteSeg {
    NoteSeg { icon: None, text }
}

const fn boss(name: &'static str, text: &'static str) -> NoteSeg {
    NoteSeg {
        icon: Some(name),
        text,
    }
}

/// A dungeon that can exalt a stat, with an optional inline note whose boss/item
/// names are annotated with their sprites in the stat-header tooltip.
struct DungeonSource {
    name: &'static str,
    note: &'static [NoteSeg],
}

const fn dungeon(name: &'static str) -> DungeonSource {
    DungeonSource { name, note: &[] }
}

/// Resolve a boss/item sprite id by display name first (bosses store the human
/// name there) then by id_name (items store it there).
fn resolve_sprite(name: &str) -> Option<i32> {
    let mgr = get_asset_manager();
    mgr.object_id_for_display_name(name)
        .or_else(|| mgr.object_id_for_name(name))
}

/// Dungeons that can exalt each stat, in display order (ATT, DEF, SPD, DEX,
/// VIT, WIS, HP, MP). Shown in the stat-header hover tooltip.
const STAT_DUNGEONS: [&[DungeonSource]; 8] = [
    &[
        DungeonSource {
            name: "Spectral Penitentiary",
            note: &[
                seg("(only from"),
                boss("Soulwarden Murcian", "Soulwarden Murcian)"),
            ],
        },
        DungeonSource {
            name: "Shatters",
            note: &[seg("("), boss("Twilight Archmage", "Twilight Archmage)")],
        },
        DungeonSource {
            name: "Moonlight Village",
            note: &[
                seg("(if"),
                boss("Dancer Miko", "Dancer Miko"),
                seg("is defeated last)"),
            ],
        },
    ],
    &[DungeonSource {
        name: "Lost Halls",
        note: &[
            seg("(+1 extra if"),
            boss("Vial of Pure Darkness", "Vial of Pure Darkness"),
            seg("was used after the"),
            boss("Marble Colossus", "Marble Colossus"),
            seg("fight)"),
        ],
    }],
    &[
        dungeon("Cultist Hideout"),
        dungeon("Ice Citadel"),
        dungeon("Neo Forax"),
    ],
    &[
        dungeon("The Nest"),
        dungeon("Plagued Nest"),
        dungeon("Neo Katalund"),
    ],
    &[
        dungeon("Kogbold Steamworks"),
        dungeon("Advanced Kogbold Steamworks"),
        dungeon("Neo Malogia"),
    ],
    &[
        dungeon("Fungal Cavern"),
        dungeon("Crystal Cavern"),
        dungeon("Neo Untaris"),
    ],
    &[
        dungeon("Oryx's Sanctuary"),
        DungeonSource {
            name: "Moonlight Village",
            note: &[
                seg("(if"),
                boss("Sage Genji", "Sage Genji"),
                seg("is defeated last)"),
            ],
        },
        DungeonSource {
            name: "Shatters",
            note: &[seg("("), boss("The Forgotten King", "The Forgotten King)")],
        },
    ],
    &[
        dungeon("The Void"),
        DungeonSource {
            name: "Moonlight Village",
            note: &[
                seg("(if"),
                boss("Drummer Kaguya", "Drummer Kaguya"),
                seg("is defeated last)"),
            ],
        },
        DungeonSource {
            name: "Shatters",
            note: &[seg("("), boss("The Forgotten King", "The Forgotten King)")],
        },
    ],
];

/// Raw completion count needed to fully exalt a single stat (5 milestones at
/// cumulative 5/15/30/50/75). Used for the "points" display mode.
const STAT_MAX_POINTS: u8 = 75;

/// Matrix cell display mode toggled from the panel header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExaltDisplayMode {
    /// Exalt milestones reached (0-5).
    Milestones,
    /// Raw dungeon-completion points toward full exaltation (0-75).
    Points,
    /// Points still needed to fully exalt each stat (75-0).
    PointsRemaining,
}

/// Layout orientation for the class x stat matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExaltLayout {
    /// Classes down the left as rows, stats across the top (RealmEye style).
    ClassesVertical,
    /// Transposed: classes across the top, stats down the left.
    ClassesHorizontal,
}

/// Class ordering for the matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExaltSort {
    /// Weapon-type grouping (dagger, bow, staff, wand, sword, katana).
    WeaponType,
    /// Total exaltations ascending.
    ExaltAsc,
    /// Total exaltations descending.
    ExaltDesc,
    /// Alphabetical by class name.
    Alphabetical,
    /// By a single stat's points (triggered by clicking a stat header).
    ByStat { index: usize, desc: bool },
}

/// Color scheme for the matrix cell numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExaltCellColors {
    /// Green when complete, blue when partial, gray when zero.
    Simple,
    /// Dark-to-white grey ramp by milestone level.
    Greyscale,
    /// Red -> orange -> yellow -> green by milestone level.
    Dynamic,
}

pub struct ExaltationsPanel {
    layout: ExaltLayout,
    /// Matrix cell display mode (milestones vs raw points).
    display_mode: ExaltDisplayMode,
    /// Class ordering for the matrix.
    sort: ExaltSort,
    /// Color scheme for matrix cell numbers.
    cell_colors: ExaltCellColors,
    /// The class shown in the always-visible left detail column.
    selected_class: Option<i32>,
    /// Resolved exalted-skin object ids per class (populated lazily).
    exalted_skin_cache: HashMap<i32, i32>,
    /// Resolved exaltation-banner object ids per stat index (populated lazily).
    banner_cache: HashMap<usize, i32>,
    /// Set when arrow-key navigation changes the selection, so the matrix
    /// scrolls the newly-selected class into view on the next frame.
    scroll_to_selected: bool,
}

impl Default for ExaltationsPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl ExaltationsPanel {
    pub fn new() -> Self {
        Self {
            layout: ExaltLayout::ClassesHorizontal,
            display_mode: ExaltDisplayMode::Milestones,
            sort: ExaltSort::WeaponType,
            cell_colors: ExaltCellColors::Simple,
            selected_class: None,
            exalted_skin_cache: HashMap::new(),
            banner_cache: HashMap::new(),
            scroll_to_selected: false,
        }
    }

    /// Class ids in the order selected by the current sort.
    fn sorted_classes(&self, stats: &HashMap<i32, ClassExaltation>) -> Vec<i32> {
        match self.sort {
            ExaltSort::WeaponType => {
                // Base weapon-type order, but Druid (a wand class) sits right
                // after Summoner instead of at the end.
                let mut out = Vec::with_capacity(class_ids::ALL.len());
                for &cid in class_ids::ALL {
                    if cid == class_ids::DRUID {
                        continue;
                    }
                    out.push(cid);
                    if cid == class_ids::SUMMONER {
                        out.push(class_ids::DRUID);
                    }
                }
                out
            }
            ExaltSort::ExaltAsc | ExaltSort::ExaltDesc => {
                let mut out: Vec<i32> = class_ids::ALL.to_vec();
                out.sort_by_key(|&cid| Self::class_total(stats, cid));
                if self.sort == ExaltSort::ExaltDesc {
                    out.reverse();
                }
                out
            }
            ExaltSort::Alphabetical => {
                let mut out: Vec<i32> = class_ids::ALL.to_vec();
                out.sort_by_key(|&cid| CharacterClass::from_id(cid as u16).name());
                out
            }
            ExaltSort::ByStat { index, desc } => {
                let mut out: Vec<i32> = class_ids::ALL.to_vec();
                out.sort_by_key(|&cid| Self::class_points(stats, cid)[index]);
                if desc {
                    out.reverse();
                }
                out
            }
        }
    }

    /// Resolve (and cache) the exalted-skin sprite id for a class, or fall back
    /// to the default class sprite (`class_id`) when it can't be resolved.
    fn exalted_skin_id(&mut self, class_id: i32) -> i32 {
        if let Some(&id) = self.exalted_skin_cache.get(&class_id) {
            return id;
        }
        let name = format!(
            "Exalted {}",
            CharacterClass::from_id(class_id as u16).name()
        );
        if let Some(id) = get_asset_manager().object_id_for_name(&name) {
            self.exalted_skin_cache.insert(class_id, id);
            return id;
        }
        class_id
    }

    /// Resolve (and cache) the exaltation-banner sprite id for a stat, or fall
    /// back to the stat potion sprite when it can't be resolved.
    fn banner_id(&mut self, stat_index: usize) -> i32 {
        if let Some(&id) = self.banner_cache.get(&stat_index) {
            return id;
        }
        if let Some(id) = get_asset_manager().object_id_for_name(STATS[stat_index].banner_idname) {
            self.banner_cache.insert(stat_index, id);
            return id;
        }
        STATS[stat_index].potion_id
    }

    /// Per-class stat levels (0-5) in display order, zero-filled when absent.
    fn class_levels(stats: &HashMap<i32, ClassExaltation>, class_id: i32) -> [u8; 8] {
        stats
            .get(&class_id)
            .map(exalt_proficiency::stat_levels)
            .unwrap_or([0; 8])
    }

    /// Per-class raw exaltation points (0-75) in display order, zero-filled when
    /// absent. Values above the max are clamped so a fully-exalted stat reads 75.
    fn class_points(stats: &HashMap<i32, ClassExaltation>, class_id: i32) -> [u8; 8] {
        match stats.get(&class_id) {
            Some(e) => [
                e.attack,
                e.defense,
                e.speed,
                e.dexterity,
                e.vitality,
                e.wisdom,
                e.hp,
                e.mp,
            ]
            .map(|v| v.min(STAT_MAX_POINTS as u32) as u8),
            None => [0; 8],
        }
    }

    /// Total exalt levels (0-40) for a class.
    fn class_total(stats: &HashMap<i32, ClassExaltation>, class_id: i32) -> u32 {
        stats
            .get(&class_id)
            .map(exalt_proficiency::class_total_levels)
            .unwrap_or(0)
    }

    /// Whether every canonical class has stat `stat_index` at level 5.
    fn stat_exalted_account_wide(stats: &HashMap<i32, ClassExaltation>, stat_index: usize) -> bool {
        class_ids::ALL
            .iter()
            .all(|&cid| Self::class_levels(stats, cid)[stat_index] >= 5)
    }

    // -- Rendering ----------------------------------------------------------

    /// Header band: layout, display, sort and color options.
    fn render_header(&mut self, ui: &mut egui::Ui, ctx: &mut PanelContext) {
        let shadcn = ctx.shadcn;
        shadcn.header_band(ui, shadcn.secondary_header_fill(), |ui| {
            shadcn.band_row(ui, |ui| {
                ui.label("Layout:");
                egui::ComboBox::from_id_salt("exalt_layout")
                    .selected_text(match self.layout {
                        ExaltLayout::ClassesVertical => "Classes ↓",
                        ExaltLayout::ClassesHorizontal => "Classes →",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.layout,
                            ExaltLayout::ClassesVertical,
                            "Classes ↓",
                        );
                        ui.selectable_value(
                            &mut self.layout,
                            ExaltLayout::ClassesHorizontal,
                            "Classes →",
                        );
                    });
                ui.separator();
                ui.label("Show:");
                egui::ComboBox::from_id_salt("exalt_display_mode")
                    .selected_text(match self.display_mode {
                        ExaltDisplayMode::Milestones => "Milestones",
                        ExaltDisplayMode::Points => "Points",
                        ExaltDisplayMode::PointsRemaining => "Points remaining",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.display_mode,
                            ExaltDisplayMode::Milestones,
                            "Milestones (X/5)",
                        );
                        ui.selectable_value(
                            &mut self.display_mode,
                            ExaltDisplayMode::Points,
                            "Points (Y/75)",
                        );
                        ui.selectable_value(
                            &mut self.display_mode,
                            ExaltDisplayMode::PointsRemaining,
                            "Points remaining",
                        );
                    });
                ui.separator();
                ui.label("Sort:");
                let by_stat = match self.sort {
                    ExaltSort::ByStat { index, .. } => Some(index),
                    _ => None,
                };
                let sort_combo = egui::ComboBox::from_id_salt("exalt_sort")
                    .selected_text(match self.sort {
                        ExaltSort::WeaponType => "Weapon type".to_string(),
                        ExaltSort::ExaltAsc => "Exaltations ↑".to_string(),
                        ExaltSort::ExaltDesc => "Exaltations ↓".to_string(),
                        ExaltSort::Alphabetical => "A-Z".to_string(),
                        ExaltSort::ByStat { index, desc } => {
                            // Leading padding reserves room for the potion sprite
                            // painted over the button's left edge below.
                            format!(
                                "     {} {}",
                                STATS[index].short,
                                if desc { "↓" } else { "↑" }
                            )
                        }
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.sort, ExaltSort::ExaltAsc, "Exaltations ↑");
                        ui.selectable_value(&mut self.sort, ExaltSort::ExaltDesc, "Exaltations ↓");
                        ui.selectable_value(&mut self.sort, ExaltSort::Alphabetical, "A-Z");
                        ui.selectable_value(&mut self.sort, ExaltSort::WeaponType, "Weapon type");
                    });
                if let Some(index) = by_stat {
                    let id = STATS[index].potion_id;
                    let btn = sort_combo.response.rect;
                    let icon = egui::Rect::from_center_size(
                        egui::pos2(btn.left() + 12.0, btn.center().y),
                        egui::vec2(14.0, 14.0),
                    );
                    ctx.sprite_renderer.draw_sprite_in_rect(ui, id, icon);
                }

                ui.separator();
                ui.label("Colors:");
                egui::ComboBox::from_id_salt("exalt_cell_colors")
                    .selected_text(match self.cell_colors {
                        ExaltCellColors::Simple => "Simple",
                        ExaltCellColors::Greyscale => "Greyscale",
                        ExaltCellColors::Dynamic => "Dynamic",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.cell_colors,
                            ExaltCellColors::Simple,
                            "Simple",
                        );
                        ui.selectable_value(
                            &mut self.cell_colors,
                            ExaltCellColors::Greyscale,
                            "Greyscale",
                        );
                        ui.selectable_value(
                            &mut self.cell_colors,
                            ExaltCellColors::Dynamic,
                            "Dynamic",
                        );
                    });
            });
        });
    }

    /// Draw a single class-header square button. Returns the click response.
    /// When `show_name` is false (narrow buttons), the class name row is hidden
    /// and only the icon and count are drawn.
    fn class_button(
        &mut self,
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        class_id: i32,
        size: egui::Vec2,
        selected: bool,
        show_name: bool,
        stats: &HashMap<i32, ClassExaltation>,
    ) -> egui::Response {
        let total = Self::class_total(stats, class_id);
        let fully_exalted = total >= exalt_proficiency::MAX_CLASS_LEVELS;
        let skin_id = if fully_exalted {
            self.exalted_skin_id(class_id)
        } else {
            class_id
        };
        let class_name = CharacterClass::from_id(class_id as u16).name();

        let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
        if ui.is_rect_visible(rect) {
            let bg = if selected {
                Color32::from_rgba_unmultiplied(
                    exalt_colors::BLUE.r(),
                    exalt_colors::BLUE.g(),
                    exalt_colors::BLUE.b(),
                    34,
                )
            } else if resp.hovered() {
                Color32::from_white_alpha(22)
            } else {
                Color32::from_white_alpha(10)
            };
            ui.painter().rect_filled(rect, 4.0, bg);
            let stroke = if selected {
                egui::Stroke::new(2.0_f32, exalt_colors::BLUE)
            } else if fully_exalted {
                egui::Stroke::new(2.0_f32, exalt_colors::AMBER)
            } else {
                egui::Stroke::new(1.0_f32, Color32::from_white_alpha(30))
            };
            ui.painter()
                .rect_stroke(rect, 4.0, stroke, egui::StrokeKind::Inside);

            let sprite = 27.0_f32.min(size.x - 8.0);
            let name_font = egui::FontId::proportional(10.0);
            let num_font = egui::FontId::proportional(10.0);
            let name_h = if show_name { 15.0 + 2.0 } else { 0.0 };
            let content_h = sprite + 5.0 + name_h + 13.0;
            let top = rect.top() + ((rect.height() - content_h) * 0.5).max(4.0);

            let sprite_rect = egui::Rect::from_center_size(
                egui::pos2(rect.center().x, top + sprite * 0.5),
                egui::vec2(sprite, sprite),
            );
            sprite_renderer.draw_outlined_sprite_in_rect(ui, skin_id, sprite_rect);

            let painter = ui.painter();
            let num_top = if show_name {
                let name_rect = painter.text(
                    egui::pos2(rect.center().x, sprite_rect.bottom() + 5.0),
                    egui::Align2::CENTER_TOP,
                    class_name,
                    name_font,
                    Color32::WHITE,
                );
                name_rect.bottom() + 2.0
            } else {
                sprite_rect.bottom() + 5.0
            };

            let (num_text, num_color) = if fully_exalted {
                ("EXALTED".to_string(), exalt_colors::AMBER)
            } else {
                (format!("{total}/40"), exalt_colors::BLUE)
            };
            painter.text(
                egui::pos2(rect.center().x, num_top),
                egui::Align2::CENTER_TOP,
                num_text,
                num_font,
                num_color,
            );
        }
        resp.hover_tip(format!("{class_name} - select"))
    }

    /// Draw a stat header (potion or exaltation banner + short name) with the
    /// portals/"Not exalted on" tooltip.
    fn stat_header(
        &mut self,
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        stat_index: usize,
        size: egui::Vec2,
        stats: &HashMap<i32, ClassExaltation>,
    ) {
        let account_exalted = Self::stat_exalted_account_wide(stats, stat_index);
        let icon_id = if account_exalted {
            self.banner_id(stat_index)
        } else {
            STATS[stat_index].potion_id
        };
        let sort_dir = match self.sort {
            ExaltSort::ByStat { index, desc } if index == stat_index => Some(desc),
            _ => None,
        };

        let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if ui.is_rect_visible(rect) {
            let active = sort_dir.is_some();
            let bg = if active {
                Color32::from_rgba_unmultiplied(
                    exalt_colors::BLUE.r(),
                    exalt_colors::BLUE.g(),
                    exalt_colors::BLUE.b(),
                    34,
                )
            } else if resp.hovered() {
                Color32::from_white_alpha(22)
            } else {
                Color32::from_white_alpha(10)
            };
            ui.painter().rect_filled(rect, 4.0, bg);
            let stroke = if active {
                egui::Stroke::new(2.0_f32, exalt_colors::BLUE)
            } else if account_exalted {
                egui::Stroke::new(2.0_f32, exalt_colors::AMBER)
            } else {
                egui::Stroke::new(1.0_f32, Color32::from_white_alpha(30))
            };
            ui.painter()
                .rect_stroke(rect, 4.0, stroke, egui::StrokeKind::Inside);
            // Potions are compact 8x8 icons; exaltation banners are tall (~16x20
            // incl. pole/tassels). Keep the banner box modest and reserve extra
            // bottom padding so the centered label isn't glued to the outline.
            let gap = 3.0;
            let text_h = 13.0;
            let bottom_pad = 4.0;
            let icon_w = (if account_exalted { 26.0_f32 } else { 22.0 }).min(size.x - 6.0);
            let icon_h = if account_exalted { 26.0 } else { 22.0 };
            let content_h = icon_h + gap + text_h;
            let top = rect.top() + ((rect.height() - content_h - bottom_pad) * 0.5).max(1.0);
            let icon_rect = egui::Rect::from_center_size(
                egui::pos2(rect.center().x, top + icon_h * 0.5),
                egui::vec2(icon_w, icon_h),
            );
            if account_exalted {
                sprite_renderer.draw_object_full_outlined_in_rect(ui, icon_id, icon_rect, 32);
            } else {
                sprite_renderer.draw_outlined_sprite_in_rect(ui, icon_id, icon_rect);
            }
            let name_color = if sort_dir.is_some() {
                exalt_colors::BLUE
            } else if account_exalted {
                exalt_colors::AMBER
            } else {
                Color32::WHITE
            };
            let label = match sort_dir {
                Some(true) => format!("{} ↓", STATS[stat_index].short),
                Some(false) => format!("{} ↑", STATS[stat_index].short),
                None => STATS[stat_index].short.to_string(),
            };
            ui.painter().text(
                egui::pos2(rect.center().x, icon_rect.bottom() + gap),
                egui::Align2::CENTER_TOP,
                label,
                egui::FontId::proportional(11.5),
                name_color,
            );
        }

        if resp.clicked() {
            self.sort = match self.sort {
                ExaltSort::ByStat { index, desc } if index == stat_index => ExaltSort::ByStat {
                    index: stat_index,
                    desc: !desc,
                },
                _ => ExaltSort::ByStat {
                    index: stat_index,
                    desc: true,
                },
            };
        }

        let not_exalted: Vec<i32> = class_ids::ALL
            .iter()
            .copied()
            .filter(|&cid| Self::class_levels(stats, cid)[stat_index] < 5)
            .collect();
        let portal_map = realmhound_core::assets::get_dungeon_portal_map();

        resp.hover_tip_ui(|ui| {
            ui.label(RichText::new("Click to sort by this stat").color(Color32::WHITE));
            ui.add_space(4.0);
            ui.label(RichText::new("Exalted in dungeons:").color(exalt_colors::GRAYTXT));
            ui.add_space(2.0);
            for src in STAT_DUNGEONS[stat_index] {
                let portal_id = portal_map.get_portal_id(src.name);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let (pr, _) =
                        ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover());
                    if let Some(id) = portal_id {
                        sprite_renderer.draw_sprite_in_rect(ui, id, pr);
                    }
                    ui.label(RichText::new(src.name).color(Color32::WHITE));
                    let ppp = ui.ctx().pixels_per_point();
                    for part in src.note {
                        if let Some(icon_name) = part.icon {
                            // The name is white; any trailing punctuation baked
                            // into `text` (e.g. a closing bracket) stays grey.
                            let (name_txt, tail) = match part.text.strip_prefix(icon_name) {
                                Some(rest) => (icon_name, rest),
                                None => (part.text, ""),
                            };
                            if let Some(id) = resolve_sprite(icon_name) {
                                let sz = sprite_renderer.rendered_sprite_size(id, 20.0, ppp);
                                let (ir, _) = ui.allocate_exact_size(sz, egui::Sense::hover());
                                sprite_renderer.draw_sprite_in_rect(ui, id, ir);
                                ui.spacing_mut().item_spacing.x = 2.0;
                            }
                            ui.label(RichText::new(name_txt).color(Color32::WHITE));
                            if !tail.is_empty() {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                ui.label(RichText::new(tail).color(exalt_colors::GRAYTXT));
                            }
                            ui.spacing_mut().item_spacing.x = 4.0;
                        } else {
                            ui.label(RichText::new(part.text).color(exalt_colors::GRAYTXT));
                        }
                    }
                });
            }
            ui.add_space(6.0);
            if not_exalted.is_empty() {
                ui.label(RichText::new("Fully exalted on all classes!").color(exalt_colors::GREEN));
            } else {
                ui.label(RichText::new("Not exalted on:").color(exalt_colors::GRAYTXT));
                ui.add_space(2.0);
                ui.horizontal_wrapped(|ui| {
                    for cid in &not_exalted {
                        let (cr, _) =
                            ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::hover());
                        sprite_renderer.draw_outlined_sprite_in_rect(ui, *cid, cr);
                        ui.add_space(2.0);
                    }
                });
            }
        });
    }

    /// Draw a single matrix cell. Color depends on the active cell-color scheme;
    /// `level` is the 0-5 milestone used by the greyscale/dynamic ramps.
    fn cell(
        ui: &mut egui::Ui,
        value: u8,
        complete: bool,
        partial: bool,
        level: u8,
        scheme: ExaltCellColors,
        size: egui::Vec2,
    ) {
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        if !ui.is_rect_visible(rect) {
            return;
        }
        let color = match scheme {
            ExaltCellColors::Simple => {
                if complete {
                    exalt_colors::GREEN
                } else if partial {
                    exalt_colors::BLUE
                } else {
                    exalt_colors::GRAYTXT
                }
            }
            ExaltCellColors::Greyscale => match level {
                0 => Color32::from_gray(96),
                1 | 2 => Color32::from_gray(150),
                3 | 4 => Color32::from_gray(205),
                _ => Color32::WHITE,
            },
            ExaltCellColors::Dynamic => match level {
                0 => Color32::from_rgb(214, 74, 74),
                1 | 2 => Color32::from_rgb(226, 143, 42),
                3 | 4 => Color32::from_rgb(226, 205, 60),
                _ => exalt_colors::GREEN,
            },
        };
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            format!("{value}"),
            egui::FontId::proportional(20.0),
            color,
        );
    }

    /// Per-class, per-stat cell values (text, complete, partial, milestone level)
    /// for the current display mode, in display order.
    fn cell_values(
        &self,
        stats: &HashMap<i32, ClassExaltation>,
        class_id: i32,
    ) -> [(u8, bool, bool, u8); 8] {
        let mut out = [(0u8, false, false, 0u8); 8];
        match self.display_mode {
            ExaltDisplayMode::Milestones => {
                let levels = Self::class_levels(stats, class_id);
                for i in 0..8 {
                    out[i] = (levels[i], levels[i] >= 5, levels[i] > 0, levels[i]);
                }
            }
            ExaltDisplayMode::Points => {
                let pts = Self::class_points(stats, class_id);
                for i in 0..8 {
                    out[i] = (
                        pts[i],
                        pts[i] >= STAT_MAX_POINTS,
                        pts[i] > 0,
                        Self::points_level(pts[i]),
                    );
                }
            }
            ExaltDisplayMode::PointsRemaining => {
                let pts = Self::class_points(stats, class_id);
                for i in 0..8 {
                    let complete = pts[i] >= STAT_MAX_POINTS;
                    out[i] = (
                        STAT_MAX_POINTS - pts[i],
                        complete,
                        pts[i] > 0 && !complete,
                        Self::points_level(pts[i]),
                    );
                }
            }
        }
        out
    }

    /// Milestone level (0-5) for a raw point total (15 points per milestone).
    fn points_level(points: u8) -> u8 {
        (points / 15).min(5)
    }

    /// Draw the account-wide exaltation progress in the matrix's empty corner.
    /// When `stacked`, each word sits on its own line (narrow horizontal-layout
    /// corner); otherwise the label is one line with the count beneath it.
    fn draw_progress_corner(ui: &egui::Ui, rect: egui::Rect, cur: u32, max: u32, stacked: bool) {
        if !ui.is_rect_visible(rect) {
            return;
        }
        let label_font = egui::FontId::proportional(12.5);
        let num_font = egui::FontId::proportional(15.0);
        let lines: [(String, egui::FontId, Color32); 3] = if stacked {
            [
                ("Total".into(), label_font.clone(), Color32::WHITE),
                ("Progress".into(), label_font, Color32::WHITE),
                (format!("{cur}/{max}"), num_font, exalt_colors::BLUE),
            ]
        } else {
            [
                ("Total Progress".into(), label_font, Color32::WHITE),
                (format!("{cur}/{max}"), num_font, exalt_colors::BLUE),
                (
                    String::new(),
                    egui::FontId::proportional(1.0),
                    Color32::TRANSPARENT,
                ),
            ]
        };
        let count = lines.iter().filter(|(t, _, _)| !t.is_empty()).count() as f32;
        let line_h = 18.0;
        let mut y = rect.center().y - (line_h * count) * 0.5 + line_h * 0.5;
        for (text, font, color) in lines {
            if text.is_empty() {
                continue;
            }
            ui.painter().text(
                egui::pos2(rect.center().x, y),
                egui::Align2::CENTER_CENTER,
                text,
                font,
                color,
            );
            y += line_h;
        }
    }

    /// Render the class x stat matrix. Returns a class id if one was clicked.
    fn render_matrix(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &mut PanelContext,
        stats: &HashMap<i32, ClassExaltation>,
        cur: u32,
        max: u32,
    ) -> Option<i32> {
        let mut clicked: Option<i32> = None;
        let classes = self.sorted_classes(stats);
        let selected = self.selected_class;
        let class_w = 64.0_f32;
        let class_h = 72.0_f32;
        let stat_h = 56.0_f32;
        let cell_h = 26.0_f32;
        let spacing = egui::vec2(6.0, 6.0);

        match self.layout {
            ExaltLayout::ClassesVertical => {
                // Stat columns scale to fill the available width (vertical scroll
                // only). Reserve scrollbar width so the pinned header columns
                // stay aligned with the scrolling body columns.
                ui.spacing_mut().item_spacing = spacing;
                let scrollbar_w = 18.0;
                let avail = ui.available_width();
                let stat_w =
                    ((avail - class_w - spacing.x * 9.0 - scrollbar_w) / 8.0).clamp(30.0, 90.0);

                // Pinned header row: empty corner + 8 stat headers (stays put
                // while the class rows scroll underneath).
                let header = ui.horizontal(|ui| {
                    let (corner, _) =
                        ui.allocate_exact_size(egui::vec2(class_w, stat_h), egui::Sense::hover());
                    Self::draw_progress_corner(ui, corner, cur, max, false);
                    for i in 0..8 {
                        self.stat_header(
                            ui,
                            ctx.sprite_renderer,
                            i,
                            egui::vec2(stat_w, stat_h),
                            stats,
                        );
                    }
                });
                // Full-width horizontal divider under the pinned header.
                let y = header.response.rect.bottom() + spacing.y * 0.5;
                ui.painter().hline(
                    header.response.rect.left()..=ui.max_rect().right(),
                    y,
                    egui::Stroke::new(1.0_f32, Color32::from_white_alpha(40)),
                );

                ScrollArea::vertical()
                    .id_salt("exalt_matrix")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing = spacing;
                        for &cid in &classes {
                            let values = self.cell_values(stats, cid);
                            ui.horizontal(|ui| {
                                let resp = self.class_button(
                                    ui,
                                    ctx.sprite_renderer,
                                    cid,
                                    egui::vec2(class_w, class_h),
                                    selected == Some(cid),
                                    true,
                                    stats,
                                );
                                if resp.clicked() {
                                    clicked = Some(cid);
                                }
                                if selected == Some(cid) && self.scroll_to_selected {
                                    resp.scroll_to_me(Some(egui::Align::Center));
                                }
                                ui.vertical(|ui| {
                                    // Spacer so cells center vertically against the tall class button.
                                    let pad = (class_h - cell_h) * 0.5;
                                    ui.add_space(pad);
                                    ui.horizontal(|ui| {
                                        for i in 0..8 {
                                            Self::cell(
                                                ui,
                                                values[i].0,
                                                values[i].1,
                                                values[i].2,
                                                values[i].3,
                                                self.cell_colors,
                                                egui::vec2(stat_w, cell_h),
                                            );
                                        }
                                    });
                                });
                            });
                        }
                    });
            }
            ExaltLayout::ClassesHorizontal => {
                ui.spacing_mut().item_spacing = spacing;
                let stat_label_w = 72.0_f32;
                let row_h = cell_h.max(stat_h);
                // Equal gap between the stat frames and the dividers on each side.
                let side_margin = 6.0_f32;

                // Stretch the class columns to fill the row edge-to-edge (no
                // horizontal scroll) when they fit; fall back to a minimum width
                // and scroll on narrow windows.
                let n = classes.len().max(1) as f32;
                let scroll_avail = ui.available_width() - (stat_label_w + side_margin * 3.0);
                let class_w = ((scroll_avail - spacing.x * (n - 1.0) - 4.0) / n).max(52.0);

                // Hide class names on every button when the widest one no longer
                // fits, leaving just the scaled icon and count.
                let name_font = egui::FontId::proportional(10.0);
                let max_name_w = classes
                    .iter()
                    .map(|&cid| {
                        let name = CharacterClass::from_id(cid as u16).name();
                        ui.fonts_mut(|f| {
                            f.layout_no_wrap(name.to_string(), name_font.clone(), Color32::WHITE)
                        })
                        .size()
                        .x
                    })
                    .fold(0.0_f32, f32::max);
                let show_names = max_name_w <= class_w - 6.0;

                let mut left_rect = egui::Rect::NOTHING;
                ScrollArea::vertical()
                    .id_salt("exalt_matrix_v")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            ui.add_space(side_margin);
                            // Pinned left column: empty corner + stat headers (stays
                            // put while class columns scroll horizontally).
                            let left = ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing = spacing;
                                let (corner, _) = ui.allocate_exact_size(
                                    egui::vec2(stat_label_w, class_h),
                                    egui::Sense::hover(),
                                );
                                Self::draw_progress_corner(ui, corner, cur, max, true);
                                for i in 0..8 {
                                    self.stat_header(
                                        ui,
                                        ctx.sprite_renderer,
                                        i,
                                        egui::vec2(stat_label_w, row_h),
                                        stats,
                                    );
                                }
                            });
                            left_rect = left.response.rect;

                            ui.add_space(side_margin);
                            ScrollArea::horizontal()
                                .id_salt("exalt_matrix")
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.spacing_mut().item_spacing = spacing;
                                    ui.vertical(|ui| {
                                        // Class-header row across the top.
                                        ui.horizontal(|ui| {
                                            for &cid in &classes {
                                                let resp = self.class_button(
                                                    ui,
                                                    ctx.sprite_renderer,
                                                    cid,
                                                    egui::vec2(class_w, class_h),
                                                    selected == Some(cid),
                                                    show_names,
                                                    stats,
                                                );
                                                if resp.clicked() {
                                                    clicked = Some(cid);
                                                }
                                                if selected == Some(cid) && self.scroll_to_selected
                                                {
                                                    resp.scroll_to_me(Some(egui::Align::Center));
                                                }
                                            }
                                        });
                                        for i in 0..8 {
                                            ui.horizontal(|ui| {
                                                for &cid in &classes {
                                                    let values = self.cell_values(stats, cid);
                                                    Self::cell(
                                                        ui,
                                                        values[i].0,
                                                        values[i].1,
                                                        values[i].2,
                                                        values[i].3,
                                                        self.cell_colors,
                                                        egui::vec2(class_w, row_h),
                                                    );
                                                }
                                            });
                                        }
                                    });
                                });
                        });

                        // Full-height dividers flanking the stat column, each
                        // `side_margin` away from the frames so both gaps are equal.
                        let stroke = egui::Stroke::new(1.0_f32, Color32::from_white_alpha(40));
                        let y = left_rect.top()..=ui.max_rect().bottom();
                        ui.painter()
                            .vline(left_rect.left() - side_margin, y.clone(), stroke);
                        ui.painter()
                            .vline(left_rect.right() + side_margin, y, stroke);
                    });
            }
        }

        clicked
    }

    /// Render the per-class detail page (header + in-game exalt menu).
    fn render_detail(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &mut PanelContext,
        class_id: i32,
        stats: &HashMap<i32, ClassExaltation>,
    ) {
        let total = Self::class_total(stats, class_id);
        let fully_exalted = total >= exalt_proficiency::MAX_CLASS_LEVELS;
        let skin_id = if fully_exalted {
            self.exalted_skin_id(class_id)
        } else {
            class_id
        };
        let class_name = CharacterClass::from_id(class_id as u16).name();

        ScrollArea::vertical()
            .id_salt("exalt_detail")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::hover());
                    ctx.sprite_renderer
                        .draw_outlined_sprite_in_rect(ui, skin_id, rect);
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(class_name)
                            .strong()
                            .size(16.0)
                            .color(Color32::WHITE),
                    );
                    if fully_exalted {
                        ui.label(
                            RichText::new("EXALTED")
                                .strong()
                                .size(16.0)
                                .color(exalt_colors::AMBER),
                        );
                    } else {
                        ui.label(
                            RichText::new(format!("{total}/40"))
                                .size(16.0)
                                .color(exalt_colors::BLUE),
                        );
                    }
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.set_max_width(320.0);
                        ExaltView::new(stats).render_menu_compact(
                            ui,
                            class_id,
                            ctx.sprite_renderer,
                        );
                    });
                });
            });
    }
}

impl Panel for ExaltationsPanel {
    fn show(&mut self, ui: &mut egui::Ui, ctx: &mut PanelContext) -> Vec<AppAction> {
        // Clone the stats out of the borrowed context so we can pass a stable
        // reference while still mutating `self` (sprite caches) during render.
        let stats = ctx.account_data.characters.exaltation_stats.clone();
        let (cur, max) = exalt_proficiency::total_progress(&stats);

        self.render_header(ui, ctx);

        if stats.is_empty() {
            empty_state(
                ui,
                "No exaltation data yet",
                Some("Open the character menu in-game (or refresh characters) to load exaltation progress."),
            );
            return Vec::new();
        }

        // Always show the selected class on the left; select the first class in
        // the current sort order when nothing is selected yet.
        let sorted = self.sorted_classes(&stats);
        if self.selected_class.is_none() {
            self.selected_class = sorted.first().copied();
        }

        // Arrow keys move the selection through the sorted list, wrapping.
        // Down/Right advance, Up/Left go back (so both layouts feel natural).
        let nav = ui.input(|i| {
            (i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::ArrowDown)) as i32
                - (i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::ArrowUp)) as i32
        });
        if nav != 0 && !sorted.is_empty() {
            let cur = self
                .selected_class
                .and_then(|c| sorted.iter().position(|&x| x == c))
                .unwrap_or(0);
            let n = sorted.len() as i32;
            let next = ((cur as i32 + nav) % n + n) % n;
            self.selected_class = Some(sorted[next as usize]);
            self.scroll_to_selected = true;
        }

        let avail_h = ui.available_height();
        ui.horizontal_top(|ui| {
            let detail_w = 296.0_f32;
            ui.allocate_ui_with_layout(
                egui::vec2(detail_w, avail_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    if let Some(class_id) = self.selected_class {
                        self.render_detail(ui, ctx, class_id, &stats);
                    }
                },
            );
            // Vertical layout's leftmost matrix column is the class buttons, so
            // it gets the standard detail/matrix separator. The horizontal
            // layout draws its own left divider around the stat column (in
            // `render_matrix`) so both stat-column margins stay symmetric.
            if self.layout == ExaltLayout::ClassesVertical {
                ui.separator();
            }
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), avail_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    if let Some(clicked) = self.render_matrix(ui, ctx, &stats, cur, max) {
                        self.selected_class = Some(clicked);
                    }
                },
            );
        });

        self.scroll_to_selected = false;
        Vec::new()
    }
}

//! Shared in-game "Exaltations" menu rendering, reused by the
//! Characters tab and the Exaltations tab's class-detail page.
//!
//! [`ExaltView`] borrows the account's per-class exaltation stats and renders
//! the Total Progress bar, the four proficiency squares (Fast Learner / Mastery
//! / Armor / Weapon) and the per-stat "Stat Bonuses" list.

use crate::ui_ext::HoverTooltipExt;
use std::collections::HashMap;

use eframe::egui::{self, Color32, RichText};
use realmhound_core::{
    api::{CharacterClass, ClassExaltation},
    loot::class_ids,
    stats::exalt_proficiency,
};

use crate::rendering::SpriteRenderer;

/// Format a value expressed in tenths as a fixed-1-decimal string
/// (e.g. `75 -> "7.5"`, `20 -> "2"`).
pub fn fmt_tenths(tenths: u32) -> String {
    if tenths % 10 == 0 {
        format!("{}", tenths / 10)
    } else {
        format!("{}.{}", tenths / 10, tenths % 10)
    }
}

/// In-game exaltation menu palette (color references).
pub mod exalt_colors {
    use eframe::egui::Color32;
    pub const BLUE: Color32 = Color32::from_rgb(4, 151, 255); // #0497ff
    pub const GREEN: Color32 = Color32::from_rgb(122, 193, 66); // #7ac142
    pub const AMBER: Color32 = Color32::from_rgb(255, 191, 0); // #ffbf00
    pub const DIM: Color32 = Color32::from_rgb(62, 64, 72);
    pub const GRAYTXT: Color32 = Color32::from_rgb(150, 152, 160);
    pub const NODE_EMPTY: Color32 = Color32::from_rgb(40, 42, 50);
}

/// Stat display order used throughout the exaltation menu.
pub const EXALT_STAT_LABELS: [&str; 8] = ["ATT", "DEF", "SPD", "DEX", "VIT", "WIS", "HP", "MP"];

/// Blend two colors in sRGB space (`t = 0` yields `a`, `t = 1` yields `b`).
fn mix_rgb(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// Theme-tied dark surfaces for the exaltation menu. Derived from the active
/// egui visuals so the squares and stat rows tint with the selected palette
/// instead of using fixed near-black colors.
#[derive(Clone, Copy)]
pub struct ExaltSurfaces {
    pub fill: Color32,
    pub hover: Color32,
    pub track: Color32,
    pub dim: Color32,
}

impl ExaltSurfaces {
    pub fn from_visuals(v: &egui::Visuals) -> Self {
        let bg = v.panel_fill;
        let fg = v.widgets.noninteractive.fg_stroke.color;
        Self {
            fill: mix_rgb(bg, Color32::BLACK, 0.12),
            hover: mix_rgb(bg, fg, 0.06),
            track: mix_rgb(bg, Color32::BLACK, 0.55),
            dim: mix_rgb(bg, fg, 0.28),
        }
    }
}

/// Data backing a single proficiency square's hover tooltip.
struct ProfTooltip {
    title: &'static str,
    level: u8,
    max_level: u8,
    node_labels: Vec<String>,
    requirement: Option<String>,
    desc_pre: String,
    desc_hi: &'static str,
    desc_post: String,
    /// Class ids to show in a per-stat breakdown grid (armor/weapon only).
    group: Option<(&'static str, Vec<i32>)>,
}

/// Borrows the account's per-class exaltation stats to render the in-game
/// exaltation menu for a single class.
pub struct ExaltView<'a> {
    stats: &'a HashMap<i32, ClassExaltation>,
}

impl<'a> ExaltView<'a> {
    pub fn new(stats: &'a HashMap<i32, ClassExaltation>) -> Self {
        Self { stats }
    }

    /// Render the in-game style "Exaltations" menu without the "EXALTATIONS"
    /// title or Total Progress bar: the four proficiency squares (Fast Learner
    /// / Mastery / Armor / Weapon) for the viewed class, then the per-stat
    /// "Stat Bonuses" list (used by the Exaltations tab, which shows the title
    /// and total elsewhere and needs a compact side panel).
    pub fn render_menu_compact(
        &self,
        ui: &mut egui::Ui,
        class_id: i32,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        self.render_menu_inner(ui, class_id, sprite_renderer, false);
    }

    fn render_menu_inner(
        &self,
        ui: &mut egui::Ui,
        class_id: i32,
        sprite_renderer: &mut SpriteRenderer,
        show_header: bool,
    ) {
        use exalt_colors::*;

        let default = ClassExaltation::default();
        let exalt = self.stats.get(&class_id).unwrap_or(&default);

        let class_name = CharacterClass::from_id(class_id as u16).name();
        let armor_label = class_ids::armor_label(class_id);
        let weapon_label = class_ids::weapon_label(class_id);

        // Keep the menu aligned with the rest of the column instead of
        // stretching across all available width.
        let menu_width = ui.available_width().min(280.0);

        ui.vertical(|ui| {
            ui.set_max_width(menu_width);
            if show_header {
                ui.label(
                    RichText::new("EXALTATIONS")
                        .strong()
                        .size(16.0)
                        .color(Color32::from_rgb(150, 200, 255)),
                );
                ui.add_space(4.0);

                // Total progress across all canonical classes.
                let (cur, max) = exalt_proficiency::total_progress(self.stats);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Total Progress")
                            .strong()
                            .size(14.0)
                            .color(Color32::WHITE),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{cur}/{max}"))
                                .strong()
                                .size(14.0)
                                .color(BLUE),
                        );
                    });
                });
                ui.add_space(2.0);
                Self::draw_total_progress_bar(ui, cur, max);
                ui.add_space(10.0);
            }

            // Precompute proficiency values.
            let fl_lvl = exalt_proficiency::fast_learner_level(exalt);
            let fl_pct = exalt_proficiency::fast_learner_xp_pct(exalt);
            let ma_lvl = exalt_proficiency::mastery_level(exalt);
            let ma_tenths = exalt_proficiency::mastery_dmg_tenths(exalt);
            let ar_lvl = exalt_proficiency::armor_proficiency_level(self.stats, class_id);
            let ar_tenths = exalt_proficiency::armor_ic_tenths(self.stats, class_id);
            let we_lvl = exalt_proficiency::weapon_proficiency_level(self.stats, class_id);
            let we_pct = exalt_proficiency::weapon_dr_pct(self.stats, class_id);

            let surf = ExaltSurfaces::from_visuals(ui.visuals());

            const INNER_GAP: f32 = 6.0;
            const SEP_PAD: f32 = 6.0;
            const SEP_W: f32 = 2.0;
            const GROUP_GAP: f32 = SEP_PAD * 2.0 + SEP_W;

            // Size the four squares so the row exactly spans the progress
            // bar width above it.
            let avail = ui.available_width();
            let sq = ((avail - INNER_GAP - 2.0 * GROUP_GAP) / 4.0).max(24.0);

            // Group-header row. Each label is left-aligned to its group's
            // leading square (class over XP, armor over IC, weapon over DR).
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                Self::col_header(ui, class_name, sq * 2.0 + INNER_GAP, GRAYTXT);
                ui.add_space(GROUP_GAP);
                Self::col_header(ui, armor_label, sq, GRAYTXT);
                ui.add_space(GROUP_GAP);
                Self::col_header(ui, weapon_label, sq, GRAYTXT);
            });

            // Squares row.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                self.prof_square(
                    ui,
                    sprite_renderer,
                    "XP",
                    &format!("+{fl_pct}%"),
                    sq,
                    true,
                    surf,
                    ProfTooltip {
                        title: "Fast Learner",
                        level: fl_lvl,
                        max_level: 4,
                        node_labels: vec![
                            "+5%".into(),
                            "+10%".into(),
                            "+15%".into(),
                            "+20%".into(),
                        ],
                        requirement: None,
                        desc_pre: format!("{class_name} class receives "),
                        desc_hi: "extra XP",
                        desc_post: ".".into(),
                        group: None,
                    },
                );
                ui.add_space(INNER_GAP);
                self.prof_square(
                    ui,
                    sprite_renderer,
                    "DMG",
                    &format!("+{}%", fmt_tenths(ma_tenths)),
                    sq,
                    true,
                    surf,
                    ProfTooltip {
                        title: "Mastery",
                        level: ma_lvl,
                        max_level: 4,
                        node_labels: vec![
                            "+2.5%".into(),
                            "+5%".into(),
                            "+7.5%".into(),
                            "+10%".into(),
                        ],
                        requirement: Some(format!(
                            "ACHIEVE {} STAT BONUSES ON ALL STATS ON {}",
                            (ma_lvl + 1).min(4),
                            class_name.to_uppercase()
                        )),
                        desc_pre: format!("{class_name} class inflicts "),
                        desc_hi: "more Damage",
                        desc_post: " to enemies.".into(),
                        group: None,
                    },
                );
                ui.add_space(SEP_PAD);
                Self::group_separator(ui, SEP_W, sq, surf);
                ui.add_space(SEP_PAD);
                let ic_value = if ar_tenths == 0 {
                    "0s".to_string()
                } else {
                    format!("-{}s", fmt_tenths(ar_tenths))
                };
                self.prof_square(
                    ui,
                    sprite_renderer,
                    "IC",
                    &ic_value,
                    sq,
                    false,
                    surf,
                    ProfTooltip {
                        title: "Armor Proficiency",
                        level: ar_lvl,
                        max_level: 5,
                        node_labels: vec![
                            "-0.2s".into(),
                            "-0.4s".into(),
                            "-0.6s".into(),
                            "-0.8s".into(),
                            "-1s".into(),
                        ],
                        requirement: Some(format!(
                            "ACHIEVE {} STAT BONUSES ON ALL STATS FOR {}-ARMORED CLASSES",
                            (ar_lvl + 1).min(5),
                            armor_label.to_uppercase()
                        )),
                        desc_pre: format!(
                            "{armor_label}-armored classes disengage faster from the "
                        ),
                        desc_hi: "In Combat",
                        desc_post: " condition.".into(),
                        group: Some(("armor", class_ids::armor_classes(class_id))),
                    },
                );
                ui.add_space(SEP_PAD);
                Self::group_separator(ui, SEP_W, sq, surf);
                ui.add_space(SEP_PAD);
                self.prof_square(
                    ui,
                    sprite_renderer,
                    "DR",
                    &format!("+{we_pct}%"),
                    sq,
                    false,
                    surf,
                    ProfTooltip {
                        title: "Weapon Proficiency",
                        level: we_lvl,
                        max_level: 5,
                        node_labels: vec![
                            "+5%".into(),
                            "+10%".into(),
                            "+15%".into(),
                            "+20%".into(),
                            "+25%".into(),
                        ],
                        requirement: Some(format!(
                            "ACHIEVE {} STAT BONUSES ON ALL STATS FOR {} CLASSES",
                            (we_lvl + 1).min(5),
                            weapon_label.to_uppercase()
                        )),
                        desc_pre: format!("{weapon_label} classes receive "),
                        desc_hi: "bonus loot drop",
                        desc_post: " chances.".into(),
                        group: Some(("weapon", class_ids::weapon_classes(class_id))),
                    },
                );
            });

            ui.add_space(10.0);
            self.render_stat_bonuses(ui, exalt, sprite_renderer);
        });
    }

    /// Paint a fixed-width, left-aligned column header above the squares.
    fn col_header(ui: &mut egui::Ui, text: &str, width: f32, color: Color32) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 18.0), egui::Sense::hover());
        ui.painter().text(
            rect.left_center(),
            egui::Align2::LEFT_CENTER,
            text,
            egui::FontId::proportional(13.0),
            color,
        );
    }

    /// Paint a thin vertical separator between proficiency groups.
    fn group_separator(ui: &mut egui::Ui, width: f32, height: f32, surf: ExaltSurfaces) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        let x = rect.center().x;
        ui.painter().line_segment(
            [
                egui::pos2(x, rect.top() + 4.0),
                egui::pos2(x, rect.bottom() - 4.0),
            ],
            egui::Stroke::new(2.0_f32, surf.dim),
        );
    }

    /// Draw the Total Progress bar as five blue-filled segments over a dark track.
    pub fn draw_total_progress_bar(ui: &mut egui::Ui, cur: u32, max: u32) {
        use exalt_colors::*;
        let track = ExaltSurfaces::from_visuals(ui.visuals()).track;
        let width = ui.available_width();
        let height = 12.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        let painter = ui.painter();

        let frac = if max > 0 {
            (cur as f32 / max as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let segments = 5usize;
        let gap = 4.0;
        let seg_w = (width - gap * (segments as f32 - 1.0)) / segments as f32;
        for i in 0..segments {
            let x0 = rect.left() + i as f32 * (seg_w + gap);
            let seg =
                egui::Rect::from_min_size(egui::pos2(x0, rect.top()), egui::vec2(seg_w, height));
            painter.rect_filled(seg, 2.0, track);
            // Fill this segment according to overall progress.
            let seg_start = i as f32 / segments as f32;
            let seg_end = (i + 1) as f32 / segments as f32;
            if frac > seg_start {
                let local = ((frac - seg_start) / (seg_end - seg_start)).clamp(0.0, 1.0);
                let fill = egui::Rect::from_min_size(seg.min, egui::vec2(seg_w * local, height));
                let color = if cur >= max { GREEN } else { BLUE };
                painter.rect_filled(fill, 2.0, color);
            }
        }
    }

    /// Draw a single proficiency square (dark box with a segmented progress
    /// border, a big value and an amber tag) plus its in-game style hover card.
    fn prof_square(
        &self,
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        tag: &str,
        value: &str,
        size: f32,
        cardinal_gaps: bool,
        surf: ExaltSurfaces,
        tt: ProfTooltip,
    ) {
        use exalt_colors::*;

        let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        let painter = ui.painter();

        let bg = if response.hovered() {
            surf.hover
        } else {
            surf.fill
        };
        painter.rect_filled(rect, 6.0, bg);
        Self::draw_segmented_border(
            painter,
            rect.shrink(3.0),
            tt.max_level,
            tt.level,
            cardinal_gaps,
            BLUE,
            surf.dim,
        );

        // Value (white) and tag (amber).
        painter.text(
            rect.center() - egui::vec2(0.0, 6.0),
            egui::Align2::CENTER_CENTER,
            value,
            egui::FontId::proportional(15.0),
            Color32::WHITE,
        );
        painter.text(
            rect.center() + egui::vec2(0.0, 12.0),
            egui::Align2::CENTER_CENTER,
            tag,
            egui::FontId::proportional(11.0),
            AMBER,
        );

        response.hover_tip_ui(|ui| {
            let surf = ExaltSurfaces::from_visuals(ui.visuals());
            // Grow the tooltip to fit its widest content (the group grid, for
            // Armor/Weapon). Measured content width is cached across frames so
            // the header band and node track span the full width.
            let tw_id = ui.make_persistent_id(("exalt_tt_w", tt.title));
            let tw = ui
                .data(|d| d.get_temp::<f32>(tw_id))
                .unwrap_or(300.0)
                .max(300.0);
            ui.set_max_width(tw);

            // Header band: fill everything from the tooltip's top edge down to a
            // line, bleeding into the frame's menu_margin so there are no gaps.
            let margin = ui.style().spacing.menu_margin;
            let inset = ui.visuals().window_stroke.width;
            let (content, _) = ui.allocate_exact_size(egui::vec2(tw, 26.0), egui::Sense::hover());
            let band = egui::Rect::from_min_max(
                egui::pos2(
                    content.left() - margin.left as f32 + inset,
                    content.top() - margin.top as f32 + inset,
                ),
                egui::pos2(
                    content.right() + margin.right as f32 - inset,
                    content.bottom(),
                ),
            );
            let cr = ui.visuals().menu_corner_radius;
            let top_round = egui::CornerRadius {
                nw: cr.nw,
                ne: cr.ne,
                sw: 0,
                se: 0,
            };
            let painter = ui.painter().with_clip_rect(band);
            painter.rect_filled(band, top_round, surf.fill);
            painter.text(
                egui::pos2(content.left(), content.center().y),
                egui::Align2::LEFT_CENTER,
                tt.title,
                egui::FontId::proportional(16.0),
                Color32::WHITE,
            );
            let lvl_color = if tt.level >= tt.max_level {
                GREEN
            } else {
                BLUE
            };
            painter.text(
                egui::pos2(content.right(), content.center().y),
                egui::Align2::RIGHT_CENTER,
                format!("{}/{}", tt.level, tt.max_level),
                egui::FontId::proportional(16.0),
                lvl_color,
            );
            if let Some(req) = &tt.requirement {
                ui.add_space(2.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(req).strong().size(12.0).color(Color32::WHITE));
                });
            }
            ui.add_space(6.0);
            let track_color = if tt.level >= tt.max_level {
                GREEN
            } else {
                BLUE
            };
            Self::draw_node_track(ui, tt.level, tt.max_level, &tt.node_labels, track_color, tw);
            ui.add_space(6.0);
            // Description with an amber keyword.
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.label(RichText::new(&tt.desc_pre).size(13.0).color(GRAYTXT));
                ui.label(RichText::new(tt.desc_hi).size(13.0).color(AMBER));
                ui.label(RichText::new(&tt.desc_post).size(13.0).color(GRAYTXT));
            });
            if let Some((salt, group)) = &tt.group {
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(4.0);
                self.draw_group_grid(ui, sprite_renderer, salt, group);
            }
            let measured = ui.min_rect().width();
            ui.data_mut(|d| d.insert_temp(tw_id, measured));
        });
    }

    /// Draw a rounded-rect border split into `segments` arcs, lighting the
    /// first `filled` in `active` and the rest in `dim` (the square's progress
    /// ring). When `cardinal_gaps` is set, the boundaries are offset so the gaps
    /// land on the side midpoints instead of the corners.
    fn draw_segmented_border(
        painter: &egui::Painter,
        rect: egui::Rect,
        segments: u8,
        filled: u8,
        cardinal_gaps: bool,
        active: Color32,
        dim: Color32,
    ) {
        let segments = segments.max(1);
        let r = 6.0_f32.min(rect.width().min(rect.height()) * 0.5);
        let perim = Self::rounded_perimeter(rect, r);
        let seg_len = perim / segments as f32;
        let gap = seg_len * 0.16;
        // Cardinal gaps: start a boundary at the top-edge midpoint.
        let offset = if cardinal_gaps {
            (rect.width() - 2.0 * r) * 0.5
        } else {
            0.0
        };
        for i in 0..segments {
            let t0 = offset + i as f32 * seg_len + gap * 0.5;
            let t1 = offset + (i + 1) as f32 * seg_len - gap * 0.5;
            let color = if i < filled { active } else { dim };
            let steps = 20;
            let pts: Vec<egui::Pos2> = (0..=steps)
                .map(|s| {
                    let d = t0 + (t1 - t0) * (s as f32 / steps as f32);
                    Self::rounded_perimeter_point(rect, d, r)
                })
                .collect();
            painter.add(egui::Shape::line(pts, egui::Stroke::new(4.5_f32, color)));
        }
    }

    /// Total path length of a rounded rectangle with corner radius `r`.
    fn rounded_perimeter(rect: egui::Rect, r: f32) -> f32 {
        let w = rect.width();
        let h = rect.height();
        2.0 * (w - 2.0 * r) + 2.0 * (h - 2.0 * r) + 2.0 * std::f32::consts::PI * r
    }

    /// Point at clockwise distance `d` along a rounded rectangle's path,
    /// starting where the top edge begins (just after the top-left corner arc).
    fn rounded_perimeter_point(rect: egui::Rect, d: f32, r: f32) -> egui::Pos2 {
        let hs = rect.width() - 2.0 * r; // horizontal straight length
        let vs = rect.height() - 2.0 * r; // vertical straight length
        let arc = std::f32::consts::FRAC_PI_2 * r;
        let perim = 2.0 * hs + 2.0 * vs + 4.0 * arc;
        let mut t = d.rem_euclid(perim);
        let l = rect.left();
        let top = rect.top();
        let right = rect.right();
        let bottom = rect.bottom();

        if t < hs {
            return egui::pos2(l + r + t, top);
        }
        t -= hs;
        if t < arc {
            let a = -std::f32::consts::FRAC_PI_2 + t / r;
            return egui::pos2(right - r + r * a.cos(), top + r + r * a.sin());
        }
        t -= arc;
        if t < vs {
            return egui::pos2(right, top + r + t);
        }
        t -= vs;
        if t < arc {
            let a = t / r;
            return egui::pos2(right - r + r * a.cos(), bottom - r + r * a.sin());
        }
        t -= arc;
        if t < hs {
            return egui::pos2(right - r - t, bottom);
        }
        t -= hs;
        if t < arc {
            let a = std::f32::consts::FRAC_PI_2 + t / r;
            return egui::pos2(l + r + r * a.cos(), bottom - r + r * a.sin());
        }
        t -= arc;
        if t < vs {
            return egui::pos2(l, bottom - r - t);
        }
        t -= vs;
        let a = std::f32::consts::PI + t / r;
        egui::pos2(l + r + r * a.cos(), top + r + r * a.sin())
    }

    /// Draw the horizontal node track shown in a proficiency tooltip: a line
    /// with a diamond node per level, filled up to the current level. The track
    /// spans `width` so it stretches across the tooltip.
    fn draw_node_track(
        ui: &mut egui::Ui,
        level: u8,
        max_level: u8,
        labels: &[String],
        filled_color: Color32,
        width: f32,
    ) {
        use exalt_colors::*;
        let n = max_level as usize;
        let height = 42.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        let painter = ui.painter();
        let y = rect.top() + 12.0;
        let left_pad = 14.0;
        let right_pad = 14.0;
        let x0 = rect.left() + left_pad;
        let x_end = rect.right() - right_pad;
        let spacing = if n > 0 { (x_end - x0) / n as f32 } else { 0.0 };

        painter.line_segment(
            [egui::pos2(x0, y), egui::pos2(x_end, y)],
            egui::Stroke::new(4.0_f32, DIM),
        );
        if level > 0 {
            let xf = x0 + spacing * (level as f32).min(n as f32);
            painter.line_segment(
                [egui::pos2(x0, y), egui::pos2(xf, y)],
                egui::Stroke::new(4.0_f32, filled_color),
            );
        }
        // Origin marker.
        Self::draw_diamond(painter, egui::pos2(x0, y), 5.0, BLUE, egui::Stroke::NONE);
        for i in 1..=n {
            let x = x0 + spacing * i as f32;
            let achieved = (i as u8) <= level;
            let fill = if achieved { filled_color } else { NODE_EMPTY };
            let stroke = egui::Stroke::new(1.5_f32, if achieved { filled_color } else { DIM });
            Self::draw_diamond(painter, egui::pos2(x, y), 8.0, fill, stroke);
            if let Some(lbl) = labels.get(i - 1) {
                painter.text(
                    egui::pos2(x, y + 14.0),
                    egui::Align2::CENTER_TOP,
                    lbl,
                    egui::FontId::proportional(11.0),
                    if achieved { Color32::WHITE } else { GRAYTXT },
                );
            }
        }
    }

    fn draw_diamond(
        painter: &egui::Painter,
        c: egui::Pos2,
        r: f32,
        fill: Color32,
        stroke: egui::Stroke,
    ) {
        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(c.x, c.y - r),
                egui::pos2(c.x + r, c.y),
                egui::pos2(c.x, c.y + r),
                egui::pos2(c.x - r, c.y),
            ],
            fill,
            stroke,
        ));
    }

    /// Draw the per-class, per-stat breakdown grid shown in armor/weapon
    /// proficiency tooltips: a class sprite followed by each stat's exalt level.
    fn draw_group_grid(
        &self,
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        salt: &str,
        group: &[i32],
    ) {
        use exalt_colors::*;
        egui::Grid::new(("exalt_group_grid", salt))
            .num_columns(9)
            .spacing([8.0, 5.0])
            .show(ui, |ui| {
                ui.label("");
                for s in EXALT_STAT_LABELS {
                    ui.label(RichText::new(s).strong().size(11.0).color(GRAYTXT));
                }
                ui.end_row();

                for &cid in group {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::hover());
                    sprite_renderer.draw_outlined_sprite_in_rect(ui, cid, rect);
                    let levels = self
                        .stats
                        .get(&cid)
                        .map(exalt_proficiency::stat_levels)
                        .unwrap_or([0; 8]);
                    for lvl in levels {
                        if lvl >= 5 {
                            ui.label(RichText::new(format!("{lvl}/5")).size(11.0).color(GREEN));
                        } else {
                            let mut job = egui::text::LayoutJob::default();
                            let font = egui::FontId::proportional(11.0);
                            job.append(
                                &format!("{lvl}"),
                                0.0,
                                egui::TextFormat {
                                    font_id: font.clone(),
                                    color: BLUE,
                                    ..Default::default()
                                },
                            );
                            job.append(
                                "/5",
                                0.0,
                                egui::TextFormat {
                                    font_id: font,
                                    color: Color32::WHITE,
                                    ..Default::default()
                                },
                            );
                            ui.label(job);
                        }
                    }
                    ui.end_row();
                }
            });
    }

    /// Render exaltation progress for the class with in-game style UI.
    pub fn render_stat_bonuses(
        &self,
        ui: &mut egui::Ui,
        exalt: &ClassExaltation,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        ui.label(
            RichText::new("Stat Bonuses")
                .strong()
                .size(14.0)
                .color(Color32::WHITE),
        );
        ui.add_space(4.0);

        // Stat definitions: (name, raw_value, potion_id, portal_id)
        // Potion IDs: ATT=2591, DEF=2592, SPD=2593, DEX=2636, VIT=2612, WIS=2613, Life=2793, Mana=2794
        // Portal IDs: Spectral=23691, LostHalls=45092, Cultist=45155, Nest=4259, Kogbold=49433, Fungal=45679, O3=6218, Void=45075
        let stats: [(&str, i32, i32, i32); 8] = [
            ("ATT", exalt.attack as i32, 2591, 23691), // Spectral Penitentiary
            ("DEF", exalt.defense as i32, 2592, 45092), // Lost Halls
            ("SPD", exalt.speed as i32, 2593, 45155),  // Cultist Hideout
            ("DEX", exalt.dexterity as i32, 2636, 4259), // The Nest
            ("VIT", exalt.vitality as i32, 2612, 49433), // Kogbold Steamworks
            ("WIS", exalt.wisdom as i32, 2613, 45679), // Fungal Cavern
            ("HP", exalt.hp as i32, 2793, 6218),       // Oryx's Sanctuary
            ("MP", exalt.mp as i32, 2794, 45075),      // The Void
        ];

        // Thresholds: 0, 5, 15, 30, 50, 75
        let thresholds = [0, 5, 15, 30, 50, 75];
        let needed_per_level = [5, 10, 15, 20, 25]; // Completions needed: 0->1, 1->2, 2->3, 3->4, 4->5

        let surf = ExaltSurfaces::from_visuals(ui.visuals());
        let row_bg = surf.fill;
        let bar_track = surf.track;
        let card_w = ui.available_width();
        let card_h = 40.0;
        let pad = 8.0;
        let icon = 18.0;

        for (name, raw, potion_id, portal_id) in stats {
            let level = if raw >= 75 {
                5
            } else if raw >= 50 {
                4
            } else if raw >= 30 {
                3
            } else if raw >= 15 {
                2
            } else if raw >= 5 {
                1
            } else {
                0
            };

            let (progress, max_progress) = if level >= 5 {
                (25, 25)
            } else {
                let threshold = thresholds[level as usize];
                let needed = needed_per_level[level as usize];
                ((raw - threshold) as i32, needed)
            };
            let frac = if max_progress > 0 {
                (progress as f32 / max_progress as f32).clamp(0.0, 1.0)
            } else {
                1.0
            };

            let (card, _resp) =
                ui.allocate_exact_size(egui::vec2(card_w, card_h), egui::Sense::hover());

            ui.painter().rect_filled(card, 6.0, row_bg);

            // Progress bar: thin, square, full card width, near the bottom.
            let bar_h = 5.0;
            let bar_rect = egui::Rect::from_min_size(
                egui::pos2(card.left() + pad, card.bottom() - 7.0 - bar_h),
                egui::vec2(card.width() - 2.0 * pad, bar_h),
            );
            ui.painter().rect_filled(bar_rect, 0.0, bar_track);
            if frac > 0.0 {
                let fill = egui::Rect::from_min_size(
                    bar_rect.min,
                    egui::vec2(bar_rect.width() * frac, bar_h),
                );
                let color = if level >= 5 {
                    exalt_colors::GREEN
                } else {
                    exalt_colors::BLUE
                };
                ui.painter().rect_filled(fill, 0.0, color);
            }

            // Top content row (centered above the bar).
            let cy = card.top() + (bar_rect.top() - card.top()) * 0.5;

            let potion_rect = egui::Rect::from_center_size(
                egui::pos2(card.left() + pad + icon * 0.5, cy),
                egui::vec2(icon, icon),
            );
            sprite_renderer.draw_sprite_in_rect(ui, potion_id, potion_rect);
            ui.painter().text(
                egui::pos2(potion_rect.right() + 6.0, cy),
                egui::Align2::LEFT_CENTER,
                format!("{name} {level}/5"),
                egui::FontId::proportional(13.0),
                Color32::WHITE,
            );

            let portal_rect = egui::Rect::from_center_size(
                egui::pos2(card.right() - pad - icon * 0.5, cy),
                egui::vec2(icon, icon),
            );
            sprite_renderer.draw_sprite_in_rect(ui, portal_id, portal_rect);
            let exalted = level >= 5;
            let right_text = if exalted {
                "EXALTED".to_string()
            } else {
                format!("{progress}/{max_progress}")
            };
            let right_color = if exalted {
                Color32::from_rgb(0x04, 0x97, 0xff)
            } else {
                exalt_colors::GRAYTXT
            };
            ui.painter().text(
                egui::pos2(portal_rect.left() - 6.0, cy),
                egui::Align2::RIGHT_CENTER,
                right_text,
                egui::FontId::proportional(12.0),
                right_color,
            );

            ui.add_space(4.0);
        }
    }
}

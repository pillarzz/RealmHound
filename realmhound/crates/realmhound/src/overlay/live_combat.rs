//! Live Combat overlay body.
//!
//! Shows the boss fight the player is currently on. The list is produced by
//! [`realmhound_core::combat::CombatTracker::live_snapshots`]: a single boss,
//! or one tab per boss when several are fought in parallel (Stone Guardians,
//! Tomb of the Ancients, ...). Engaging any other boss replaces it.
//!
//! Each tab is a mid-fight projection, so the local player's own damage can
//! still be an estimate (see [`DamageProvenance`]) -- their row is tinted to say
//! so rather than quietly presenting a guess as fact.

use std::sync::Arc;

use eframe::egui::{self, Color32, RichText};
use realmhound_core::combat::{DamageProvenance, FightParticipant, LiveFightSnapshot};

use crate::rendering::SpriteRenderer;

use super::{
    overlay_tooltip, overlay_tooltip_with, row_text, scrollbar_hit, thousands, HitRegions,
    OverlayContent,
};

/// Side of the per-row class sprite.
const ICON: f32 = 16.0;
/// Side of the boss sprite beside the HP bar.
const BOSS_ICON: f32 = 14.0;
/// Facing used for every character sprite, matching the Combat History rows.
const SPRITE_DIRECTION: i32 = 6;
/// Fill behind a row, scaled against the top dealer's damage.
const SHARE_FILL: Color32 = Color32::from_rgb(60, 90, 150);
/// Damage-bar toggle icon while the bars are shown.
const BARS_ICON_ON: Color32 = Color32::from_rgb(120, 160, 230);
/// Height of the boss header bar.
const HEADER_H: f32 = 18.0;
/// Remaining boss HP in the header bar, and the track behind it.
const BOSS_HP_FILL: Color32 = Color32::from_rgb(180, 70, 70);
const BOSS_HP_TRACK: Color32 = Color32::from_rgb(40, 40, 46);
/// Gap between the end of a full-length bar and the percentage column.
const BAR_GAP: f32 = 4.0;
/// Row tint for the local player.
const LOCAL_COLOR: Color32 = Color32::from_rgb(120, 215, 130);
/// Row tint for the local player when their own figure is still an estimate or
/// a lower bound. Deliberately never applied to anyone else: an amber remote row
/// reads as a warning about *that player*, which it is not.
const UNCERTAIN_COLOR: Color32 = Color32::from_rgb(230, 190, 110);
const NORMAL_COLOR: Color32 = Color32::from_rgb(225, 225, 230);
/// Boss tab text, fill and border: idle, hovered and selected.
const TAB_TEXT_IDLE: Color32 = Color32::from_gray(165);
const TAB_TEXT_HOVER: Color32 = Color32::from_gray(215);
const TAB_TEXT_ACTIVE: Color32 = Color32::from_rgb(220, 230, 250);
const TAB_FILL_IDLE: Color32 = Color32::from_gray(32);
const TAB_FILL_HOVER: Color32 = Color32::from_gray(48);
const TAB_FILL_ACTIVE: Color32 = Color32::from_rgb(42, 58, 88);
const TAB_STROKE_IDLE: Color32 = Color32::from_gray(62);
const TAB_STROKE_ACTIVE: Color32 = Color32::from_rgb(110, 145, 200);
/// Dot on a tab whose fight is still running.
const TAB_LIVE: Color32 = Color32::from_rgb(210, 80, 80);
/// Height of one boss tab.
const TAB_HEIGHT: f32 = 20.0;
/// How far ahead a boss's last local hit must be before auto-follow moves to it.
/// Without the margin, hitting two bosses in alternation swaps the tab on every
/// packet batch.
const SWITCH_MARGIN_MS: i64 = 1_500;

#[derive(Default)]
pub(super) struct LiveCombatOverlay {
    /// Boss whose tab the user clicked, which overrides auto-follow until they
    /// click it again or change zone.
    pinned: Option<i32>,
    /// Boss auto-follow last settled on, kept so the hysteresis below has
    /// something to compare against.
    followed: Option<i32>,
    /// Boss ids of the last instance rendered. A zone change swaps the whole set
    /// out, which is the one moment the overlay resets its selection.
    last_ids: Vec<i32>,
    /// Width the row list's scrollbar took last frame, so the header timer and
    /// the pinned row, which sit above the scroll area, line up with the rows.
    gutter: f32,
}

/// Fixed column widths shared by every row of a fight (pinned row included),
/// so ranks, icons, damage and percentages line up vertically.
#[derive(Clone, Copy)]
struct Columns {
    /// Rank column, sized for the largest rank; ranks are right-aligned in it.
    rank: f32,
    /// Percentage column, sized for the widest share label.
    pct: f32,
}

impl Columns {
    fn for_fight(ui: &egui::Ui, fight: &LiveFightSnapshot, total: i64) -> Self {
        let rank_chars = format!("{}.", fight.participants.len().max(1)).len();
        let pct_chars = fight
            .participants
            .iter()
            .map(|p| share_label(share(p.damage, total)).len())
            .max()
            .unwrap_or(0)
            .max(3);
        let char_w = ui
            .painter()
            .layout_no_wrap(
                "0".to_owned(),
                egui::FontId::monospace(12.0),
                Color32::WHITE,
            )
            .size()
            .x;
        Self {
            rank: char_w * rank_chars as f32,
            pct: char_w * pct_chars as f32,
        }
    }

    /// Offset from the row's left edge to the icon column.
    fn icon_left(&self, ui: &egui::Ui) -> f32 {
        self.rank + ui.spacing().item_spacing.x
    }
}

/// Borrows everything one frame of the Live Combat overlay needs.
pub(super) struct LiveCombatContent<'a> {
    pub overlay: &'a mut LiveCombatOverlay,
    pub fights: &'a [Arc<LiveFightSnapshot>],
    pub follow: &'a mut bool,
    pub share_bars: &'a mut bool,
    pub class_icons: bool,
    /// False when Combat History records no boss groups, which leaves the
    /// overlay nothing to show.
    pub tracking_any: bool,
    pub max_rows: usize,
}

impl OverlayContent for LiveCombatContent<'_> {
    fn title_extras(&mut self, ui: &mut egui::Ui) {
        let color = if *self.follow {
            LOCAL_COLOR
        } else {
            Color32::from_gray(110)
        };
        let response = ui.add(
            egui::Button::new(RichText::new("me").size(11.0).color(color))
                .frame(false)
                .min_size(egui::vec2(20.0, 0.0)),
        );
        overlay_tooltip(ui, &response, "Pin my own row above the list");
        if response.clicked() {
            *self.follow = !*self.follow;
        }

        let response = bars_button(ui, *self.share_bars);
        overlay_tooltip(ui, &response, "Show damage bars behind each row");
        if response.clicked() {
            *self.share_bars = !*self.share_bars;
        }
    }

    fn body(&mut self, ui: &mut egui::Ui, sprites: &mut SpriteRenderer, hits: &mut HitRegions) {
        self.overlay.body(
            ui,
            sprites,
            self.fights,
            self.max_rows,
            *self.follow,
            *self.share_bars,
            self.class_icons,
            self.tracking_any,
            hits,
        );
    }
}

impl LiveCombatOverlay {
    fn body(
        &mut self,
        ui: &mut egui::Ui,
        sprites: &mut SpriteRenderer,
        fights: &[Arc<LiveFightSnapshot>],
        max_rows: usize,
        follow: bool,
        share_bars: bool,
        class_icons: bool,
        tracking_any: bool,
        hits: &mut HitRegions,
    ) {
        let fight: Option<&LiveFightSnapshot> = if fights.is_empty() {
            self.last_ids.clear();
            self.pinned = None;
            self.followed = None;
            None
        } else {
            self.sync_instance(fights);
            Some(
                self.pinned
                    .or(self.followed)
                    .and_then(|id| fights.iter().find(|f| f.boss_object_id == id))
                    .map(|f| &**f)
                    .unwrap_or(&fights[0]),
            )
        };

        let total = fight.map_or(1, |f| f.total_damage().max(1));
        // Bars are drawn against the top dealer so close ranks stay readable.
        let top = share_bars
            .then(|| fight.and_then(|f| f.participants.iter().map(|p| p.damage).max()))
            .flatten()
            .filter(|&d| d > 0);
        let mut pinned = None;
        let columns = fight.map(|f| Columns::for_fight(ui, f, total));
        if let (Some(fight), Some(columns)) = (fight, columns) {
            if fights.len() > 1 {
                self.tab_strip(ui, fights, fight.boss_object_id, hits);
                ui.add_space(2.0);
            }

            header(ui, sprites, fight, columns, self.gutter);
            ui.add_space(2.0);

            pinned = follow
                .then(|| fight.participants.iter().position(|p| p.is_local))
                .flatten();
            if let Some(index) = pinned {
                let size = egui::vec2((ui.available_width() - self.gutter).max(0.0), ICON);
                ui.allocate_ui(size, |ui| {
                    row(
                        ui,
                        sprites,
                        &fight.participants[index],
                        index + 1,
                        total,
                        top,
                        columns,
                        class_icons,
                        hits,
                    );
                });
                ui.add_space(2.0);
            }
        }

        let outer_width = ui.available_width();
        let output = super::scroll_rows(ui, "live_combat_rows", |ui| {
            let (Some(fight), Some(columns)) = (fight, columns) else {
                ui.vertical_centered(|ui| {
                    ui.add_space(8.0);
                    if tracking_any {
                        ui.label(row_text("No fight in progress", Color32::from_gray(120)));
                    } else {
                        ui.label(
                            RichText::new(
                                "Combat History is not tracking any boss groups. Enable \
                                 them in Settings > Combat History to see fights here.",
                            )
                            .size(11.0)
                            .color(Color32::from_gray(150)),
                        );
                    }
                });
                return;
            };
            let rows: &[FightParticipant] = if max_rows == 0 {
                &fight.participants
            } else {
                let end = max_rows.min(fight.participants.len());
                &fight.participants[..end]
            };
            for (index, participant) in rows.iter().enumerate() {
                if pinned == Some(index) {
                    continue;
                }
                row(
                    ui,
                    sprites,
                    participant,
                    index + 1,
                    total,
                    top,
                    columns,
                    class_icons,
                    hits,
                );
            }
            if max_rows > 0 && fight.participants.len() > rows.len() {
                let hidden = fight.participants.len() - rows.len();
                ui.label(row_text(format!("+{hidden} more"), Color32::from_gray(110)));
            }
        });
        let gutter = (outer_width - output.inner_rect.width()).max(0.0);
        if (gutter - self.gutter).abs() > 0.5 {
            // The header and pinned row were laid out with last frame's width.
            self.gutter = gutter;
            ui.ctx().request_repaint();
        }
        if let Some(bar) = scrollbar_hit(&output) {
            hits.add(bar);
        }
    }

    /// Point the overlay at the boss the player is currently attacking, and
    /// reset the selection when they have changed zone.
    ///
    /// A new instance shares no boss ids with the old one; within an instance
    /// fights are only ever appended, so a prefix match means "same zone, more
    /// bosses" and a pinned tab survives.
    ///
    /// Auto-follow uses the local-hit clock, not general fight activity: a boss
    /// merely standing in view refreshes the latter, which is what used to make
    /// the display flip between rows. It also requires the newcomer to be ahead
    /// by [`SWITCH_MARGIN_MS`], so alternating between two bosses settles on one
    /// instead of swapping every packet batch.
    fn sync_instance(&mut self, fights: &[Arc<LiveFightSnapshot>]) {
        let ids: Vec<i32> = fights.iter().map(|f| f.boss_object_id).collect();
        if self.last_ids.is_empty() || !ids.starts_with(&self.last_ids) {
            self.pinned = None;
            self.followed = None;
        }
        self.last_ids = ids;

        let newest = fights
            .iter()
            .filter(|f| f.last_local_hit_at > 0)
            .max_by_key(|f| f.last_local_hit_at);
        if let Some(target) = newest {
            let switch = match self.followed {
                // Nothing to displace yet: the first boss hit wins outright.
                None => true,
                Some(id) if id == target.boss_object_id => false,
                Some(id) => {
                    let incumbent = fights
                        .iter()
                        .find(|f| f.boss_object_id == id)
                        .map(|f| f.last_local_hit_at)
                        .unwrap_or(0);
                    target.last_local_hit_at >= incumbent + SWITCH_MARGIN_MS
                }
            };
            if switch {
                self.followed = Some(target.boss_object_id);
            }
        }

        // A selected tab can still disappear: the retention cap evicts the
        // oldest fight of a very long run. Fall back rather than blank.
        let gone = |id: &i32| !fights.iter().any(|f| f.boss_object_id == *id);
        if self.followed.as_ref().is_some_and(gone) {
            self.followed = None;
        }
        if self.pinned.as_ref().is_some_and(gone) {
            self.pinned = None;
        }
    }

    /// Horizontal, scrollable strip with one entry per boss.
    fn tab_strip(
        &mut self,
        ui: &mut egui::Ui,
        fights: &[Arc<LiveFightSnapshot>],
        selected: i32,
        hits: &mut HitRegions,
    ) {
        let top = ui.cursor().top();
        let strip = egui::ScrollArea::horizontal()
            .id_salt("live_combat_tabs")
            .auto_shrink([false, true]);
        super::styled_scroll(ui, strip, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let pinned = self.pinned.is_some();

                for fight in fights {
                    let boss = fight.boss_object_id;
                    let active = boss == selected;
                    let response = tab_button(ui, &fight.boss_name, active, !fight.ended);
                    if response.clicked() {
                        self.toggle_pin(boss);
                    }
                    overlay_tooltip_with(ui, &response, || {
                        pin_hint(
                            &format!(
                                "{}\n{} damage over {}",
                                fight.boss_name,
                                thousands(fight.total_damage()),
                                elapsed(fight.elapsed_ms),
                            ),
                            pinned && active,
                        )
                    });
                }
            });
        });

        // The whole strip, bar included: a locked overlay has to stay able to
        // switch boss and to scroll to a tab that is off the right edge.
        let laid_out = ui.min_rect();
        hits.add(egui::Rect::from_min_max(
            egui::pos2(laid_out.left(), top),
            egui::pos2(laid_out.right(), laid_out.bottom()),
        ));
    }

    /// Clicking the tab auto-follow is already on resumes following; clicking
    /// any other pins it, so the user can park on a boss while fighting another.
    fn toggle_pin(&mut self, boss: i32) {
        self.pinned = match self.pinned {
            Some(current) if current == boss => None,
            _ => Some(boss),
        };
    }
}

/// Title-bar toggle for the damage bars, drawn as three ranked bars.
fn bars_button(ui: &mut egui::Ui, on: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(16.0, super::TITLE_BAR_H), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let color = match (on, response.hovered()) {
            (true, _) => BARS_ICON_ON,
            (false, true) => Color32::from_gray(160),
            (false, false) => Color32::from_gray(110),
        };
        let left = rect.center().x - 5.0;
        let top = rect.center().y - 4.5;
        for (i, len) in [10.0, 7.0, 4.0].into_iter().enumerate() {
            let bar = egui::Rect::from_min_size(
                egui::pos2(left, top + i as f32 * 3.5),
                egui::vec2(len, 2.0),
            );
            ui.painter().rect_filled(bar, 0.5, color);
        }
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// Append the pin state to a tab's hover text.
fn pin_hint(body: &str, pinned: bool) -> String {
    if pinned {
        format!("{body}\n\nPinned. Click again to follow the boss you are attacking.")
    } else {
        format!("{body}\n\nClick to pin; the overlay otherwise follows the boss you are attacking.")
    }
}

/// One boss tab: a bordered pill with the full boss name, marked with a red dot
/// while its fight is still running.
fn tab_button(ui: &mut egui::Ui, label: &str, selected: bool, live: bool) -> egui::Response {
    const PAD_X: f32 = 7.0;
    const DOT_R: f32 = 2.5;
    let dot_w = if live { DOT_R * 2.0 + 5.0 } else { 0.0 };
    let font = egui::FontId::proportional(11.0);
    let text_w = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), Color32::WHITE)
        .size()
        .x;
    let size = egui::vec2(text_w + dot_w + PAD_X * 2.0, TAB_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }

    let hovered = response.hovered();
    let (fill, stroke, text) = if selected {
        (TAB_FILL_ACTIVE, TAB_STROKE_ACTIVE, TAB_TEXT_ACTIVE)
    } else if hovered {
        (TAB_FILL_HOVER, TAB_STROKE_IDLE, TAB_TEXT_HOVER)
    } else {
        (TAB_FILL_IDLE, TAB_STROKE_IDLE, TAB_TEXT_IDLE)
    };
    let painter = ui.painter();
    painter.rect(
        rect,
        4.0,
        fill,
        egui::Stroke::new(1.0_f32, stroke),
        egui::StrokeKind::Inside,
    );
    let mut x = rect.left() + PAD_X;
    if live {
        painter.circle_filled(egui::pos2(x + DOT_R, rect.center().y), DOT_R, TAB_LIVE);
        x += dot_w;
    }
    painter.text(
        egui::pos2(x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        text,
    );
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// One bar holding the boss icon, name and elapsed timer, filled by the boss's
/// remaining HP. A name too long for one line wraps and the bar grows to fit.
///
/// The name starts on the rows' icon column and the timer ends on their
/// percentage column, `inset` short of the right edge to clear the scrollbar.
fn header(
    ui: &mut egui::Ui,
    sprites: &mut SpriteRenderer,
    fight: &LiveFightSnapshot,
    columns: Columns,
    inset: f32,
) {
    const PAD: f32 = 4.0;
    let width = ui.available_width();
    let name_offset = columns.icon_left(ui).max(PAD * 2.0 + BOSS_ICON);
    let timer_offset = inset.max(PAD);
    let timer = ui.painter().layout_no_wrap(
        elapsed(fight.elapsed_ms),
        egui::FontId::monospace(12.0),
        Color32::from_gray(235),
    );
    let name_wrap = (width - name_offset - PAD - timer_offset - timer.size().x).max(1.0);
    let name = ui.painter().layout(
        fight.boss_name.clone(),
        egui::FontId::proportional(12.0),
        Color32::from_gray(245),
        name_wrap,
    );
    let height = HEADER_H.max(name.size().y + PAD);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }

    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, BOSS_HP_TRACK);
    // Only filled once the server has reported a pool; a boss with no known
    // stats would otherwise show a full-or-empty bar that means nothing.
    if fight.boss_max_hp > 0 && fight.boss_hp >= 0 {
        let fraction = (fight.boss_hp as f32 / fight.boss_max_hp as f32).clamp(0.0, 1.0);
        let mut filled = rect;
        filled.set_width(rect.width() * fraction);
        painter.rect_filled(filled, 2.0, BOSS_HP_FILL);
    }

    let icon = egui::Rect::from_min_size(
        egui::pos2(rect.left() + PAD, rect.center().y - BOSS_ICON * 0.5),
        egui::vec2(BOSS_ICON, BOSS_ICON),
    );
    sprites.draw_outlined_sprite_in_rect(ui, fight.boss_object_type, icon);

    let painter = ui.painter();
    let timer_pos = egui::pos2(
        rect.right() - timer_offset - timer.size().x,
        rect.center().y - timer.size().y * 0.5,
    );
    painter.galley(timer_pos, timer, Color32::from_gray(235));
    let name_pos = egui::pos2(
        rect.left() + name_offset,
        rect.center().y - name.size().y * 0.5,
    );
    painter.galley(name_pos, name, Color32::from_gray(245));
}

/// One participant row. With `top` set (the highest damage in the fight), a bar
/// is painted behind it from the icon to the percentage column, scaled so the
/// top dealer fills it.
fn row(
    ui: &mut egui::Ui,
    sprites: &mut SpriteRenderer,
    participant: &FightParticipant,
    rank: usize,
    total: i64,
    top: Option<i64>,
    columns: Columns,
    class_icons: bool,
    hits: &mut HitRegions,
) {
    let share = share(participant.damage, total);
    let color = row_color(participant);

    ui.horizontal(|ui| {
        ui.set_height(ICON);
        let full = ui.available_rect_before_wrap();
        // Reserved now so the bar sits beneath the text laid out after it.
        let bar_slot = ui.painter().add(egui::Shape::Noop);

        right_aligned_text(
            ui,
            columns.rank,
            format!("{rank}."),
            Color32::from_gray(130),
        );

        let (rect, _) = ui.allocate_exact_size(egui::vec2(ICON, ICON), egui::Sense::hover());
        let bar_left = rect.left();
        let (icon_id, tex1, tex2) = icon_for(participant, class_icons);
        if icon_id > 0 {
            sprites.draw_dyed_outlined_character_sprite(
                ui,
                icon_id,
                rect,
                SPRITE_DIRECTION,
                tex1,
                tex2,
            );
        }

        let name = ui.add(
            egui::Label::new(row_text(&participant.name, color))
                .sense(egui::Sense::click())
                .selectable(false),
        );
        // The name is the only thing in a row that answers a click, so it is the
        // only part of the body that may interrupt click-through. Clipped to the
        // scroll viewport so a row scrolled out of sight does not leave a live
        // patch behind.
        hits.add(name.rect.intersect(ui.clip_rect()));
        overlay_tooltip_with(ui, &name, || {
            format!("Click to copy\n{}", callout(participant, rank, share))
        });
        if name.clicked() {
            ui.ctx().copy_text(callout(participant, rank, share));
        }

        let pct_left = ui
            .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let pct = right_aligned_text(
                    ui,
                    columns.pct,
                    share_label(share),
                    Color32::from_gray(150),
                );
                ui.label(row_text(thousands(participant.damage), color));
                pct.left()
            })
            .inner;

        if let Some(top) = top {
            let span = (pct_left - BAR_GAP - bar_left).max(0.0);
            let width = span * bar_length(participant.damage, top);
            let bar = egui::Rect::from_min_size(
                egui::pos2(bar_left, full.top()),
                egui::vec2(width, ICON),
            );
            ui.painter().set(
                bar_slot,
                egui::Shape::rect_filled(bar, 2.0, SHARE_FILL.gamma_multiply(0.6)),
            );
        }
    });
}

/// A fixed-width cell with `text` right-aligned in it, in the row font.
fn right_aligned_text(ui: &mut egui::Ui, width: f32, text: String, color: Color32) -> egui::Rect {
    let galley = ui
        .painter()
        .layout_no_wrap(text, egui::FontId::monospace(12.0), color);
    let width = width.max(galley.size().x);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, ICON), egui::Sense::hover());
    let pos = egui::pos2(
        rect.right() - galley.size().x,
        rect.center().y - galley.size().y * 0.5,
    );
    ui.painter().galley(pos, galley, color);
    rect
}

/// A participant's share of the fight's total damage, in `0.0..=1.0`.
fn share(damage: i64, total: i64) -> f32 {
    (damage as f32 / total.max(1) as f32).clamp(0.0, 1.0)
}

/// Whole-percent label for a share, e.g. `15%`.
fn share_label(share: f32) -> String {
    format!("{:.0}%", share * 100.0)
}

/// Bar length relative to the top dealer, in `0.0..=1.0`.
fn bar_length(damage: i64, top: i64) -> f32 {
    if top <= 0 {
        return 0.0;
    }
    (damage as f32 / top as f32).clamp(0.0, 1.0)
}

/// Sprite id and dyes for a row: the bare class sprite when `class_icons` is on,
/// otherwise the player's skin and dyes.
fn icon_for(participant: &FightParticipant, class_icons: bool) -> (i32, u32, u32) {
    if class_icons || participant.skin_id <= 0 {
        let (tex1, tex2) = if class_icons {
            (0, 0)
        } else {
            (participant.tex1, participant.tex2)
        };
        return (participant.object_type, tex1, tex2);
    }
    (participant.skin_id, participant.tex1, participant.tex2)
}

/// Green for the local player, amber when their own figure is not exact.
///
/// Uncertainty is only ever signalled on the local row. `PartiallyObserved`
/// applies to the whole roster when the player joined a fight late, so tinting
/// every row on it turned the entire overlay amber.
fn row_color(participant: &FightParticipant) -> Color32 {
    if !participant.is_local {
        return NORMAL_COLOR;
    }
    if is_uncertain(participant.provenance) {
        UNCERTAIN_COLOR
    } else {
        LOCAL_COLOR
    }
}

/// Whether this figure is an estimate, a lower bound, or not yet resolvable.
fn is_uncertain(provenance: DamageProvenance) -> bool {
    matches!(
        provenance,
        DamageProvenance::PartiallyObserved
            | DamageProvenance::SelfPartial
            | DamageProvenance::SelfPending
            | DamageProvenance::SelfEstimated
            | DamageProvenance::Unresolved
    )
}

/// `m:ss`, or `h:mm:ss` for the rare fight that runs past an hour.
fn elapsed(ms: i64) -> String {
    let total = (ms / 1000).max(0);
    let (hours, minutes, seconds) = (total / 3600, (total % 3600) / 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// The one-line summary clicking a row's name puts on the clipboard, ready to
/// be pasted into chat: rank, player, damage and share of the fight.
fn callout(participant: &FightParticipant, rank: usize, share: f32) -> String {
    format!(
        "{rank}: {} {} {:.0}%",
        participant.name,
        thousands(participant.damage),
        share * 100.0
    )
}

#[cfg(test)]
mod tests {
    use super::{
        bar_length, callout, elapsed, icon_for, is_uncertain, row_color, Arc, LiveCombatOverlay,
        NORMAL_COLOR,
    };
    use realmhound_core::combat::{
        DamageProvenance, FightParticipant, LiveFightSnapshot, ParticipantEndStatus,
    };

    fn participant(object_id: i32, name: &str, damage: i64, is_local: bool) -> FightParticipant {
        FightParticipant {
            object_id,
            object_type: 0x0300,
            skin_id: 0,
            tex1: 0,
            tex2: 0,
            name: name.to_string(),
            equipment: [0; 4],
            equipment_enchants: Default::default(),
            damage,
            hits: 1,
            is_local,
            provenance: DamageProvenance::Observed,
            end_status: ParticipantEndStatus::Present,
            pet: None,
            damage_taken: None,
            damage_taken_provenance: realmhound_core::combat::DamageTakenProvenance::Observed,
            damage_taken_hits: None,
            damage_blocked: None,
            guarded_damage: None,
            guarded_hits: None,
            summon_damage: 0,
            summon_hits: 0,
            summons: Vec::new(),
            weapons: Vec::new(),
            packet_damage: 0,
            packet_hits: 0,
        }
    }

    fn fight(
        id: i32,
        started_at: i64,
        elapsed_ms: i64,
        who: Vec<FightParticipant>,
    ) -> Arc<LiveFightSnapshot> {
        Arc::new(LiveFightSnapshot {
            boss_object_id: id,
            boss_object_type: 0x10E4,
            boss_name: format!("Boss {id}"),
            dungeon: "Test Dungeon".to_string(),
            boss_hp: -1,
            boss_max_hp: 0,
            started_at,
            last_local_hit_at: 0,
            elapsed_ms,
            boss_seen_at: started_at,
            boss_gone: true,
            participants: who,
            ended: true,
        })
    }

    /// A fight the local player last hit at `hit_at`.
    fn hit(id: i32, hit_at: i64) -> Arc<LiveFightSnapshot> {
        let mut snapshot = (*fight(id, 0, 1_000, vec![participant(1, "Me", 100, true)])).clone();
        snapshot.last_local_hit_at = hit_at;
        Arc::new(snapshot)
    }

    #[test]
    fn elapsed_rolls_over_into_hours() {
        assert_eq!(elapsed(0), "0:00");
        assert_eq!(elapsed(-500), "0:00");
        assert_eq!(elapsed(9_000), "0:09");
        assert_eq!(elapsed(83_000), "1:23");
        assert_eq!(elapsed(3_723_000), "1:02:03");
    }

    #[test]
    fn bars_are_scaled_against_the_top_dealer() {
        assert_eq!(bar_length(56_165, 56_165), 1.0);
        assert!((bar_length(28_000, 56_000) - 0.5).abs() < 1e-6);
        assert_eq!(bar_length(10, 0), 0.0);
    }

    #[test]
    fn the_row_callout_carries_rank_name_damage_and_share() {
        let who = participant(1, "Nicky", 1_234_567, true);
        assert_eq!(callout(&who, 3, 0.4567), "3: Nicky 1,234,567 46%");
    }

    #[test]
    fn only_inexact_provenances_are_flagged() {
        assert!(!is_uncertain(DamageProvenance::Observed));
        assert!(!is_uncertain(DamageProvenance::SelfComputed));
        assert!(!is_uncertain(DamageProvenance::SelfReconciled));
        assert!(is_uncertain(DamageProvenance::SelfPending));
        assert!(is_uncertain(DamageProvenance::SelfEstimated));
        assert!(is_uncertain(DamageProvenance::PartiallyObserved));
        assert!(is_uncertain(DamageProvenance::Unresolved));
    }

    #[test]
    fn only_the_local_row_is_ever_tinted_amber() {
        // Joining a fight late marks every participant `PartiallyObserved`,
        // which used to turn the whole overlay yellow.
        let mut remote = participant(2, "Someone", 10, false);
        remote.provenance = DamageProvenance::PartiallyObserved;
        assert_eq!(row_color(&remote), NORMAL_COLOR);

        let mut local = participant(1, "Me", 10, true);
        local.provenance = DamageProvenance::PartiallyObserved;
        assert_ne!(row_color(&local), NORMAL_COLOR);
    }

    #[test]
    fn class_icons_drop_skins_and_dyes() {
        let mut who = participant(1, "Nicky", 10, true);
        who.skin_id = 0x1234;
        who.tex1 = 7;
        who.tex2 = 9;
        assert_eq!(icon_for(&who, false), (0x1234, 7, 9));
        assert_eq!(icon_for(&who, true), (0x0300, 0, 0));

        who.skin_id = 0;
        assert_eq!(icon_for(&who, false), (0x0300, 7, 9));
    }

    #[test]
    fn the_tab_follows_whichever_boss_the_player_is_hitting() {
        let mut overlay = LiveCombatOverlay::default();
        overlay.sync_instance(&[hit(10, 1_000), hit(11, 0)]);
        assert_eq!(overlay.followed, Some(10));

        // The player turns on the second boss.
        overlay.sync_instance(&[hit(10, 1_000), hit(11, 9_000)]);
        assert_eq!(overlay.followed, Some(11));
    }

    #[test]
    fn fights_the_player_never_touched_are_still_listed() {
        // Auto-follow is a view hint, never a filter: a boss the group killed
        // while the local player was elsewhere must keep its tab and its rows.
        let fights = vec![
            hit(10, 0),
            fight(11, 0, 1_000, vec![participant(2, "Other", 400, false)]),
        ];
        let mut overlay = LiveCombatOverlay::default();
        overlay.sync_instance(&fights);
        assert_eq!(overlay.followed, None, "nothing to follow yet");
    }

    #[test]
    fn alternating_between_two_bosses_does_not_swap_the_tab_every_batch() {
        // Both bosses are being hit continuously; without the margin the tab
        // would flip on whichever packet batch landed last.
        let mut overlay = LiveCombatOverlay::default();
        overlay.sync_instance(&[hit(10, 10_000), hit(11, 0)]);
        assert_eq!(overlay.followed, Some(10));

        overlay.sync_instance(&[hit(10, 10_000), hit(11, 10_400)]);
        assert_eq!(overlay.followed, Some(10), "swapped on a 400ms lead");

        overlay.sync_instance(&[hit(10, 10_000), hit(11, 12_000)]);
        assert_eq!(overlay.followed, Some(11), "never committed");
    }

    #[test]
    fn a_pinned_tab_outranks_auto_follow() {
        let mut overlay = LiveCombatOverlay::default();
        overlay.sync_instance(&[hit(10, 1_000), hit(11, 0)]);
        overlay.toggle_pin(10);

        overlay.sync_instance(&[hit(10, 1_000), hit(11, 50_000)]);
        assert_eq!(overlay.pinned, Some(10));
        assert_eq!(overlay.followed, Some(11), "auto-follow still tracks");

        // Clicking the pinned tab hands control back to auto-follow.
        overlay.toggle_pin(10);
        assert_eq!(overlay.pinned, None);
    }

    #[test]
    fn changing_zone_drops_the_pin_and_the_followed_boss() {
        let mut overlay = LiveCombatOverlay::default();
        overlay.sync_instance(&[hit(10, 1_000)]);
        overlay.toggle_pin(10);

        // A new instance brings an unrelated boss id.
        overlay.sync_instance(&[hit(99, 0)]);
        assert_eq!(overlay.pinned, None);
        assert_eq!(overlay.followed, None);
    }

    #[test]
    fn a_new_boss_in_the_same_zone_keeps_the_pin() {
        let mut overlay = LiveCombatOverlay::default();
        overlay.sync_instance(&[hit(10, 1_000)]);
        overlay.toggle_pin(10);

        overlay.sync_instance(&[hit(10, 1_000), hit(11, 0)]);
        assert_eq!(overlay.pinned, Some(10));
    }
}

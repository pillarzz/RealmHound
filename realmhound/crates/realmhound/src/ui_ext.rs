//! Hover-gated tooltip extension for [`egui::Response`].
//!
//! egui's `on_hover_text` / `on_hover_ui` always run `Tooltip::should_show_tooltip`,
//! whose post-scroll repaint guard fires BEFORE the `!hovered()` early-out. With
//! many tooltipped widgets that produces a repaint-request storm for a short
//! window after every scroll. These wrappers only enter egui's tooltip path when
//! the pointer is actually over the widget (or that widget's tooltip is already
//! open), so at most one widget hits the guard per frame instead of all of them.
//!
//! Behavior for the hovered widget is unchanged: egui's normal delay,
//! positioning, and occlusion logic still runs.

use eframe::egui::{Color32, Response, Ui, WidgetText};

/// Gold pill background for account-owned unlocks — the Missions tab's
/// `REGULAR` badge colour (`#c9b200`).
pub const PILL_GOLD: Color32 = Color32::from_rgb(0xc9, 0xb2, 0x00);

/// Green pill background for unlocks that also exist in the seasonal forge —
/// the Missions tab's `SEASONAL` badge colour (`#15dca6`).
pub const PILL_GREEN: Color32 = Color32::from_rgb(0x15, 0xdc, 0xa6);

/// Colored status pill matching the mission-tooltip badge style: a filled
/// rounded rect with CAPS white text centered vertically and horizontally
/// behind a 1px black outline (e.g. `OWNED` / `SEASONAL` / `CRUCIBLE`).
pub fn status_pill(ui: &mut Ui, text: &str, background: Color32) {
    let font = eframe::egui::FontId::proportional(11.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE);
    let size = galley.size() + eframe::egui::vec2(10.0, 4.0);
    let (rect, _) = ui.allocate_exact_size(size, eframe::egui::Sense::hover());
    ui.painter().rect_filled(rect, 3.0, background);

    let center = rect.center();
    let outline = Color32::from_black_alpha(220);
    for dx in [-1.0f32, 0.0, 1.0] {
        for dy in [-1.0f32, 0.0, 1.0] {
            if dx == 0.0 && dy == 0.0 {
                continue;
            }
            ui.painter().text(
                center + eframe::egui::vec2(dx, dy),
                eframe::egui::Align2::CENTER_CENTER,
                text,
                font.clone(),
                outline,
            );
        }
    }
    ui.painter().text(
        center,
        eframe::egui::Align2::CENTER_CENTER,
        text,
        font,
        Color32::WHITE,
    );
}

pub trait HoverTooltipExt {
    /// Hover-gated [`Response::on_hover_text`].
    fn hover_tip(self, text: impl Into<WidgetText>) -> Self;
    /// Hover-gated [`Response::on_hover_ui`].
    fn hover_tip_ui(self, add_contents: impl FnOnce(&mut Ui)) -> Self;
    /// Hover-gated [`Response::on_disabled_hover_text`].
    fn disabled_hover_tip(self, text: impl Into<WidgetText>) -> Self;
}

impl HoverTooltipExt for Response {
    fn hover_tip(self, text: impl Into<WidgetText>) -> Self {
        if enabled_tip(&self) {
            self.on_hover_text(text)
        } else {
            self
        }
    }

    fn hover_tip_ui(self, add_contents: impl FnOnce(&mut Ui)) -> Self {
        if enabled_tip(&self) {
            self.on_hover_ui(add_contents)
        } else {
            self
        }
    }

    fn disabled_hover_tip(self, text: impl Into<WidgetText>) -> Self {
        if disabled_tip(&self) {
            self.on_disabled_hover_text(text)
        } else {
            self
        }
    }
}

#[inline]
fn enabled_tip(r: &Response) -> bool {
    // contains_pointer: the widget under the cursor runs egui's normal tooltip
    // logic unchanged. is_tooltip_open: keep large/interactive tooltips alive
    // while the pointer is over the tooltip itself rather than the widget.
    r.contains_pointer() || r.is_tooltip_open()
}

#[inline]
fn disabled_tip(r: &Response) -> bool {
    // egui gates disabled-widget tooltips on Context::rect_contains_pointer,
    // which (unlike Response::contains_pointer) ignores occlusion -- match it.
    r.ctx.rect_contains_pointer(r.layer_id, r.rect) || r.is_tooltip_open()
}

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

use eframe::egui::{Response, Ui, WidgetText};

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

//! Shared keyboard scroll navigation (PageUp / PageDown / Home / End).
//!
//! Pages with a single primary scroll area can opt into keyboard scrolling by
//! reading a [`ScrollNav`] before building their `ScrollArea`, applying it to
//! the builder, and storing the resulting offset afterwards:
//!
//! ```ignore
//! let nav = ScrollNav::read(ui, "my_page_scroll");
//! let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
//! area = nav.apply(area);
//! let out = area.show(ui, |ui| { /* content */ });
//! nav.store(ui, &out);
//! ```
//!
//! The offset and maximum offset from the previous frame are stashed in egui
//! temp memory keyed by `key`, so this works even inside static render methods
//! that hold no per-panel state. Only wire ONE scroll area per page/key to
//! avoid double-processing.
//!
//! End jumps to the exact maximum offset computed from the previous frame's
//! content and viewport sizes. A sentinel like `f32::MAX` must NOT be used:
//! virtualized scroll areas (`ScrollArea::show_rows`) derive their visible row
//! range directly from the offset, and an out-of-range value panics on the
//! row-slice index.

use eframe::egui::{self, scroll_area::ScrollAreaOutput, Key, ScrollArea, Ui};

/// Fraction of the visible viewport a single PageUp/PageDown moves.
const PAGE_FRACTION: f32 = 0.9;

/// Per-key scroll state carried across frames.
#[derive(Clone, Copy, Default)]
struct NavState {
    /// Offset (in points) at the end of the previous frame.
    offset: f32,
    /// Largest valid offset last frame: `content_height - viewport_height`.
    max_offset: f32,
}

pub struct ScrollNav {
    id: egui::Id,
    target: Option<f32>,
}

impl ScrollNav {
    /// Read the pending keyboard navigation for this frame. `key` must be a
    /// stable, page-unique string. Returns a nav with no target unless one of
    /// the navigation keys was pressed this frame (and no text field is
    /// focused, so typing in search boxes is never hijacked).
    pub fn read(ui: &Ui, key: &str) -> Self {
        let id = egui::Id::new(("scroll_nav", key));
        let state = ui
            .ctx()
            .memory(|m| m.data.get_temp::<NavState>(id))
            .unwrap_or_default();

        let target = if ui.ctx().wants_keyboard_input() {
            None
        } else {
            let page = (ui.available_height() * PAGE_FRACTION).max(40.0);
            ui.input(|i| {
                if i.key_pressed(Key::Home) {
                    Some(0.0)
                } else if i.key_pressed(Key::End) {
                    // Exact bottom from last frame's measured content; clamped so
                    // an unmeasured first frame can't produce a bad offset.
                    Some(state.max_offset.max(0.0))
                } else if i.key_pressed(Key::PageDown) {
                    Some(state.offset + page)
                } else if i.key_pressed(Key::PageUp) {
                    Some((state.offset - page).max(0.0))
                } else {
                    None
                }
            })
        };

        Self { id, target }
    }

    /// Force the scroll offset for this frame if a key was pressed.
    pub fn apply(&self, area: ScrollArea) -> ScrollArea {
        match self.target {
            Some(t) => area.vertical_scroll_offset(t),
            None => area,
        }
    }

    /// Remember the resulting offset and the bottom-most valid offset so the
    /// next PageUp/PageDown/End is relative to the real (clamped) position.
    pub fn store<R>(&self, ui: &Ui, out: &ScrollAreaOutput<R>) {
        let viewport_h = out.inner_rect.height();
        let max_offset = (out.content_size.y - viewport_h).max(0.0);
        let state = NavState {
            offset: out.state.offset.y,
            max_offset,
        };
        ui.ctx().memory_mut(|m| m.data.insert_temp(self.id, state));
    }
}

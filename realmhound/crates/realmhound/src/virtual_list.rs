//! Reusable virtualized fixed-height row list.
//!
//! Thin wrapper over [`egui::ScrollArea::show_rows`] that builds and paints only
//! the rows intersecting the viewport, so long lists stay cheap per frame.

use eframe::egui;

/// Render only the visible rows of a fixed-height list.
///
/// `row_height` is the per-row pitch EXCLUDING egui item spacing: `show_rows`
/// adds `item_spacing.y` itself, so the body produced for each row must occupy
/// exactly `row_height` or scroll math and row positions drift. Fold any leading
/// gap into `row_height` and reproduce it inside `render_row`.
///
/// `prologue` runs once inside the viewport UI before the rows (e.g. to set a
/// clip rect). Each row is wrapped in `push_id(index)` so its widget IDs stay
/// stable regardless of scroll position, matching `show_rows`' one-ID-per-row
/// assumption.
pub fn show<P, R>(
    area: egui::ScrollArea,
    ui: &mut egui::Ui,
    row_height: f32,
    row_count: usize,
    prologue: P,
    mut render_row: R,
) -> egui::scroll_area::ScrollAreaOutput<()>
where
    P: FnOnce(&mut egui::Ui),
    R: FnMut(&mut egui::Ui, usize),
{
    area.show_rows(ui, row_height, row_count, |ui, range| {
        prologue(ui);
        for i in range {
            ui.push_id(i, |ui| render_row(ui, i));
        }
    })
}

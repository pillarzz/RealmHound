//! UI color constants for consistent theming across the application.

use egui::Color32;

/// Color for seasonal content (green-teal).
/// Hex: #15dca6
pub const SEASONAL_COLOR: Color32 = Color32::from_rgb(0x15, 0xdc, 0xa6);

/// Color for regular/permanent content (gold).
/// Hex: #c9b200
pub const REGULAR_COLOR: Color32 = Color32::from_rgb(0xc9, 0xb2, 0x00);

/// Translucent overlay used to dim non-matching item slots while a search is
/// active, so highlighted matches stand out.
pub const DIM_OVERLAY: Color32 = Color32::from_black_alpha(150);

/// Opaque blend between two colors at fraction `t` (0 = `a`, 1 = `b`).
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Chrome shade derived from the active theme, keeping the same relative
/// brightness as the old fixed grays but picking up the palette's hue. `t` is
/// the blend fraction from the theme background toward its foreground; the
/// constants below reproduce the original dark-mode look on the default theme.
pub fn themed_shade(visuals: &egui::Visuals, t: f32) -> Color32 {
    let bg = visuals.panel_fill;
    let fg = visuals
        .override_text_color
        .unwrap_or(visuals.widgets.noninteractive.fg_stroke.color);
    mix(bg, fg, t)
}

/// Item slot background (was `rgb(40, 40, 40)`).
pub fn slot_fill(visuals: &egui::Visuals) -> Color32 {
    themed_shade(visuals, 0.09)
}

/// Item slot border (was `rgb(60, 60, 60)`).
pub fn slot_stroke(visuals: &egui::Visuals) -> Color32 {
    themed_shade(visuals, 0.20)
}

/// Unavailable/locked slot background (was `rgb(25, 25, 28)`).
pub fn unavailable_slot_fill(visuals: &egui::Visuals) -> Color32 {
    themed_shade(visuals, 0.025)
}

/// Unavailable/locked slot border (was `rgb(45, 45, 50)`).
pub fn unavailable_slot_stroke(visuals: &egui::Visuals) -> Color32 {
    themed_shade(visuals, 0.09)
}

/// Card/panel frame background (was `rgb(30, 30, 35)`).
pub fn card_fill(visuals: &egui::Visuals) -> Color32 {
    themed_shade(visuals, 0.05)
}

/// Card/panel frame border (was `rgb(60, 60, 65)`).
pub fn card_stroke(visuals: &egui::Visuals) -> Color32 {
    themed_shade(visuals, 0.16)
}

/// Sprite fallback tile background (was `rgb(50, 50, 55)`).
pub fn sprite_fallback_fill(visuals: &egui::Visuals) -> Color32 {
    themed_shade(visuals, 0.12)
}

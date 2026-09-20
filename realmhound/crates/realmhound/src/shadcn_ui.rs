//! Thin wrapper around the `egui-shadcn` crate.
//!
//! `egui-shadcn` has an explicitly unstable API. Centralizing every call here
//! keeps the breakage surface to one file when upgrading the crate, and gives
//! the rest of the app a small, ergonomic vocabulary of themed widgets.

use crate::ui_ext::HoverTooltipExt;
use std::cell::RefCell;
use std::rc::Rc;

use egui::{Response, Ui, WidgetText};
use egui_shadcn::button::{Button, ButtonJustify, ButtonSize, ButtonVariant};
use egui_shadcn::calendar::{calendar_with_props, CalendarMode, CalendarProps};
use egui_shadcn::card::{CardProps, CardVariant};
use egui_shadcn::dialog::{DialogAlign, DialogProps};
use egui_shadcn::icons::icon_calendar;
use egui_shadcn::popover::{popover, PopoverAlign, PopoverProps, PopoverSide};
use egui_shadcn::select::{SelectItem, SelectPortalContainer, SelectProps, SelectSide, SelectSize};
use egui_shadcn::separator::SeparatorProps;
use egui_shadcn::tokens::{
    ColorPalette, ControlSize, ControlVariant, ShadcnBaseColor, ToggleVariant,
};
use egui_shadcn::Theme;

/// Default brand color (indigo) used by the Default presets and as the initial
/// Custom primary.
pub const DEFAULT_PRIMARY: egui::Color32 = egui::Color32::from_rgb(124, 131, 255);

/// A neutral (slate) shadcn palette for the given mode, before any brand color
/// is applied. Surfaces for both presets and custom themes start here.
fn neutral_palette(mode: Mode) -> ColorPalette {
    match mode {
        Mode::Dark => ColorPalette::shadcn_dark(ShadcnBaseColor::Slate),
        Mode::Light => ColorPalette::shadcn_light(ShadcnBaseColor::Slate),
    }
}

/// Apply a single brand color to the primary/ring tokens, deriving a readable
/// foreground. This is the whole "Custom" knob: one color drives the rest.
fn apply_primary(mut palette: ColorPalette, primary: egui::Color32) -> ColorPalette {
    palette.primary = primary;
    palette.primary_foreground = readable_fg(primary);
    palette.ring = primary;
    palette
}

/// `#rrggbb` for persistence.
pub fn color_to_hex(c: egui::Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

/// Parse `#rrggbb` (or `rrggbb`); falls back to [`DEFAULT_PRIMARY`].
pub fn color_from_hex(s: &str) -> egui::Color32 {
    let h = s.trim().trim_start_matches('#');
    if h.len() == 6 {
        if let Ok(v) = u32::from_str_radix(h, 16) {
            return rgb(v);
        }
    }
    DEFAULT_PRIMARY
}

/// Light or dark surface mode. Drives both the shadcn palette and the
/// underlying egui [`Visuals`](egui::Visuals) chrome.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Light,
    Dark,
}

impl Mode {
    pub fn key(self) -> &'static str {
        match self {
            Mode::Light => "light",
            Mode::Dark => "dark",
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key {
            "light" => Mode::Light,
            _ => Mode::Dark,
        }
    }

    pub fn is_dark(self) -> bool {
        matches!(self, Mode::Dark)
    }
}

#[inline]
const fn rgb(hex: u32) -> egui::Color32 {
    egui::Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// sRGB luminance in `[0, 1]`. Used to pick readable foregrounds.
fn luminance(c: egui::Color32) -> f32 {
    (c.r() as f32 * 0.299 + c.g() as f32 * 0.587 + c.b() as f32 * 0.114) / 255.0
}

/// Opaque linear blend between two colors (alpha ignored, result opaque).
fn mix_opaque(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    egui::Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Text color that reads on top of `bg`.
fn readable_fg(bg: egui::Color32) -> egui::Color32 {
    if luminance(bg) > 0.55 {
        rgb(0x1a1a1a)
    } else {
        rgb(0xf5f6ff)
    }
}

/// Signature colors of a curated theme. Surfaces/text are applied on top of a
/// shadcn base palette of the matching [`Mode`]; the rest (charts) is inherited.
struct Curated {
    mode: Mode,
    background: egui::Color32,
    foreground: egui::Color32,
    card: egui::Color32,
    surface: egui::Color32,
    border: egui::Color32,
    primary: egui::Color32,
    accent: egui::Color32,
    destructive: egui::Color32,
    muted_foreground: egui::Color32,
}

impl Curated {
    fn to_palette(&self) -> ColorPalette {
        let base = if self.mode.is_dark() {
            ColorPalette::shadcn_dark(ShadcnBaseColor::Neutral)
        } else {
            ColorPalette::shadcn_light(ShadcnBaseColor::Neutral)
        };
        ColorPalette {
            background: self.background,
            foreground: self.foreground,
            card: self.card,
            card_foreground: self.foreground,
            popover: self.card,
            popover_foreground: self.foreground,
            border: self.border,
            input: self.surface,
            ring: self.primary,
            primary: self.primary,
            primary_foreground: readable_fg(self.primary),
            secondary: self.surface,
            secondary_foreground: self.foreground,
            accent: self.accent,
            accent_foreground: readable_fg(self.accent),
            muted: self.surface,
            muted_foreground: self.muted_foreground,
            destructive: self.destructive,
            destructive_foreground: readable_fg(self.destructive),
            sidebar: self.card,
            sidebar_foreground: self.foreground,
            sidebar_primary: self.primary,
            sidebar_primary_foreground: readable_fg(self.primary),
            sidebar_accent: self.surface,
            sidebar_accent_foreground: self.foreground,
            sidebar_border: self.border,
            sidebar_ring: self.primary,
            ..base
        }
    }
}

/// A complete, named theme shown in the single "Theme" selector. Each preset
/// bakes its surfaces, accent and [`Mode`]; picking one is a one-click choice.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ThemePreset {
    DefaultDark,
    DefaultLight,
    Nord,
    TokyoNight,
    CatppuccinMocha,
    CatppuccinLatte,
    RosePine,
    RosePineDawn,
    Gruvbox,
}

impl ThemePreset {
    pub const ALL: [ThemePreset; 9] = [
        ThemePreset::DefaultDark,
        ThemePreset::DefaultLight,
        ThemePreset::Nord,
        ThemePreset::TokyoNight,
        ThemePreset::CatppuccinMocha,
        ThemePreset::CatppuccinLatte,
        ThemePreset::RosePine,
        ThemePreset::RosePineDawn,
        ThemePreset::Gruvbox,
    ];

    /// Dark-mode-only presets exposed in the theme selector while light mode
    /// support is incomplete (hardcoded dark backgrounds / white text throughout
    /// the UI). Re-enable ALL + Custom when light mode is fully supported.
    pub const DARK_ONLY: [ThemePreset; 6] = [
        ThemePreset::DefaultDark,
        ThemePreset::Nord,
        ThemePreset::TokyoNight,
        ThemePreset::CatppuccinMocha,
        ThemePreset::RosePine,
        ThemePreset::Gruvbox,
    ];

    pub fn key(self) -> &'static str {
        match self {
            ThemePreset::DefaultDark => "default-dark",
            ThemePreset::DefaultLight => "default-light",
            ThemePreset::Nord => "nord",
            ThemePreset::TokyoNight => "tokyo-night",
            ThemePreset::CatppuccinMocha => "catppuccin-mocha",
            ThemePreset::CatppuccinLatte => "catppuccin-latte",
            ThemePreset::RosePine => "rose-pine",
            ThemePreset::RosePineDawn => "rose-pine-dawn",
            ThemePreset::Gruvbox => "gruvbox",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ThemePreset::DefaultDark => "Default Dark",
            ThemePreset::DefaultLight => "Default Light",
            ThemePreset::Nord => "Nord",
            ThemePreset::TokyoNight => "Tokyo Night",
            ThemePreset::CatppuccinMocha => "Catppuccin Mocha",
            ThemePreset::CatppuccinLatte => "Catppuccin Latte",
            ThemePreset::RosePine => "Rose Pine",
            ThemePreset::RosePineDawn => "Rose Pine Dawn",
            ThemePreset::Gruvbox => "Gruvbox",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.key() == key)
    }

    pub fn mode(self) -> Mode {
        match self {
            ThemePreset::DefaultLight
            | ThemePreset::CatppuccinLatte
            | ThemePreset::RosePineDawn => Mode::Light,
            _ => Mode::Dark,
        }
    }

    /// Resolve to a raw shadcn palette (before the shared contrast pass).
    fn palette(self) -> ColorPalette {
        match self {
            ThemePreset::DefaultDark => apply_primary(neutral_palette(Mode::Dark), DEFAULT_PRIMARY),
            ThemePreset::DefaultLight => {
                apply_primary(neutral_palette(Mode::Light), DEFAULT_PRIMARY)
            }
            ThemePreset::Nord => Curated {
                mode: Mode::Dark,
                background: rgb(0x2e3440),
                foreground: rgb(0xeceff4),
                card: rgb(0x3b4252),
                surface: rgb(0x434c5e),
                border: rgb(0x4c566a),
                primary: rgb(0x88c0d0),
                accent: rgb(0x5e81ac),
                destructive: rgb(0xbf616a),
                muted_foreground: rgb(0xa9b1c2),
            }
            .to_palette(),
            ThemePreset::TokyoNight => Curated {
                mode: Mode::Dark,
                background: rgb(0x1a1b26),
                foreground: rgb(0xc0caf5),
                card: rgb(0x16161e),
                surface: rgb(0x24283b),
                border: rgb(0x292e42),
                primary: rgb(0x7aa2f7),
                accent: rgb(0xbb9af7),
                destructive: rgb(0xf7768e),
                muted_foreground: rgb(0x787c99),
            }
            .to_palette(),
            ThemePreset::CatppuccinMocha => Curated {
                mode: Mode::Dark,
                background: rgb(0x1e1e2e),
                foreground: rgb(0xcdd6f4),
                card: rgb(0x181825),
                surface: rgb(0x313244),
                border: rgb(0x45475a),
                primary: rgb(0x89b4fa),
                accent: rgb(0xcba6f7),
                destructive: rgb(0xf38ba8),
                muted_foreground: rgb(0xa6adc8),
            }
            .to_palette(),
            ThemePreset::CatppuccinLatte => Curated {
                mode: Mode::Light,
                background: rgb(0xeff1f5),
                foreground: rgb(0x4c4f69),
                card: rgb(0xe6e9ef),
                surface: rgb(0xccd0da),
                border: rgb(0xbcc0cc),
                primary: rgb(0x1e66f5),
                accent: rgb(0x8839ef),
                destructive: rgb(0xd20f39),
                muted_foreground: rgb(0x6c6f85),
            }
            .to_palette(),
            ThemePreset::RosePine => Curated {
                mode: Mode::Dark,
                background: rgb(0x191724),
                foreground: rgb(0xe0def4),
                card: rgb(0x1f1d2e),
                surface: rgb(0x26233a),
                border: rgb(0x403d52),
                primary: rgb(0xebbcba),
                accent: rgb(0xc4a7e7),
                destructive: rgb(0xeb6f92),
                muted_foreground: rgb(0x908caa),
            }
            .to_palette(),
            ThemePreset::RosePineDawn => Curated {
                mode: Mode::Light,
                background: rgb(0xfaf4ed),
                foreground: rgb(0x575279),
                card: rgb(0xfffaf3),
                surface: rgb(0xf2e9e1),
                border: rgb(0xdfdad9),
                primary: rgb(0xd7827e),
                accent: rgb(0x907aa9),
                destructive: rgb(0xb4637a),
                muted_foreground: rgb(0x797593),
            }
            .to_palette(),
            ThemePreset::Gruvbox => Curated {
                mode: Mode::Dark,
                background: rgb(0x282828),
                foreground: rgb(0xebdbb2),
                card: rgb(0x1d2021),
                surface: rgb(0x3c3836),
                border: rgb(0x504945),
                primary: rgb(0xfabd2f),
                accent: rgb(0xfe8019),
                destructive: rgb(0xfb4934),
                muted_foreground: rgb(0xa89984),
            }
            .to_palette(),
        }
    }
}

/// What the user has selected in the appearance settings: a named
/// [`ThemePreset`], or a custom single brand color + mode.
#[derive(Clone, Copy, PartialEq)]
pub enum ThemeSelection {
    Preset(ThemePreset),
    Custom { primary: egui::Color32, mode: Mode },
}

impl ThemeSelection {
    /// Selector key: a preset key, or `"custom"`.
    pub fn key(&self) -> &'static str {
        match self {
            ThemeSelection::Preset(p) => p.key(),
            ThemeSelection::Custom { .. } => "custom",
        }
    }

    pub fn mode(&self) -> Mode {
        match self {
            ThemeSelection::Preset(p) => p.mode(),
            ThemeSelection::Custom { mode, .. } => *mode,
        }
    }

    fn palette(&self) -> ColorPalette {
        match self {
            ThemeSelection::Preset(p) => p.palette(),
            ThemeSelection::Custom { primary, mode } => {
                apply_primary(neutral_palette(*mode), *primary)
            }
        }
    }
}

/// Owns the shadcn theme used by every wrapper widget.
///
/// Built once from the chosen [`ThemeSelection`] so per-frame rendering doesn't
/// recompute design tokens. `Clone` is cheap (it copies the resolved palette)
/// and avoids re-running `Theme::new`, which logs on every call.
#[derive(Clone)]
pub struct Shadcn {
    theme: Theme,
    selection: ThemeSelection,
    /// When true, panel buttons/toggles use the compact (smaller) variant.
    compact: bool,
}

impl Default for Shadcn {
    fn default() -> Self {
        Self::from_selection(ThemeSelection::Preset(ThemePreset::DefaultDark))
    }
}

impl Shadcn {
    /// Build the themed widget set from a resolved [`ThemeSelection`].
    pub fn from_selection(selection: ThemeSelection) -> Self {
        let mut palette = selection.palette();

        // Off-state outlines (notably the switch track, whose OFF border is
        // `mix(border, input, 0.4)` painted over an `input` fill) are nearly
        // invisible with the stock low-alpha tokens. Re-derive border/input as
        // opaque blends of background->foreground so every contour reads in both
        // light and dark; border is pushed harder than the input fill so the two
        // stay distinct.
        let bg = palette.background;
        let fg = palette.foreground;
        palette.border = mix_opaque(bg, fg, 0.40);
        palette.input = mix_opaque(bg, fg, 0.16);

        Self {
            theme: Theme::new(palette),
            selection,
            compact: true,
        }
    }

    /// Set whether compact (smaller) controls are used.
    pub fn set_compact(&mut self, compact: bool) {
        self.compact = compact;
    }

    /// Whether compact mode is active.
    pub fn is_compact(&self) -> bool {
        self.compact
    }

    pub fn selection(&self) -> ThemeSelection {
        self.selection
    }

    pub fn mode(&self) -> Mode {
        self.selection.mode()
    }

    /// Resolved design tokens, used to derive matching egui [`Visuals`].
    pub fn colors(&self) -> &ColorPalette {
        &self.theme.palette
    }

    /// Fill for the primary top-bar band (page tabs) plus the widget/task bars
    /// nested in the same band, noticeably darker than the background so these
    /// always-present, tab-independent elements read as a distinct fixed band.
    pub fn top_bar_fill(&self) -> egui::Color32 {
        self.darken_background(11)
    }

    /// Fill for a panel's secondary (sub-tab / filters) header, a shade between
    /// the darkest top bar and the plain background.
    pub fn secondary_header_fill(&self) -> egui::Color32 {
        self.darken_background(4)
    }

    /// Border/outline color from the active theme palette.
    pub fn border(&self) -> egui::Color32 {
        self.theme.palette.border
    }

    fn darken_background(&self, amount: u8) -> egui::Color32 {
        let bg = self.theme.palette.background;
        egui::Color32::from_rgba_premultiplied(
            bg.r().saturating_sub(amount),
            bg.g().saturating_sub(amount),
            bg.b().saturating_sub(amount),
            bg.a(),
        )
    }

    /// Wrap a panel's secondary header (sub-tabs / filter rows) in a full-bleed
    /// band filled with `fill`, mirroring the primary top-bar band: the fill
    /// bleeds 8px past the panel padding on the left/right so it reaches the
    /// window edges, closes the gap up to the primary separator, and a
    /// matching-width divider is painted on its bottom edge.
    pub fn header_band<R>(
        &self,
        ui: &mut Ui,
        fill: egui::Color32,
        add_contents: impl FnOnce(&mut Ui) -> R,
    ) -> R {
        self.header_band_impl(ui, fill, true, true, add_contents)
    }

    /// A stacked header band that keeps a visible top divider separating it from
    /// the band above. Unlike [`Shadcn::header_band`] it shares the same fill as
    /// the band above (no darker top-edge cover), so it reads as a seam between
    /// two secondary bands rather than a seam below the primary top bar.
    pub fn header_band_stacked_divided<R>(
        &self,
        ui: &mut Ui,
        fill: egui::Color32,
        add_contents: impl FnOnce(&mut Ui) -> R,
    ) -> R {
        self.header_band_impl(ui, fill, false, true, add_contents)
    }

    fn header_band_impl<R>(
        &self,
        ui: &mut Ui,
        fill: egui::Color32,
        top_cover: bool,
        top_divider: bool,
        add_contents: impl FnOnce(&mut Ui) -> R,
    ) -> R {
        let gap = ui.spacing().item_spacing.y.round() as i8;
        let band = egui::Frame::NONE
            .fill(fill)
            .outer_margin(egui::Margin {
                left: -8,
                right: -8,
                top: -gap,
                bottom: 0,
            })
            .inner_margin(egui::Margin {
                left: 8,
                right: 8,
                top: 4,
                bottom: 4,
            })
            .show(ui, |ui| {
                // Force the band to span the full panel width so its fill and
                // dividers reach the window edges even when the inner content
                // (a plain `ui.horizontal`) is narrower than the panel.
                ui.set_min_width(ui.available_width());
                add_contents(ui)
            });
        let rect = band.response.rect;
        let stroke = egui::Stroke::new(1.0_f32, self.theme.palette.border);
        let x = egui::Rangef::new(rect.left() - 8.0, rect.right() + 8.0);
        // The negative top outer margin pulls the band's fill up by `gap`, so the
        // fill's true top edge sits `gap` px above `rect.top()`. Anchor the top
        // divider (and cover) there so content, which is centered within the fill,
        // ends up with equal margins to the top and bottom dividers.
        let fill_top = rect.top() - gap as f32;
        if top_cover {
            // Sub-pixel rounding of the frame's negative top margin can leave a
            // thin strip of plain background between the darker primary top bar
            // above and this band's fill. Cover it with the primary top-bar tone
            // so the bar above reads as fully filled right down to the divider.
            let cover = egui::Rect::from_min_max(
                egui::pos2(rect.left() - 8.0, fill_top - 3.0),
                egui::pos2(rect.right() + 8.0, fill_top),
            );
            ui.painter()
                .with_clip_rect(ui.ctx().content_rect())
                .rect_filled(cover, 0.0, self.top_bar_fill());
        }
        if top_cover || top_divider {
            ui.painter().hline(x, fill_top, stroke);
        }
        ui.painter().hline(x, rect.bottom(), stroke);
        band.inner
    }

    /// Height of a secondary header-band content row. Every band row across the
    /// panels uses this so adjacent bands share the exact same height and stay
    /// aligned regardless of their content's natural height.
    pub const BAND_ROW_HEIGHT: f32 = 28.0;

    /// Lay out a header-band content row at a fixed height with content
    /// vertically centered between the band's dividers. Replaces the
    /// `ui.horizontal(|ui| { ui.set_min_height(28.0); .. })` pattern, which
    /// top-pins content (extra space is added *below*, not centered) and lets
    /// each band size to its own content, causing adjacent bands to misalign.
    pub fn band_row<R>(&self, ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), Self::BAND_ROW_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            add_contents,
        )
        .inner
    }

    /// A header-band content row that wraps onto additional rows when its
    /// content exceeds the available width, growing the band's height instead of
    /// clipping. Used by filter bands whose toggles/pills/buttons must fold into
    /// a second row at narrow window widths. A single (unwrapped) row keeps the
    /// same [`BAND_ROW_HEIGHT`] as [`Shadcn::band_row`] so adjacent bands align.
    /// Use [`Shadcn::band_divider`] instead of `ui.separator()` inside this row:
    /// a plain separator would stretch to the full remaining panel height.
    ///
    /// Every item added inside must be a single measurable widget: a wrapping
    /// layout can only fold items it can measure up front, so nested
    /// `ui.horizontal`/`Frame` blocks (e.g. hand-built pills) overflow the right
    /// edge instead of wrapping -- allocate such content as one widget instead.
    ///
    /// [`BAND_ROW_HEIGHT`]: Self::BAND_ROW_HEIGHT
    pub fn band_row_wrapped<R>(&self, ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
        ui.horizontal_wrapped(|ui| {
            ui.set_min_height(Self::BAND_ROW_HEIGHT);
            ui.spacing_mut().item_spacing.y = 4.0;
            add_contents(ui)
        })
        .inner
    }

    /// A fixed-height vertical divider for use inside a [`Shadcn::band_row_wrapped`].
    /// Unlike `ui.separator()`, whose vertical variant fills the ui's available
    /// height (the whole panel inside a wrapping band), this allocates a fixed
    /// box so it reads as a short in-row separator and wraps like any other item.
    pub fn band_divider(&self, ui: &mut Ui) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(7.0, 18.0), egui::Sense::hover());
        ui.painter().vline(
            rect.center().x,
            egui::Rangef::new(rect.top(), rect.bottom()),
            egui::Stroke::new(1.0_f32, self.theme.palette.border),
        );
    }

    /// A themed separator that bleeds the full window width, for use *inside* a
    /// [`Shadcn::header_band`] where the frame clip would otherwise crop a
    /// normal separator to the content area. Allocates separator-sized vertical
    /// space and paints the divider past the clip via the screen rect.
    pub fn full_width_separator(&self, ui: &mut Ui) {
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 5.0), egui::Sense::hover());
        ui.painter().with_clip_rect(ui.ctx().content_rect()).hline(
            egui::Rangef::new(rect.left() - 16.0, rect.right() + 16.0),
            rect.center().y,
            egui::Stroke::new(1.0_f32, self.theme.palette.border),
        );
    }

    /// A full-width, left-aligned navigation row for a vertical settings
    /// sidebar. Paints a highlighted background (with border) when `selected`,
    /// a subtle fill on hover, and left-aligns the label. Returns the click
    /// response.
    pub fn nav_item(&self, ui: &mut Ui, label: &str, selected: bool) -> Response {
        let palette = &self.theme.palette;
        let height = 30.0;
        let width = ui.available_width();
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());

        if ui.is_rect_visible(rect) {
            if selected {
                ui.painter().rect(
                    rect,
                    6.0,
                    palette.secondary,
                    egui::Stroke::new(1.0_f32, palette.border),
                    egui::StrokeKind::Inside,
                );
            } else if response.hovered() {
                ui.painter().rect_filled(
                    rect,
                    6.0,
                    egui_shadcn::tokens::mix(palette.background, palette.secondary, 0.5),
                );
            }

            let text_color = if selected {
                palette.foreground
            } else {
                palette.secondary_foreground
            };
            let galley = egui::WidgetText::from(label).into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                width - 20.0,
                egui::TextStyle::Button,
            );
            let text_pos = egui::pos2(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0);
            ui.painter()
                .with_clip_rect(rect)
                .galley(text_pos, galley, text_color);
        }

        response
    }

    /// A nested navigation sub-item (Discord-style), indented under a parent
    /// [`Self::nav_item`]. Selected items show a left accent bar and full-strength
    /// text; unselected items are muted and highlight subtly on hover.
    pub fn nav_subitem(&self, ui: &mut Ui, label: &str, selected: bool) -> Response {
        let palette = &self.theme.palette;
        let height = 26.0;
        // Align the sub-item text under the parent nav_item's label (which sits
        // at a 10px left pad after its "🔊 " icon), so sub-tabs read as tree
        // branches directly below the word "Sound".
        let parent_pad = 10.0;
        let icon_w = egui::WidgetText::from("🔊 ")
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Button,
            )
            .size()
            .x;
        let indent = parent_pad + icon_w;
        let width = ui.available_width();
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());

        if ui.is_rect_visible(rect) {
            if selected {
                ui.painter().rect_filled(rect, 6.0, palette.secondary);
                // Left accent bar marking the active sub-item.
                let bar = egui::Rect::from_min_size(
                    egui::pos2(rect.left() + 2.0, rect.center().y - 8.0),
                    egui::vec2(3.0, 16.0),
                );
                ui.painter().rect_filled(bar, 2.0, palette.accent);
            } else if response.hovered() {
                ui.painter().rect_filled(
                    rect,
                    6.0,
                    egui_shadcn::tokens::mix(palette.background, palette.secondary, 0.5),
                );
            }

            let text_color = if selected {
                palette.foreground
            } else if response.hovered() {
                palette.secondary_foreground
            } else {
                egui_shadcn::tokens::mix(palette.background, palette.secondary_foreground, 0.7)
            };
            let galley = egui::WidgetText::from(label).into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                width - indent - 8.0,
                egui::TextStyle::Button,
            );
            let text_pos = egui::pos2(
                rect.left() + indent,
                rect.center().y - galley.size().y / 2.0,
            );
            ui.painter()
                .with_clip_rect(rect)
                .galley(text_pos, galley, text_color);
        }

        response
    }

    /// A themed on/off toggle. Returns the underlying response (use `.changed()`).
    pub fn switch(&self, ui: &mut Ui, on: &mut bool, label: impl Into<WidgetText>) -> Response {
        // The shadcn switch renders its label after the pill in the current
        // layout; wrap it in a row so the label sits to the right, matching the
        // checkbox layout, instead of dropping below the pill.
        ui.horizontal(|ui| {
            let enabled = ui.is_enabled();
            egui_shadcn::switch::switch(
                ui,
                &self.theme,
                on,
                label,
                ControlVariant::Primary,
                ControlSize::Md,
                enabled,
            )
        })
        .inner
    }

    /// A themed primary button.
    pub fn button(&self, ui: &mut Ui, label: impl Into<WidgetText>) -> Response {
        egui_shadcn::button::button(
            ui,
            &self.theme,
            label,
            ControlVariant::Secondary,
            ControlSize::Md,
            true,
        )
    }

    /// A smaller themed button for panel toolbars and inline controls.
    pub fn button_sm(&self, ui: &mut Ui, label: impl Into<WidgetText>) -> Response {
        egui_shadcn::button::button(
            ui,
            &self.theme,
            label,
            ControlVariant::Secondary,
            ControlSize::Sm,
            true,
        )
    }

    /// A compact themed button with minimal internal padding. Renders via
    /// manual painting into an `allocate_exact_size` rect (like the vendored
    /// shadcn buttons do) so the box height is always exactly 24px --
    /// regardless of glyph metrics -- matching the compact input/select
    /// controls (`InputSize::Size1` / `SelectSize::Size1`). Using a native
    /// `egui::Button` here previously let tall glyphs (e.g. emoji icons) grow
    /// the button past its target height, misaligning toolbar rows.
    pub fn button_compact(&self, ui: &mut Ui, label: impl Into<WidgetText>) -> Response {
        let palette = &self.theme.palette;
        let height = 24.0;
        let padding = egui::vec2(6.0, 2.0);

        let galley = label.into().into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Button,
        );
        let width = galley.size().x + padding.x * 2.0;

        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());

        if ui.is_rect_visible(rect) {
            let bg = if response.hovered() {
                egui_shadcn::tokens::mix(palette.secondary, palette.accent, 0.15)
            } else {
                palette.secondary
            };
            ui.painter().rect(
                rect,
                4.0,
                bg,
                egui::Stroke::new(1.0_f32, palette.border),
                egui::StrokeKind::Inside,
            );
            let text_pos = rect.center() - galley.size() / 2.0;
            // Clip to the button's rect: some glyphs (e.g. emoji icons) report a
            // taller natural galley size than the fixed 24px box, which would
            // otherwise bleed above/below it and break row alignment even
            // though the box itself is the correct height.
            ui.painter().with_clip_rect(rect).galley(
                text_pos,
                galley,
                palette.secondary_foreground,
            );
        }

        response
    }

    /// A `btn`-styled button with space reserved on the left for a caller-drawn
    /// icon (e.g. a sprite). Returns the response and the rect the icon should
    /// be painted into. Mirrors `btn`'s compact/standard styling so icon
    /// buttons sit flush with the plain custom clipboard buttons.
    pub fn icon_button(&self, ui: &mut Ui, label: impl Into<WidgetText>) -> (Response, egui::Rect) {
        let palette = &self.theme.palette;
        let (height, padding_x, radius) = if self.compact {
            (24.0_f32, 6.0_f32, 4.0_f32)
        } else {
            (32.0_f32, 12.0_f32, 8.0_f32)
        };
        let icon_size = height - 8.0;
        let gap = 4.0;

        let galley = label.into().into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Button,
        );
        let width = padding_x * 2.0 + icon_size + gap + galley.size().x;
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());

        if ui.is_rect_visible(rect) {
            let (bg, stroke) = if self.compact {
                let bg = if response.hovered() {
                    egui_shadcn::tokens::mix(palette.secondary, palette.accent, 0.15)
                } else {
                    palette.secondary
                };
                (bg, egui::Stroke::new(1.0_f32, palette.border))
            } else {
                let bg = if response.is_pointer_button_down_on() {
                    egui_shadcn::tokens::mix(palette.secondary, egui::Color32::WHITE, 0.12)
                } else if response.hovered() {
                    egui_shadcn::tokens::mix(palette.secondary, egui::Color32::WHITE, 0.08)
                } else {
                    palette.secondary
                };
                (bg, egui::Stroke::NONE)
            };
            ui.painter()
                .rect(rect, radius, bg, stroke, egui::StrokeKind::Inside);
            let text_pos = egui::pos2(
                rect.min.x + padding_x + icon_size + gap,
                rect.center().y - galley.size().y / 2.0,
            );
            ui.painter().with_clip_rect(rect).galley(
                text_pos,
                galley,
                palette.secondary_foreground,
            );
        }

        let icon_rect = egui::Rect::from_min_size(
            egui::pos2(rect.min.x + padding_x, rect.center().y - icon_size / 2.0),
            egui::vec2(icon_size, icon_size),
        );
        (response, icon_rect)
    }

    /// A panel button that respects the compact UI setting. Uses compact sizing
    /// when enabled, standard shadcn Sm otherwise.
    pub fn btn(&self, ui: &mut Ui, label: impl Into<WidgetText>) -> Response {
        if self.compact {
            self.button_compact(ui, label)
        } else {
            self.button_sm(ui, label)
        }
    }

    /// A single-line text field that shows the remaining character count
    /// (`char_limit` minus the current length) right-aligned in light blue,
    /// painted inside the field. Returns the text field `Response`.
    pub fn text_edit_counted(
        &self,
        ui: &mut Ui,
        text: &mut String,
        char_limit: usize,
        desired_width: f32,
        hint: Option<&str>,
    ) -> Response {
        let mut edit = egui::TextEdit::singleline(text)
            .char_limit(char_limit)
            .desired_width(desired_width);
        if let Some(h) = hint {
            edit = edit.hint_text(h);
        }
        let resp = ui.add(edit);

        let remaining = char_limit.saturating_sub(text.chars().count());
        let pos = egui::pos2(resp.rect.right() - 6.0, resp.rect.center().y);
        ui.painter().text(
            pos,
            egui::Align2::RIGHT_CENTER,
            remaining.to_string(),
            egui::FontId::proportional(13.0),
            egui::Color32::from_rgb(102, 178, 255),
        );
        resp
    }

    /// A panel toggle that respects the compact UI setting.
    pub fn tgl(&self, ui: &mut Ui, on: &mut bool, label: impl Into<WidgetText>) -> Response {
        if self.compact {
            self.toggle_compact(ui, on, label)
        } else {
            self.toggle_sm(ui, on, label)
        }
    }

    /// A momentary button styled and sized like an off-state compact toggle
    /// (`tgl`): same secondary fill, border, padding and (small) font. Used
    /// where action buttons sit next to toggles and must match their size.
    pub fn btn_small(&self, ui: &mut Ui, label: impl Into<WidgetText>) -> Response {
        let palette = &self.theme.palette;
        ui.scope(|ui| {
            ui.spacing_mut().button_padding = egui::vec2(6.0, 2.0);
            let label_widget: WidgetText = label.into();
            let btn = egui::Button::new(label_widget.color(palette.secondary_foreground))
                .fill(palette.secondary)
                .stroke(egui::Stroke::new(1.0_f32, palette.border))
                .corner_radius(4.0);
            ui.add(btn)
        })
        .inner
    }

    /// A themed destructive button (red, for irreversible actions).
    pub fn button_destructive(
        &self,
        ui: &mut Ui,
        label: impl Into<WidgetText>,
        enabled: bool,
    ) -> Response {
        egui_shadcn::button::button(
            ui,
            &self.theme,
            label,
            ControlVariant::Destructive,
            ControlSize::Md,
            enabled,
        )
    }

    /// A dialog (Md) secondary button with an explicit minimum width, for
    /// laying out equal-sized modal action buttons side by side.
    pub fn button_min_width(
        &self,
        ui: &mut Ui,
        label: impl Into<WidgetText>,
        min_width: f32,
    ) -> Response {
        Button::new(label)
            .variant(ButtonVariant::Secondary)
            .size(ButtonSize::from(ControlSize::Md))
            .min_width(min_width)
            .show(ui, &self.theme)
    }

    /// A destructive (Md) button with an explicit minimum width, to pair with
    /// `button_min_width` for equal-sized modal action buttons.
    pub fn button_destructive_min_width(
        &self,
        ui: &mut Ui,
        label: impl Into<WidgetText>,
        enabled: bool,
        min_width: f32,
    ) -> Response {
        Button::new(label)
            .variant(ButtonVariant::Destructive)
            .size(ButtonSize::from(ControlSize::Md))
            .enabled(enabled)
            .min_width(min_width)
            .show(ui, &self.theme)
    }

    /// A horizontal themed separator.
    pub fn separator(&self, ui: &mut Ui) -> Response {
        egui_shadcn::separator::separator(ui, &self.theme, SeparatorProps::default())
    }

    /// A small themed toggle button (on/off state). Uses the `Outline` variant
    /// so the off-state has a visible border and the on-state uses the accent.
    pub fn toggle_sm(&self, ui: &mut Ui, on: &mut bool, label: impl Into<WidgetText>) -> Response {
        egui_shadcn::toggle::toggle(
            ui,
            &self.theme,
            on,
            label,
            ToggleVariant::Outline,
            ControlSize::Sm,
            true,
        )
    }

    /// A compact themed toggle button with minimal padding (~22px height).
    /// On-state uses the accent color; off-state uses a subtle border.
    pub fn toggle_compact(
        &self,
        ui: &mut Ui,
        on: &mut bool,
        label: impl Into<WidgetText>,
    ) -> Response {
        let palette = &self.theme.palette;
        let (fill, stroke, text_color) = if *on {
            (
                palette.accent,
                egui::Stroke::new(1.0_f32, palette.accent),
                palette.accent_foreground,
            )
        } else {
            (
                palette.secondary,
                egui::Stroke::new(1.0_f32, palette.border),
                palette.secondary_foreground,
            )
        };
        let resp = ui
            .scope(|ui| {
                ui.spacing_mut().button_padding = egui::vec2(6.0, 2.0);
                let label_widget: WidgetText = label.into();
                let btn = egui::Button::new(label_widget.color(text_color))
                    .fill(fill)
                    .stroke(stroke)
                    .corner_radius(4.0);
                ui.add(btn)
            })
            .inner;
        if resp.clicked() {
            *on = !*on;
        }
        resp
    }

    /// A themed single-value slider over `[min, max]`. Wraps the crate's
    /// multi-thumb slider, which operates on a `Vec<f32>`.
    pub fn slider_value(
        &self,
        ui: &mut Ui,
        id: &str,
        value: &mut f32,
        min: f32,
        max: f32,
    ) -> Response {
        let before = *value;
        let mut values = vec![*value];
        let mut resp =
            egui_shadcn::slider::slider(ui, &self.theme, egui::Id::new(id), &mut values, min, max);
        if let Some(new_value) = values.first() {
            *value = *new_value;
        }
        // The underlying slider updates its value on a bare track click but does
        // not flag the response as changed, so clicking the empty bar would not
        // persist. Treat any value delta (track click, arrow keys, drag) as a
        // change so callers save it.
        if (*value - before).abs() > f32::EPSILON {
            resp.mark_changed();
        }
        // Grab keyboard focus on any pointer interaction (the crate only focuses
        // on a click, not a drag) so Left/Right arrows nudge this slider instead
        // of leaking to a neighbouring widget such as a boss-picker field.
        if resp.clicked() || resp.dragged() || resp.drag_started() {
            resp.request_focus();
        }
        // While focused, lock arrow keys to this slider. egui decides focus
        // navigation at the *start* of the next frame, so consuming events mid
        // frame is too late; instead register an event filter (the upstream
        // egui slider does this, but egui-shadcn's does not) so arrows nudge the
        // value instead of moving focus to a neighbouring widget.
        if resp.has_focus() {
            ui.ctx().memory_mut(|m| {
                m.set_focus_lock_filter(
                    resp.id,
                    egui::EventFilter {
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        ..Default::default()
                    },
                );
            });
        }
        resp
    }

    /// A surface card with a title/description header wrapping `contents`.
    pub fn card(
        &self,
        ui: &mut Ui,
        id: &str,
        heading: impl Into<String>,
        contents: impl FnOnce(&mut Ui),
    ) -> Response {
        let props = CardProps::default()
            .id(egui::Id::new(id))
            .variant(CardVariant::Surface)
            .heading(heading);
        // Force the card to fill the available horizontal space instead of
        // shrinking to its content width.
        egui_shadcn::card::card(ui, &self.theme, props, |card_ui| {
            card_ui.set_min_width(card_ui.available_width());
            contents(card_ui);
        })
    }

    /// A single-select dropdown over `options` (value, label) pairs.
    ///
    /// `selected` holds the string key of the current value. Returns the
    /// response; check `.changed()` and read `selected` for the new value.
    pub fn select(
        &self,
        ui: &mut Ui,
        id: &str,
        selected: &mut Option<String>,
        width: f32,
        options: &[(&str, &str)],
    ) -> Response {
        self.select_with_side(ui, id, selected, width, options, SelectSide::Bottom)
    }

    /// Same as [`Self::select`] but opens the popup upward instead of downward.
    /// Use for triggers that sit near the bottom of a scrollable panel (e.g. the
    /// last card in a settings tab), where opening downward can leave too little
    /// room and cause the popup to fail to open on smaller windows.
    pub fn select_up(
        &self,
        ui: &mut Ui,
        id: &str,
        selected: &mut Option<String>,
        width: f32,
        options: &[(&str, &str)],
    ) -> Response {
        self.select_with_side(ui, id, selected, width, options, SelectSide::Top)
    }

    fn select_with_side(
        &self,
        ui: &mut Ui,
        id: &str,
        selected: &mut Option<String>,
        width: f32,
        options: &[(&str, &str)],
        side: SelectSide,
    ) -> Response {
        let items: Vec<SelectItem> = options
            .iter()
            .map(|(value, label)| SelectItem::option(*value, *label))
            .collect();
        // The shadcn trigger paints its text unclipped, so a value wider than the
        // box overflows. Grow `width` (treated as a minimum) to fit the longest
        // option: text + left padding + the chevron icon zone.
        let font = egui::FontId::proportional(SelectSize::Size2.font_size());
        let max_label = options
            .iter()
            .map(|(_, label)| {
                ui.painter()
                    .layout_no_wrap(label.to_string(), font.clone(), egui::Color32::WHITE)
                    .size()
                    .x
            })
            .fold(0.0_f32, f32::max);
        let width = width.max(max_label + 44.0);
        let props = SelectProps::new(egui::Id::new(id), selected)
            .size(SelectSize::Size2)
            .width(width)
            .side(side)
            // The settings dialog renders at `Order::Tooltip`; raise the popup to
            // the same order so it isn't hidden behind the dialog frame.
            .container(SelectPortalContainer::Tooltip);
        egui_shadcn::select::select_with_items(ui, &self.theme, props, &items)
    }

    /// A panel select that respects the compact UI setting. Uses Size1 (24px)
    /// when compact, Size2 (32px) otherwise.
    pub fn sel(
        &self,
        ui: &mut Ui,
        id: &str,
        selected: &mut Option<String>,
        width: f32,
        options: &[(&str, &str)],
    ) -> Response {
        let size = if self.compact {
            SelectSize::Size1
        } else {
            SelectSize::Size2
        };
        let items: Vec<SelectItem> = options
            .iter()
            .map(|(value, label)| SelectItem::option(*value, *label))
            .collect();
        let font = egui::FontId::proportional(size.font_size());
        let max_label = options
            .iter()
            .map(|(_, label)| {
                ui.painter()
                    .layout_no_wrap(label.to_string(), font.clone(), egui::Color32::WHITE)
                    .size()
                    .x
            })
            .fold(0.0_f32, f32::max);
        let width = width.max(max_label + 44.0);
        let props = SelectProps::new(egui::Id::new(id), selected)
            .size(size)
            .width(width)
            .container(SelectPortalContainer::Tooltip);
        egui_shadcn::select::select_with_items(ui, &self.theme, props, &items)
    }

    /// A horizontal row whose height is fixed up front so short labels stay
    /// vertically centered next to taller controls (selects, switches). egui's
    /// horizontal layout does not retroactively re-center an item added before a
    /// taller one, so the row height must be known before contents are added.
    pub fn field_row<R>(&self, ui: &mut Ui, contents: impl FnOnce(&mut Ui) -> R) -> R {
        ui.horizontal(|ui| {
            // Cover the tallest control in a settings row (the 36px button) so
            // shorter items center against it regardless of insertion order.
            ui.set_min_height(36.0);
            contents(ui)
        })
        .inner
    }

    /// A swatch button that opens a color picker. egui's built-in
    /// `color_edit_button_srgba` shows its popup at `Order::Foreground`, which the
    /// settings dialog (`Order::Tooltip`) paints over, so the picker is hidden.
    /// This mirrors it but forces the popup to `Order::Tooltip`. Returns true when
    /// the color changed.
    // WIP: used by the custom-theme color feature, disabled until light mode is
    // supported (see app.rs). Re-enable together with that block.
    #[allow(dead_code)]
    pub fn color_picker(&self, ui: &mut Ui, id: &str, color: &mut egui::Color32) -> bool {
        use egui::color_picker::{color_picker_color32, show_color, Alpha};

        let popup_id = egui::Id::new(id).with("rh_color_popup");
        let swatch = show_color(ui, *color, egui::vec2(40.0, 22.0))
            .interact(egui::Sense::click())
            .hover_tip("Click to edit color");
        // Draw a border so the swatch reads as a button against the card.
        ui.painter().rect_stroke(
            swatch.rect,
            4.0,
            egui::Stroke::new(1.0_f32, self.theme.palette.border),
            egui::StrokeKind::Inside,
        );

        let mut changed = false;
        egui::Popup::menu(&swatch)
            .id(popup_id)
            .kind(egui::PopupKind::Tooltip)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.spacing_mut().slider_width = 240.0;
                changed = color_picker_color32(ui, color, Alpha::Opaque);
            });
        changed
    }

    /// A fixed-size modal dialog. `open` is set to `false` by the dialog itself
    /// when dismissed via Escape, the close button, or a backdrop click.
    ///
    /// Returns `Some(R)` while the dialog is open, mirroring the crate API.
    #[allow(clippy::too_many_arguments)]
    pub fn dialog<R>(
        &self,
        ui: &mut Ui,
        id: &str,
        open: &mut bool,
        title: impl Into<String>,
        width: f32,
        height: f32,
        contents: impl FnOnce(&mut Ui) -> R,
    ) -> Option<R> {
        let props = DialogProps::new(egui::Id::new(id), open)
            .title(title)
            .align(DialogAlign::Center)
            .width(width)
            .max_width(width)
            .height(height)
            .scrollable(false)
            .close_on_escape(true)
            .close_on_background(true)
            .show_close_button(true)
            .close_button_text("×");
        egui_shadcn::dialog::dialog(ui, &self.theme, props, contents)
    }

    /// Like [`Self::dialog`] but only closable via the X button: clicking the
    /// background or pressing Escape does not dismiss it. Used for the Settings
    /// window so a date-picker dropdown or stray outside click can't close it.
    pub fn dialog_sticky<R>(
        &self,
        ui: &mut Ui,
        id: &str,
        open: &mut bool,
        title: impl Into<String>,
        width: f32,
        height: f32,
        contents: impl FnOnce(&mut Ui) -> R,
    ) -> Option<R> {
        let props = DialogProps::new(egui::Id::new(id), open)
            .title(title)
            .align(DialogAlign::Center)
            .width(width)
            .max_width(width)
            .height(height)
            .scrollable(false)
            .close_on_escape(false)
            .close_on_background(false)
            .show_close_button(true)
            .close_button_text("×")
            // Drop the frame's right inner padding so a vertical scroll bar in
            // the content hugs the dialog's right border. Content re-adds its
            // own right gutter so cards don't touch the bar.
            .tokens_override({
                let mut tokens = egui_shadcn::dialog::dialog_tokens_with_options(
                    &self.theme,
                    egui_shadcn::dialog::DialogSize::Size3,
                    false,
                );
                tokens.layout.padding.right = 0;
                tokens
            });
        egui_shadcn::dialog::dialog(ui, &self.theme, props, contents)
    }

    /// Two-month date range picker. Two-click selection: first click sets start, second sets end.
    pub fn date_range_picker(
        &self,
        ui: &mut Ui,
        id: &str,
        start: &mut chrono::NaiveDate,
        end: &mut chrono::NaiveDate,
    ) -> Response {
        let mem_id = ui.make_persistent_id(id).with("range_pending");
        let pending_start: Option<chrono::NaiveDate> =
            ui.ctx().data(|d| d.get_temp(mem_id)).flatten();

        // Discard stale pending state if caller reset the dates
        let pending_start = pending_start.filter(|ps| *ps == *start);

        let mut range = if let Some(ps) = pending_start {
            egui_shadcn::date_picker::DateRange {
                from: Some(ps),
                to: None,
            }
        } else {
            egui_shadcn::date_picker::DateRange {
                from: Some(*start),
                to: Some(*end),
            }
        };

        let picker_id = ui.make_persistent_id(id);
        let open_id = picker_id.with("open");
        let mut open = ui
            .ctx()
            .memory_mut(|memory| memory.data.get_persisted::<bool>(open_id).unwrap_or(false));
        let label = match (range.from, range.to) {
            (Some(from), Some(to)) => {
                format!("{} - {}", from.format("%b %d, %Y"), to.format("%b %d, %Y"))
            }
            (Some(from), None) => from.format("%b %d, %Y").to_string(),
            _ => "Pick a date".to_string(),
        };
        let selection = Rc::new(RefCell::new((range.from, range.to)));
        let (resp, selection_result) = popover(
            ui,
            &self.theme,
            PopoverProps::new(picker_id.with("popover"), &mut open)
                .side(PopoverSide::Bottom)
                .align(PopoverAlign::Start)
                .width(576.0)
                .max_height(420.0)
                .content_padding(egui::Margin::same(0))
                .animation(true),
            |ui| {
                if self.compact {
                    let width = 180.0;
                    let height = 24.0;
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
                    if ui.is_rect_visible(rect) {
                        let palette = &self.theme.palette;
                        let stroke = if response.hovered() {
                            egui::Stroke::new(1.0_f32, palette.accent)
                        } else {
                            egui::Stroke::new(1.0_f32, palette.border)
                        };
                        ui.painter().rect(
                            rect,
                            4.0,
                            palette.background,
                            stroke,
                            egui::StrokeKind::Inside,
                        );
                        icon_calendar(
                            ui.painter(),
                            egui::pos2(rect.left() + 12.0, rect.center().y),
                            14.0,
                            palette.foreground,
                        );
                        let text = WidgetText::from(label.clone()).into_galley(
                            ui,
                            Some(egui::TextWrapMode::Truncate),
                            width - 34.0,
                            egui::TextStyle::Button,
                        );
                        ui.painter().with_clip_rect(rect).galley(
                            egui::pos2(rect.left() + 24.0, rect.center().y - text.size().y / 2.0),
                            text,
                            palette.foreground,
                        );
                    }
                    response.hover_tip(label)
                } else {
                    Button::new(label)
                        .variant(ButtonVariant::Outline)
                        .size(ButtonSize::Sm)
                        .justify(ButtonJustify::Start)
                        .icon(&icon_calendar)
                        .min_width(220.0)
                        .show(ui, &self.theme)
                }
            },
            |ui| {
                let selection = selection.clone();
                let callback_selection = selection.clone();
                calendar_with_props(
                    ui,
                    &self.theme,
                    CalendarProps::new(picker_id.with("calendar"))
                        .mode(CalendarMode::Range)
                        .range_start(range.from)
                        .range_end(range.to)
                        .default_month(
                            range
                                .from
                                .unwrap_or_else(|| chrono::Local::now().date_naive()),
                        )
                        .number_of_months(2)
                        .on_range_select(move |from, to| {
                            *callback_selection.borrow_mut() = (from, to);
                        }),
                );

                let result = *selection.borrow();
                Some(result)
            },
        );
        ui.memory_mut(|memory| memory.data.insert_persisted(open_id, open));

        if let Some((from, to)) = selection_result.flatten() {
            range = egui_shadcn::date_picker::DateRange { from, to };
        }

        match (range.from, range.to) {
            (Some(s), Some(e)) => {
                *start = s;
                *end = e;
                ui.ctx()
                    .data_mut(|d| d.insert_temp::<Option<chrono::NaiveDate>>(mem_id, None));
            }
            (Some(s), None) if pending_start != Some(s) => {
                // First click: update start so the stale-state guard preserves
                // this pending value on the next frame.
                *start = s;
                ui.ctx().data_mut(|d| d.insert_temp(mem_id, Some(s)));
            }
            _ => {}
        }

        resp
    }

    /// Single-day calendar date picker (Loot History style). Shows a button with
    /// the current selection that opens a one-month calendar popover. Dates
    /// before `min_date` are disabled. Returns `true` when the selection changes.
    #[allow(dead_code)]
    pub fn date_picker(
        &self,
        ui: &mut Ui,
        id: &str,
        selected: &mut Option<chrono::NaiveDate>,
        min_date: Option<chrono::NaiveDate>,
    ) -> bool {
        let picker_id = ui.make_persistent_id(id);
        let open_id = picker_id.with("open");
        let mut open = ui
            .ctx()
            .memory_mut(|memory| memory.data.get_persisted::<bool>(open_id).unwrap_or(false));

        let label = match selected {
            Some(d) => d.format("%b %d, %Y").to_string(),
            None => "Pick a date".to_string(),
        };
        let default_month = selected
            .or(min_date)
            .unwrap_or_else(|| chrono::Local::now().date_naive());
        let picked = Rc::new(RefCell::new(None::<chrono::NaiveDate>));

        let (_resp, selection_result) = popover(
            ui,
            &self.theme,
            PopoverProps::new(picker_id.with("popover"), &mut open)
                .side(PopoverSide::Bottom)
                .align(PopoverAlign::Start)
                .width(320.0)
                .max_height(360.0)
                .content_padding(egui::Margin::same(0))
                .animation(true),
            |ui| {
                Button::new(label.clone())
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::Sm)
                    .justify(ButtonJustify::Start)
                    .icon(&icon_calendar)
                    .min_width(200.0)
                    .show(ui, &self.theme)
            },
            |ui| {
                let sink = picked.clone();
                calendar_with_props(
                    ui,
                    &self.theme,
                    CalendarProps::new(picker_id.with("calendar"))
                        .mode(CalendarMode::Single)
                        .selected(*selected)
                        .min_date(min_date)
                        .default_month(default_month)
                        .number_of_months(1)
                        .on_select(move |d| {
                            *sink.borrow_mut() = d;
                        }),
                );
                *picked.borrow()
            },
        );
        ui.memory_mut(|memory| memory.data.insert_persisted(open_id, open));

        let mut changed = false;
        if let Some(Some(d)) = selection_result {
            if *selected != Some(d) {
                *selected = Some(d);
                changed = true;
            }
            // Close the popover once a day is chosen.
            ui.memory_mut(|memory| memory.data.insert_persisted(open_id, false));
        }
        changed
    }
}

#[cfg(test)]
mod band_row_wrapped_tests {
    use super::*;

    fn band_height(screen_w: f32, button_count: usize) -> f32 {
        let shadcn = Shadcn::default();
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(screen_w, 400.0),
        ));
        let height = std::cell::Cell::new(0.0_f32);
        // Two passes: egui needs a prior frame to know sizes.
        for _ in 0..2 {
            let _ = ctx.run(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let before = ui.cursor().top();
                    // Reproduce the real call site: the wrapping row lives inside
                    // a `header_band` frame.
                    shadcn.header_band(ui, shadcn.theme.palette.card, |ui| {
                        shadcn.band_row_wrapped(ui, |ui| {
                            for i in 0..button_count {
                                let _ = ui.button(format!("Button {i}"));
                            }
                        });
                    });
                    height.set(ui.cursor().top() - before);
                });
            });
        }
        height.get()
    }

    #[test]
    fn folds_onto_extra_rows_when_narrow() {
        // Wide window: everything fits on one band row.
        let wide = band_height(2000.0, 12);
        // Narrow window with the same content: must wrap to a taller band.
        let narrow = band_height(300.0, 12);
        assert!(
            narrow > wide + 10.0,
            "narrow row should wrap taller (narrow={narrow}, wide={wide})"
        );
    }
}

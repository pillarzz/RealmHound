//! Floating, always-on-top overlay windows drawn over the game client.
//!
//! Two overlays are provided: a live DPS meter for the boss fight currently in
//! progress, and a loot tracker tallying the bags that have dropped this
//! session. Both are separate native windows (egui *immediate viewports*) so
//! they can sit over a fullscreen game client while the main RealmHound window
//! stays on another monitor or minimized.
//!
//! Shared behaviour, provided here and reused by both overlays:
//!
//! - Undecorated and transparent, with a rounded translucent backdrop so the
//!   game stays visible underneath.
//! - **Locked** overlays pass every mouse event straight through to the game, so
//!   the user can shoot "through" the panel. The title bar, the tab strips, the
//!   scrollbars and the DPS rows' name labels are the exceptions: they stay live
//!   so a locked overlay can still be unlocked, navigated, scrolled and copied
//!   from. A locked overlay is also made
//!   non-resizable, because the window manager would otherwise resize it from
//!   the very edges those live regions sit on. Unlocking restores dragging and
//!   resizing via the title bar and the bottom-right grip.
//! - Never takes focus. Clicking an overlay must not pull input away from the
//!   game; the native tweaks in [`platform`] enforce this where the windowing
//!   layer cannot.
//! - Position, size and chrome state round-trip through
//!   [`realmhound_core::settings::OverlaySettings`].

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, CornerRadius, RichText};
use realmhound_core::combat::LiveFightSnapshot;
use realmhound_core::loot::ProcessedLootDrop;
use realmhound_core::settings::{OverlayWindowSettings, Settings};

use crate::panels::loot_filters::{LootFilters, PropertyFilters};
use crate::rendering::SpriteRenderer;

mod dps;
mod loot;
pub mod platform;

/// Window title prefixes. [`platform`] uses them to tell one overlay's windows
/// from the other's, so they must stay unique and stable. The title actually
/// given to a window appends [`next_generation`].
const DPS_TITLE: &str = "RealmHound DPS Overlay";
const LOOT_TITLE: &str = "RealmHound Loot Overlay";

/// Source of the per-window suffix that makes every native overlay title
/// unique.
///
/// eframe drops and rebuilds a viewport's window whenever the viewport is
/// pruned or its builder asks for a recreate, and on macOS the outgoing
/// `NSWindow` can outlive that drop: AppKit keeps closed-but-not-yet-released
/// windows in `NSApplication::windows`. Matching on a fixed title therefore
/// risks promoting and raising a dead window, which puts a frozen frame from a
/// previous instance back on screen under the live overlay -- the "ghost"
/// overlay that darkens when clicked, because the click reorders it to the
/// front. Stamping a generation into the title makes "the window we are
/// rendering" unambiguous and lets [`platform::dismiss_stale`] retire the rest.
static WINDOW_GENERATION: AtomicU64 = AtomicU64::new(0);

fn next_generation() -> u64 {
    WINDOW_GENERATION.fetch_add(1, Ordering::Relaxed)
}

/// Height of the chrome row holding the title and its buttons.
const TITLE_BAR_H: f32 = 20.0;
/// Side of the square bottom-right resize grip.
const GRIP: f32 = 12.0;
/// Smallest useful overlay size; below this the rows stop being readable.
const MIN_W: f32 = 180.0;
const MIN_H: f32 = 64.0;
/// Inner height of a minimized overlay: the title bar plus the frame margins.
const COLLAPSED_H: f32 = TITLE_BAR_H + 10.0;

/// How often the native promotion is re-applied.
///
/// The window level is not sticky: switching Spaces on macOS, or another app
/// claiming topmost on Windows, can demote the overlay. Re-asserting on a timer
/// is cheap (a title lookup plus a couple of setters) and keeps the overlay up
/// without a per-frame native call.
const PROMOTE_INTERVAL: Duration = Duration::from_secs(2);

/// How often a locked overlay polls the pointer to decide whether to suspend
/// click-through over its title bar.
const CHROME_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Overlay refresh rate while something on screen is actually moving.
///
/// An overlay is an *immediate* viewport, so one overlay frame is also a full
/// re-layout and repaint of the main window plus a vsynced buffer swap per
/// window. Running that at the display's rate would make an open overlay cost
/// more than the rest of the app put together, on a machine that is
/// simultaneously running the game the overlay sits on top of. 20 fps is far
/// more than a damage readout needs.
const ACTIVE_FRAME_INTERVAL: Duration = Duration::from_millis(50);

/// Keep-alive refresh rate when no fight is running.
///
/// Not zero: the overlays must keep presenting fresh frames rather than
/// whatever the windowing layer last left in the back buffer. Everything that
/// *changes* while idle (a loot drop, a new fight) already wakes the UI through
/// the worker, so this only has to cover the buffers.
const IDLE_FRAME_INTERVAL: Duration = Duration::from_millis(500);

/// Height of the band, measured from the top of the window, that counts as the
/// title bar for pointer hit-testing. Covers the button row plus the frame's
/// top margin.
const CHROME_HIT_H: f32 = TITLE_BAR_H + 10.0;

/// Width of the band, measured from the right edge of a scroll area, that
/// counts as its scrollbar for pointer hit-testing. Covers the solid bar, its
/// inner margin and the frame's right margin.
const SCROLL_HIT_W: f32 = 16.0;

/// Inset of a chrome tooltip from the window edges.
const TOOLTIP_MARGIN: f32 = 6.0;

/// Title-bar glyphs, restricted to characters the app's embedded fonts
/// actually have. Several obvious choices (U+2715 close, U+25A1 and U+2610 for
/// restore, U+21BA for reset) are absent from Noto Sans and Noto Emoji and draw
/// as empty tofu boxes; `chrome_glyphs_render` guards against that returning.
///
/// The lock toggle is the exception: see [`lock_button`].
const CLOSE_GLYPH: &str = "\u{2716}";
const MINIMIZE_GLYPH: &str = "\u{2212}";
const RESTORE_GLYPH: &str = "+";
/// The loot overlay's reset button, guarded by the same test.
pub(super) const RESET_GLYPH: &str = "\u{1F504}";

/// Backdrop color of every overlay, before opacity is applied.
const BACKDROP: Color32 = Color32::from_rgb(15, 15, 18);

/// Per-window runtime state that is deliberately *not* persisted.
struct WindowRuntime {
    title: &'static str,
    /// The title the *current* native window was opened with: [`Self::title`]
    /// plus a generation suffix. Empty until the window is first built.
    native_title: String,
    id: egui::ViewportId,
    /// Built once when the window first opens and then reused verbatim.
    ///
    /// egui diffs the builder against the previous frame's and turns any change
    /// into a viewport command. Rebuilding it each frame from the persisted
    /// geometry would therefore fight the user: they drag the window, we write
    /// the new position into settings, and the next frame's builder shoves the
    /// window back. Freezing the builder keeps the diff empty and leaves
    /// runtime changes to explicit [`egui::ViewportCommand`]s.
    builder: Option<egui::ViewportBuilder>,
    /// Last pass-through state pushed to the backend, so the command is only
    /// sent on an actual change.
    passthrough: Option<bool>,
    /// Last minimized state pushed to the backend, for the same reason.
    minimized: Option<bool>,
    /// Last resizable state pushed to the backend, for the same reason.
    resizable: Option<bool>,
    /// Whether the window reported itself covered on the previous promote tick.
    /// [`platform::order_front`] is edge-triggered on it.
    hidden: bool,
    last_promoted: Option<Instant>,
}

impl WindowRuntime {
    fn new(title: &'static str) -> Self {
        Self {
            title,
            native_title: String::new(),
            id: egui::ViewportId::from_hash_of(title),
            builder: None,
            passthrough: None,
            minimized: None,
            resizable: None,
            hidden: false,
            last_promoted: None,
        }
    }

    /// Forget the native window so the next open rebuilds it from the persisted
    /// geometry.
    ///
    /// Called every frame the overlay is hidden, so the native teardown is
    /// gated on there having been a window to tear down.
    fn closed(&mut self) {
        if self.builder.is_some() {
            // eframe prunes the viewport after this frame, but on macOS the
            // `NSWindow` can outlive that; take it off screen while we still
            // know which one it is.
            platform::dismiss_stale(self.title, "");
        }
        self.builder = None;
        self.native_title.clear();
        self.passthrough = None;
        self.minimized = None;
        self.resizable = None;
        self.hidden = false;
        self.last_promoted = None;
    }

    fn builder_for(&mut self, cfg: &OverlayWindowSettings) -> egui::ViewportBuilder {
        if let Some(builder) = &self.builder {
            return builder.clone();
        }
        // A persisted-minimized overlay opens collapsed, so there is no flash of
        // full-size chrome before `sync_native` sends the resize.
        let (min_h, height) = if cfg.minimized {
            (COLLAPSED_H, COLLAPSED_H)
        } else {
            (MIN_H, cfg.height.max(MIN_H))
        };
        self.native_title = format!("{} #{}", self.title, next_generation());
        tracing::info!("[OVERLAY] opening window {:?}", self.native_title);
        let mut builder = egui::ViewportBuilder::default()
            .with_title(&self.native_title)
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(!cfg.locked)
            .with_always_on_top()
            .with_taskbar(false)
            // Opening the overlay must never pull focus off the game.
            .with_active(false)
            .with_inner_size([cfg.width.max(MIN_W), height])
            .with_min_inner_size([MIN_W, min_h])
            .with_mouse_passthrough(cfg.locked);
        if let (Some(x), Some(y)) = (cfg.x, cfg.y) {
            if x.is_finite() && y.is_finite() {
                builder = builder.with_position([x, y]);
            }
        }
        self.builder.insert(builder).clone()
    }

    /// Push a pass-through change and re-assert the native promotion on a timer.
    fn sync_native(&mut self, ctx: &egui::Context, cfg: &OverlayWindowSettings, passthrough: bool) {
        if self.passthrough != Some(passthrough) {
            ctx.send_viewport_cmd(egui::ViewportCommand::MousePassthrough(passthrough));
            self.passthrough = Some(passthrough);
        }
        if self.minimized != Some(cfg.minimized) {
            // The minimum has to move first, or it would clamp the collapse.
            let (min_h, height) = if cfg.minimized {
                (COLLAPSED_H, COLLAPSED_H)
            } else {
                (MIN_H, cfg.height.max(MIN_H))
            };
            let width = cfg.width.max(MIN_W);
            ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::vec2(
                MIN_W, min_h,
            )));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(width, height)));
            self.minimized = Some(cfg.minimized);
        }
        // An undecorated window is still resizable from its edges by the window
        // manager itself (winit asks macOS for `Borderless | Resizable`), and
        // the scrollbar gutter a locked overlay keeps live sits right on one.
        // Taking the style bit away is the only way to stop the drag.
        if self.resizable != Some(!cfg.locked) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Resizable(!cfg.locked));
            self.resizable = Some(!cfg.locked);
        }
        let due = self
            .last_promoted
            .is_none_or(|at| at.elapsed() >= PROMOTE_INTERVAL);
        if due {
            // Retire leftovers first: raising the live window afterwards leaves
            // it on top even if the stale one was in front a moment ago.
            platform::dismiss_stale(self.title, &self.native_title);
            platform::promote(&self.native_title);
            self.hidden = platform::order_front(&self.native_title, self.hidden);
            self.last_promoted = Some(Instant::now());
        }
    }
}

/// Owns both overlay windows and the state they render from.
pub struct OverlayManager {
    dps_window: WindowRuntime,
    dps: dps::DpsOverlay,
    loot_window: WindowRuntime,
    loot: loot::LootOverlay,
    /// Mirrors "at least one overlay is open" for the packet worker, which uses
    /// it to keep waking the UI while a background tab is selected.
    active: Arc<AtomicBool>,
    /// Set once if the backend turned out not to support real child windows, so
    /// the warning is logged a single time instead of every frame.
    embedded_warned: bool,
}

impl Default for OverlayManager {
    fn default() -> Self {
        Self::new()
    }
}

impl OverlayManager {
    pub fn new() -> Self {
        Self {
            dps_window: WindowRuntime::new(DPS_TITLE),
            dps: dps::DpsOverlay::default(),
            loot_window: WindowRuntime::new(LOOT_TITLE),
            loot: loot::LootOverlay::default(),
            active: Arc::new(AtomicBool::new(false)),
            embedded_warned: false,
        }
    }

    /// Shared flag the worker reads to decide whether overlay-only data (live
    /// DPS, loot) still needs to wake the UI on a background tab.
    pub fn active_signal(&self) -> Arc<AtomicBool> {
        self.active.clone()
    }

    /// Record a loot drop for the loot overlay's counters and recent list.
    ///
    /// Called for every drop regardless of whether the overlay is open, so
    /// opening it mid-session shows the totals so far rather than starting from
    /// zero.
    pub fn push_loot(&mut self, drop: &ProcessedLootDrop) {
        self.loot.push(drop);
    }

    /// Clear the loot tallies (new session / user reset).
    pub fn reset_loot(&mut self) {
        self.loot.reset();
    }

    /// Render whichever overlays are enabled. Call once per frame from the root
    /// `eframe::App::update`, before the main window's panels.
    pub fn render(
        &mut self,
        ctx: &egui::Context,
        settings: &Arc<RwLock<Settings>>,
        sprites: &mut SpriteRenderer,
        live: &[Arc<LiveFightSnapshot>],
    ) {
        // The loot overlay shares the Live Feed's bag and item toggles rather
        // than offering a second place to configure the same thing.
        let Ok((mut cfg, bag_filters, item_filters)) = settings.read().map(|s| {
            (
                s.overlays.clone(),
                LootFilters::from_live_feed_settings(&s.live_feed.filters),
                PropertyFilters::from_live_feed_settings(&s.live_feed.filters),
            )
        }) else {
            return;
        };
        self.active.store(cfg.any_visible(), Ordering::Relaxed);
        if !cfg.any_visible() {
            self.dps_window.closed();
            self.loot_window.closed();
            return;
        }

        // Overlays are *immediate* viewports, which eframe can only paint from
        // inside the parent window's update: asking the overlay's own window to
        // redraw just bounces the request to its parent. An idle main window
        // therefore stops refreshing them, and every buffer swap then presents
        // whichever frame is still sitting in the back buffer -- a long-dead
        // fight drawn over the live one. Driving the parent keeps both buffers
        // current for as long as an overlay is on screen.
        //
        // The rate is deliberate, not "as fast as the display will go": the
        // main window is demand-driven everywhere else, and each overlay frame
        // drags its entire UI along.
        let animating = cfg.dps.visible && !cfg.dps.minimized && live.iter().any(|f| !f.ended);
        ctx.request_repaint_after(if animating {
            ACTIVE_FRAME_INTERVAL
        } else {
            IDLE_FRAME_INTERVAL
        });

        let before = cfg.clone();
        let opacity = cfg.clamped_opacity();
        let max_rows = cfg.max_rows();

        if cfg.dps.visible {
            let mut content = dps::DpsContent {
                overlay: &mut self.dps,
                fights: live,
                follow: &mut cfg.dps_follow_me,
                max_rows,
            };
            let embedded = show_window(
                ctx,
                &mut self.dps_window,
                &mut cfg.dps,
                opacity,
                sprites,
                &mut content,
            );
            self.warn_if_embedded(embedded);
        } else {
            self.dps_window.closed();
        }

        if cfg.loot.visible {
            let mut content = loot::LootContent {
                overlay: &mut self.loot,
                filters: &bag_filters,
                properties: &item_filters,
            };
            let embedded = show_window(
                ctx,
                &mut self.loot_window,
                &mut cfg.loot,
                opacity,
                sprites,
                &mut content,
            );
            self.warn_if_embedded(embedded);
        } else {
            self.loot_window.closed();
        }

        if cfg.dps != before.dps
            || cfg.loot != before.loot
            || cfg.dps_follow_me != before.dps_follow_me
        {
            if let Ok(mut s) = settings.write() {
                // Only the state the windows themselves own. Writing the whole
                // struct back would clobber a preference the settings tab
                // changed after this clone was taken.
                s.overlays.dps = cfg.dps.clone();
                s.overlays.loot = cfg.loot.clone();
                s.overlays.dps_follow_me = cfg.dps_follow_me;
            }
        }
    }

    fn warn_if_embedded(&mut self, embedded: bool) {
        if embedded && !self.embedded_warned {
            self.embedded_warned = true;
            tracing::warn!(
                "[OVERLAY] this backend cannot open child windows; overlays are unavailable"
            );
        }
    }
}

/// What an individual overlay contributes to the shared window frame.
///
/// A trait rather than a pair of closures because both halves need `&mut` on the
/// same overlay state, which two closures captured at once cannot express.
pub(super) trait OverlayContent {
    /// Buttons unique to this overlay, placed left of the shared
    /// lock / minimize / close trio.
    fn title_extras(&mut self, ui: &mut egui::Ui);

    /// The overlay's contents below the title bar. Anything that has to stay
    /// clickable on a locked overlay (tab strips, scrollbars, clickable labels)
    /// must register itself with `hits`.
    fn body(&mut self, ui: &mut egui::Ui, sprites: &mut SpriteRenderer, hits: &mut HitRegions);
}

/// Parts of the body that must keep receiving the mouse while the overlay is
/// locked, in window-local points.
///
/// A locked overlay passes clicks through, so egui never sees a hover and
/// cannot answer "is the pointer on the scrollbar". The regions are therefore
/// collected from the real layout each frame and hit-tested against the OS
/// cursor.
#[derive(Default)]
pub(super) struct HitRegions(Vec<egui::Rect>);

impl HitRegions {
    pub(super) fn add(&mut self, rect: egui::Rect) {
        if rect.is_positive() {
            self.0.push(rect);
        }
    }

    fn contains(&self, local: egui::Pos2) -> bool {
        self.0.iter().any(|rect| rect.contains(local))
    }
}

/// Open (or update) one overlay window and render its chrome plus content.
///
/// Returns `true` when the backend could not give us a real child window, in
/// which case nothing was drawn.
fn show_window(
    ctx: &egui::Context,
    runtime: &mut WindowRuntime,
    cfg: &mut OverlayWindowSettings,
    opacity: f32,
    sprites: &mut SpriteRenderer,
    content: &mut dyn OverlayContent,
) -> bool {
    let builder = runtime.builder_for(cfg);
    let id = runtime.id;
    let title = runtime.title;
    let mut embedded = false;

    ctx.show_viewport_immediate(id, builder, |ctx, class| {
        if class == egui::ViewportClass::Embedded {
            embedded = true;
            return;
        }

        if ctx.input(|i| i.viewport().close_requested()) {
            cfg.visible = false;
        }
        persist_geometry(ctx, cfg);

        // The backdrop is painted by hand rather than by the panel's frame so
        // it can be sequenced after `wipe_buffer`; a frame fill is emitted
        // before anything the closure draws.
        let frame = egui::Frame::NONE.inner_margin(egui::Margin::symmetric(6, 4));

        // Only the backdrop takes the opacity. Fading the text along with it
        // (which is all a window-opacity overlay can do) makes the numbers
        // unreadable against a bright game background, which defeats the point.
        let mut hits = HitRegions::default();
        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            wipe_buffer(ui);
            ui.painter().rect_filled(
                ui.ctx().viewport_rect(),
                CornerRadius::same(8),
                BACKDROP.gamma_multiply(perceptual_alpha(opacity)),
            );
            // Floating scrollbars only materialize on hover, and a locked
            // overlay never sees a hover, so the user would have nothing to aim
            // at. A solid bar is always there to grab.
            ui.style_mut().spacing.scroll = egui::style::ScrollStyle::solid();
            chrome(ui, title, cfg, content);
            if !cfg.minimized {
                ui.separator();
                content.body(ui, sprites, &mut hits);
                resize_grip(ui, cfg.locked);
            }
        });

        // A locked overlay passes clicks through to the game, but its own title
        // bar must stay usable or the only way back out is the settings window,
        // and its tabs and scrollbars must stay usable without unlocking first.
        // Pass-through is dropped while the pointer is over any of them.
        let over_chrome = cfg.locked && pointer_over_interactive(ctx, &hits);
        if cfg.locked {
            // No mouse events arrive while passing through, so the pointer has
            // to be polled rather than waited for.
            ctx.request_repaint_after(CHROME_POLL_INTERVAL);
        }

        // Viewport commands target whichever viewport is current, so this has to
        // run inside the callback: from outside it would reconfigure the main
        // window instead. Last, so this frame's chrome clicks are included.
        runtime.sync_native(ctx, cfg, cfg.locked && !over_chrome);
    });

    if !cfg.visible {
        runtime.closed();
    }
    embedded
}

/// Erase the overlay window's colour buffer as the first thing in its draw
/// list.
///
/// eframe clears an immediate viewport before painting it, but that clear is
/// issued straight after a GL context switch and does not reliably survive one
/// on macOS -- the same hazard its own "early clear doesn't take on Mac with
/// multiple viewports" workaround covers for the root window. Whatever the
/// previous swap left behind then shows through the translucent backdrop, which
/// is how a stale "No fight in progress" frame ends up sitting under live rows.
/// A clear submitted as a paint callback runs in the overlay's own command
/// stream, immediately before its first shape, so nothing can reorder it away.
fn wipe_buffer(ui: &egui::Ui) {
    let rect = ui.ctx().viewport_rect();
    ui.painter().add(egui::PaintCallback {
        rect,
        callback: Arc::new(eframe::egui_glow::CallbackFn::new(|_, painter| {
            use eframe::glow::HasContext as _;

            let gl = painter.gl();
            // SAFETY: the painter hands us its current context, and egui_glow
            // re-applies its own render state after the callback returns.
            unsafe {
                gl.disable(eframe::glow::SCISSOR_TEST);
                gl.clear_color(0.0, 0.0, 0.0, 0.0);
                gl.clear(eframe::glow::COLOR_BUFFER_BIT);
            }
        })),
    });
}

/// Whether the OS pointer is over a part of this overlay that has to keep
/// working while locked: the title bar, plus whatever the body registered.
///
/// Asks the platform rather than egui because a click-through window receives
/// no pointer events at all.
fn pointer_over_interactive(ctx: &egui::Context, hits: &HitRegions) -> bool {
    let Some(outer) = ctx.input(|i| i.viewport().outer_rect) else {
        return false;
    };
    let Some((x, y)) = platform::cursor_position(ctx.pixels_per_point()) else {
        return false;
    };
    let pointer = egui::pos2(x, y);
    if !outer.contains(pointer) {
        return false;
    }
    chrome_strip(outer).contains(pointer) || hits.contains(pointer - outer.min.to_vec2())
}

/// The title-bar band of a window at `outer`, in screen points.
fn chrome_strip(outer: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        outer.min,
        egui::pos2(outer.max.x, outer.min.y + CHROME_HIT_H),
    )
}

/// The vertical scrollbar gutter of a scroll area, or `None` when its content
/// fit and no bar is in play.
pub(super) fn scrollbar_hit<R>(
    output: &egui::scroll_area::ScrollAreaOutput<R>,
) -> Option<egui::Rect> {
    let inner = output.inner_rect;
    if output.content_size.y <= inner.height() + 0.5 {
        return None;
    }
    Some(egui::Rect::from_min_max(
        egui::pos2(inner.max.x - SCROLL_HIT_W, inner.min.y),
        egui::pos2(inner.max.x + SCROLL_HIT_W, inner.max.y),
    ))
}

/// Mirror the window's live geometry into the persisted settings.///
/// Updates are deadbanded to whole pixels: the window manager reports
/// fractional positions on scaled displays, and reacting to that jitter would
/// mark the settings dirty on every frame.
fn persist_geometry(ctx: &egui::Context, cfg: &mut OverlayWindowSettings) {
    fn moved(current: Option<f32>, next: f32) -> bool {
        current.is_none_or(|value| (value - next).abs() >= 1.0)
    }

    let (outer, inner) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().inner_rect));
    if let Some(rect) = outer {
        if rect.min.x.is_finite() && rect.min.y.is_finite() {
            if moved(cfg.x, rect.min.x) {
                cfg.x = Some(rect.min.x);
            }
            if moved(cfg.y, rect.min.y) {
                cfg.y = Some(rect.min.y);
            }
        }
    }
    // A minimized overlay is collapsed to its title bar; persisting that as the
    // window size would lose the restored height.
    if cfg.minimized {
        return;
    }
    if let Some(rect) = inner {
        if rect.width() >= MIN_W && (cfg.width - rect.width()).abs() >= 1.0 {
            cfg.width = rect.width();
        }
        if rect.height() >= MIN_H && (cfg.height - rect.height()).abs() >= 1.0 {
            cfg.height = rect.height();
        }
    }
}

/// The shared title bar: drag handle, overlay-specific buttons, then
/// lock / minimize / close.
fn chrome(
    ui: &mut egui::Ui,
    title: &str,
    cfg: &mut OverlayWindowSettings,
    content: &mut dyn OverlayContent,
) {
    let locked = cfg.locked;
    ui.horizontal(|ui| {
        ui.set_height(TITLE_BAR_H);

        // The whole bar is the drag handle, since dragging is the only way to
        // move an undecorated window. Registered *before* the buttons so they
        // layer on top of it and keep receiving their own clicks. A locked
        // overlay ignores the mouse entirely, so it gets no handle at all.
        if !locked {
            let bar = ui.available_rect_before_wrap();
            let response = ui.interact(bar, ui.id().with("overlay_drag"), egui::Sense::drag());
            if response.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
        }

        ui.label(
            RichText::new(short_title(title))
                .size(11.0)
                .color(Color32::from_gray(170)),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if chrome_button(ui, CLOSE_GLYPH, "Hide this overlay").clicked() {
                cfg.visible = false;
            }
            let (glyph, tip) = if cfg.minimized {
                (RESTORE_GLYPH, "Restore the overlay contents")
            } else {
                (MINIMIZE_GLYPH, "Collapse to the title bar")
            };
            if chrome_button(ui, glyph, tip).clicked() {
                cfg.minimized = !cfg.minimized;
            }
            let tip = if locked {
                "Unlock: stop passing clicks through so the overlay can be moved"
            } else {
                "Lock: freeze the overlay and pass every click through to the game"
            };
            if lock_button(ui, locked, tip).clicked() {
                cfg.locked = !cfg.locked;
            }
            content.title_extras(ui);
        });
    });
}

/// Strip the product prefix: the overlay is only ~300px wide and the user knows
/// which app opened it.
fn short_title(title: &str) -> &str {
    title.strip_prefix("RealmHound ").unwrap_or(title)
}

/// A small flat glyph button sized for the title bar.
fn chrome_button(ui: &mut egui::Ui, glyph: &str, tooltip: &str) -> egui::Response {
    let response = ui.add(
        egui::Button::new(RichText::new(glyph).size(11.0))
            .frame(false)
            .min_size(egui::vec2(16.0, TITLE_BAR_H)),
    );
    overlay_tooltip(ui, &response, tooltip);
    response
}

/// The lock toggle, drawn rather than typed.
///
/// Noto Emoji's padlocks (U+1F512 / U+1F513) are solid pictographs meant to be
/// read at emoji sizes; at the 11px the title bar uses they collapse into an
/// unreadable blob, and the open and closed ones are indistinguishable. A
/// handful of vector shapes stay crisp at the single size this is ever drawn at
/// and make the shackle's open side obvious.
fn lock_button(ui: &mut egui::Ui, locked: bool, tooltip: &str) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(16.0, TITLE_BAR_H), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let color = ui.style().interact(&response).fg_stroke.color;
        paint_padlock(ui.painter(), rect.center(), color, locked);
    }
    overlay_tooltip(ui, &response, tooltip);
    response
}

/// Paint a padlock centred on `center`: a rounded body with a shackle arc above
/// it, whose right leg is missing when `locked` is false.
fn paint_padlock(painter: &egui::Painter, center: egui::Pos2, color: Color32, locked: bool) {
    const RADIUS: f32 = 2.75;
    const LEG: f32 = 1.5;

    let body = egui::Rect::from_center_size(center + egui::vec2(0.0, 2.0), egui::vec2(9.0, 7.0));
    painter.rect_filled(body, CornerRadius::same(1), color);

    // An open shackle hinges on its attached (left) leg, so swing the arc clear
    // of the keyhole rather than just dropping the free leg.
    let hinge_x = body.center().x + if locked { 0.0 } else { 1.25 };
    let arc_y = body.top() - LEG;

    let mut shackle = vec![egui::pos2(hinge_x - RADIUS, body.top())];
    const STEPS: usize = 12;
    for step in 0..=STEPS {
        let angle = std::f32::consts::PI * (1.0 - step as f32 / STEPS as f32);
        shackle.push(egui::pos2(
            hinge_x + RADIUS * angle.cos(),
            arc_y - RADIUS * angle.sin(),
        ));
    }
    if locked {
        shackle.push(egui::pos2(hinge_x + RADIUS, body.top()));
    }
    painter.add(egui::Shape::line(
        shackle,
        egui::Stroke::new(1.3_f32, color),
    ));
}

/// Show `text` for a hovered overlay widget, anchored to the window's left edge
/// and taking the full interior width.
///
/// A tooltip cannot escape its viewport, and an overlay is only a few hundred
/// pixels wide. The default pointer-anchored placement is therefore squeezed
/// into whatever happens to be left of the cursor and wraps to one word per
/// line -- worst of all for the title-bar controls and the right-aligned damage
/// figures, which sit hard against the right edge. Placing it where the width
/// actually is keeps the whole text readable.
///
/// Built from a bare [`egui::Area`] rather than [`egui::Tooltip`] because that
/// one's area is interactable. Near the bottom of the window the tooltip is
/// constrained back up over the very widget it belongs to, and an interactable
/// area there wins the hit test: the row stops being hovered, so the tooltip
/// closes, so the row is hovered again. That is the violent flicker. An area
/// that cannot take the pointer cannot start the loop.
pub(super) fn overlay_tooltip(ui: &egui::Ui, response: &egui::Response, text: &str) {
    if !tooltip_wanted(ui, response) {
        return;
    }
    let window = ui.ctx().content_rect();
    let width = (window.width() - TOOLTIP_MARGIN * 2.0).max(MIN_W * 0.5);
    let anchor = egui::pos2(
        window.left() + TOOLTIP_MARGIN,
        response.rect.bottom() + TOOLTIP_MARGIN,
    );
    egui::Area::new(response.id.with("overlay_tooltip"))
        .order(egui::Order::Tooltip)
        .fixed_pos(anchor)
        .interactable(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(width);
                ui.label(RichText::new(text).size(11.0));
            });
        });
}

/// [`overlay_tooltip`] for text that has to be built.
///
/// Every row and tab carries hover text naming the participant and its totals.
/// Formatting those eagerly meant a handful of `String`s per row per frame for
/// something only one widget can ever show -- and a locked overlay, the normal
/// in-game state, shows none of them.
pub(super) fn overlay_tooltip_with(
    ui: &egui::Ui,
    response: &egui::Response,
    text: impl FnOnce() -> String,
) {
    if !tooltip_wanted(ui, response) {
        return;
    }
    overlay_tooltip(ui, response, &text());
}

/// Whether `response` should be showing its hover text this frame.
///
/// Not simply `Response::hovered`: the tooltip is a full-width area in the
/// `Tooltip` layer, so as soon as it is placed over the pointer -- which egui
/// does whenever the widget sits too low in the window for the text to fit
/// below it -- that area wins the hit test, the widget stops being hovered, the
/// tooltip is dropped, and the pointer is uncovered again. The result is a
/// one-frame on/off flicker on exactly the parts of a row that are not already
/// claimed by a child label. egui's own tooltips carry the same hazard and
/// answer it in `Tooltip::should_show_tooltip`; this is that rule, reduced to
/// the one case we need: an open tooltip stays open while the pointer is still
/// inside the widget that owns it.
fn tooltip_wanted(ui: &egui::Ui, response: &egui::Response) -> bool {
    if response.hovered() {
        return true;
    }
    response.is_tooltip_open()
        && ui
            .ctx()
            .pointer_hover_pos()
            .is_some_and(|pos| response.rect.contains(pos))
}

/// Bottom-right drag-to-resize corner, drawn only while the overlay is
/// unlocked (a locked overlay ignores the mouse entirely).
fn resize_grip(ui: &mut egui::Ui, locked: bool) {
    if locked {
        return;
    }
    let rect = egui::Rect::from_min_size(
        ui.max_rect().max - egui::vec2(GRIP, GRIP),
        egui::vec2(GRIP, GRIP),
    );
    let response = ui.interact(rect, ui.id().with("overlay_grip"), egui::Sense::drag());
    if response.drag_started() {
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::BeginResize(
                egui::viewport::ResizeDirection::SouthEast,
            ));
    }
    let painter = ui.painter();
    let color = Color32::from_gray(110);
    for step in 1..=3 {
        let offset = GRIP * (step as f32 / 4.0);
        painter.line_segment(
            [
                egui::pos2(rect.max.x - offset, rect.max.y),
                egui::pos2(rect.max.x, rect.max.y - offset),
            ],
            egui::Stroke::new(1.0_f32, color),
        );
    }
}

/// Convert a perceptual "how much does this hide the game" fraction into the
/// blend alpha that produces it.
///
/// egui blends in linear space, so a naive 0.9 alpha over a bright background
/// still reads as roughly half-transparent -- the slider would spend most of its
/// travel in a range that all looks the same. Pushing the requested
/// transparency through the sRGB transfer function makes the control behave the
/// way the eye expects.
fn perceptual_alpha(opacity: f32) -> f32 {
    let transparency = (1.0 - opacity).clamp(0.0, 1.0);
    let linear = if transparency <= 0.04045 {
        transparency / 12.92
    } else {
        ((transparency + 0.055) / 1.055).powf(2.4)
    };
    1.0 - linear
}

/// Shared row helper: a compact label in the overlay's monospace scale.
fn row_text(text: impl Into<String>, color: Color32) -> RichText {
    RichText::new(text.into())
        .size(12.0)
        .monospace()
        .color(color)
}

/// Format a damage figure with thousands separators, which is what makes a
/// seven-digit boss total readable at a glance.
fn thousands(value: i64) -> String {
    let negative = value < 0;
    let digits = value.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if negative {
        return format!("-{out}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        chrome_strip, perceptual_alpha, scrollbar_hit, short_title, thousands, HitRegions,
        WindowRuntime, CHROME_HIT_H, CLOSE_GLYPH, DPS_TITLE, MINIMIZE_GLYPH, RESET_GLYPH,
        RESTORE_GLYPH,
    };
    use eframe::egui;
    use realmhound_core::settings::OverlaySettings;

    #[test]
    fn reopening_an_overlay_gives_its_window_a_fresh_native_title() {
        let cfg = OverlaySettings::default().dps;
        let mut runtime = WindowRuntime::new(DPS_TITLE);

        runtime.builder_for(&cfg);
        let first = runtime.native_title.clone();
        // The same window must keep its title, or every frame would look like a
        // new generation to `dismiss_stale` and retire the live window.
        runtime.builder_for(&cfg);
        assert_eq!(runtime.native_title, first);

        runtime.closed();
        runtime.builder_for(&cfg);
        assert_ne!(
            runtime.native_title, first,
            "a replacement window must be distinguishable from the one it replaced"
        );
        // `dismiss_stale` finds leftovers by prefix, so both have to carry it.
        assert!(first.starts_with(DPS_TITLE));
        assert!(runtime.native_title.starts_with(DPS_TITLE));
    }

    #[test]
    fn perceptual_alpha_spans_the_full_range_monotonically() {
        assert_eq!(perceptual_alpha(1.0), 1.0);
        assert_eq!(perceptual_alpha(0.0), 0.0);
        let mut previous = 0.0;
        for step in 0..=20 {
            let alpha = perceptual_alpha(step as f32 / 20.0);
            assert!(alpha >= previous, "alpha must not decrease");
            previous = alpha;
        }
    }

    #[test]
    fn a_nearly_opaque_request_blends_as_nearly_opaque() {
        // The whole point: 0.9 must not land near 0.9 blend alpha, which in
        // linear space still reads as half-transparent.
        assert!(perceptual_alpha(0.9) > 0.98);
        // Mid-slider stays genuinely see-through.
        let mid = perceptual_alpha(0.5);
        assert!((0.6..0.9).contains(&mid), "unexpected midpoint {mid}");
    }

    #[test]
    fn damage_totals_get_thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(thousands(-4_200), "-4,200");
    }

    #[test]
    fn window_titles_drop_the_product_prefix() {
        assert_eq!(short_title("RealmHound DPS Overlay"), "DPS Overlay");
        // A title without the prefix is left alone.
        assert_eq!(short_title("DPS Overlay"), "DPS Overlay");
    }

    #[test]
    fn chrome_glyphs_render_in_the_bundled_fonts() {
        // Must use the fonts the app installs, not egui's defaults: the two
        // stacks disagree, and testing the wrong one is what let U+2715 and
        // U+21BA ship as tofu boxes. Segoe UI Symbol is deliberately left out
        // because it only exists on Windows.
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "NotoSans".to_owned(),
            egui::FontData::from_static(crate::app::NOTO_SANS).into(),
        );
        fonts.font_data.insert(
            "NotoEmoji".to_owned(),
            egui::FontData::from_static(crate::app::NOTO_EMOJI).into(),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            if let Some(names) = fonts.families.get_mut(&family) {
                names.clear();
                names.push("NotoSans".to_owned());
                names.push("NotoEmoji".to_owned());
            }
        }

        let ctx = egui::Context::default();
        ctx.set_fonts(fonts);
        let _ = ctx.run(egui::RawInput::default(), |_| {});

        let font = egui::FontId::proportional(11.0);
        ctx.fonts_mut(|fonts| {
            // The lock toggle is painted, not typed, so it is not listed here.
            for glyph in [CLOSE_GLYPH, MINIMIZE_GLYPH, RESTORE_GLYPH, RESET_GLYPH] {
                assert!(
                    fonts.has_glyphs(&font, glyph),
                    "{glyph:?} has no glyph and would render as a tofu box"
                );
            }
        });
    }

    #[test]
    fn the_chrome_strip_covers_the_title_bar_and_nothing_below_it() {
        // This band is what a locked overlay tests the OS pointer against to
        // decide whether to suspend click-through, so it must line up with the
        // buttons and stop before the content.
        let window = egui::Rect::from_min_size(egui::pos2(100.0, 200.0), egui::vec2(320.0, 240.0));
        let strip = chrome_strip(window);

        assert_eq!(strip.min, window.min);
        assert_eq!(strip.max.x, window.max.x);
        assert!(strip.height() < window.height());

        // A point on the button row is inside; the content below is not.
        assert!(strip.contains(egui::pos2(410.0, 205.0)));
        assert!(!strip.contains(egui::pos2(410.0, 200.0 + CHROME_HIT_H + 1.0)));
        // Neither is anything outside the window horizontally.
        assert!(!strip.contains(egui::pos2(99.0, 205.0)));
    }

    #[test]
    fn the_scrollbar_hit_band_hugs_the_gutter_and_only_appears_when_needed() {
        // This band is what lets a locked overlay be scrolled without being
        // unlocked, so it must hug the scrollbar and leave the rest of the body
        // clicking through to the game.
        let inner = egui::Rect::from_min_size(egui::pos2(0.0, 30.0), egui::vec2(320.0, 200.0));
        let output = |content_h: f32| egui::scroll_area::ScrollAreaOutput {
            inner: (),
            id: egui::Id::new("test"),
            state: egui::scroll_area::State::default(),
            content_size: egui::vec2(320.0, content_h),
            inner_rect: inner,
        };

        assert!(scrollbar_hit(&output(150.0)).is_none(), "content fits");

        let band = scrollbar_hit(&output(600.0)).expect("overflowing content has a bar");
        assert!(band.width() < inner.width() / 2.0);
        assert_eq!(band.min.y, inner.min.y);
        // The gutter is inside; the rows themselves keep passing clicks through.
        assert!(band.contains(egui::pos2(inner.max.x - 2.0, 100.0)));
        assert!(!band.contains(egui::pos2(inner.center().x, 100.0)));
        assert!(!band.contains(egui::pos2(inner.max.x - 2.0, 10.0)));
    }

    #[test]
    fn hit_regions_ignore_empty_rects() {
        let mut hits = HitRegions::default();
        hits.add(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::Vec2::ZERO,
        ));
        assert!(!hits.contains(egui::Pos2::ZERO));

        hits.add(egui::Rect::from_min_size(
            egui::pos2(10.0, 10.0),
            egui::vec2(20.0, 20.0),
        ));
        assert!(hits.contains(egui::pos2(15.0, 15.0)));
        assert!(!hits.contains(egui::pos2(5.0, 15.0)));
    }
}

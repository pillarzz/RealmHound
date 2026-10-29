//! macOS overlay promotion via AppKit.
//!
//! `NSWindowCollectionBehaviorCanJoinAllSpaces | FullScreenAuxiliary` is what
//! lets the overlay appear on the private Space a game in native Full Screen
//! mode owns, and `NSScreenSaverWindowLevel` (1000) keeps it above the game's
//! own chrome. Neither is reachable through winit.
//!
//! All AppKit window mutation must happen on the AppKit main thread. eframe's
//! update callback runs inside the winit event loop, which winit pins to the
//! main thread on macOS, so the [`MainThreadMarker::new`] check below always
//! succeeds from the UI thread and no `libdispatch` hop is needed.

use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSApplication, NSEvent, NSScreen, NSWindow, NSWindowCollectionBehavior, NSWindowOcclusionState,
};

/// `NSScreenSaverWindowLevel`: the highest practical level for a game overlay.
const SCREEN_SAVER_WINDOW_LEVEL: isize = 1000;

/// Run `f` for every on-screen `NSWindow` whose title matches `title`.
fn for_each_matching(title: &str, mut f: impl FnMut(&NSWindow)) {
    let Some(mtm) = MainThreadMarker::new() else {
        tracing::debug!("[OVERLAY] skipping AppKit promotion off the main thread");
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    for window in app.windows().iter() {
        if window.isVisible() && window.title().to_string() == title {
            f(&window);
        }
    }
}

pub(super) fn promote(title: &str) {
    // `Transient` and `Stationary` are the same mutually exclusive group:
    // "You may specify at most one of Managed, Transient, or Stationary"
    // (AppKit's NSWindow.h). Setting both left the window server with an
    // undefined Spaces participation, which only bites over a full-screen
    // Space -- it composited a cached copy of an older frame under the live
    // surface there, while the same overlay over the desktop was clean.
    // `Transient` is also the default for a non-normal window level, so this
    // is the conservative half of the pair to keep.
    let behavior = NSWindowCollectionBehavior::CanJoinAllSpaces
        | NSWindowCollectionBehavior::FullScreenAuxiliary
        | NSWindowCollectionBehavior::Transient
        | NSWindowCollectionBehavior::IgnoresCycle;
    for_each_matching(title, |window| {
        if window.collectionBehavior() != behavior {
            window.setCollectionBehavior(behavior);
        }
        if window.level() != SCREEN_SAVER_WINDOW_LEVEL {
            window.setLevel(SCREEN_SAVER_WINDOW_LEVEL);
        }
        // Panels default to hiding when their app deactivates, which would make
        // the overlay vanish the moment the game takes focus.
        if window.hidesOnDeactivate() {
            window.setHidesOnDeactivate(false);
        }
        // A borderless window's shadow is derived from the alpha mask of its
        // content and then cached by the window server, which has to be told to
        // recompute it every time that shape changes -- the classic source of
        // smeared leftovers around a transparent window. An overlay over a game
        // wants no shadow at all, so drop the mechanism rather than feed it.
        if window.hasShadow() {
            window.setHasShadow(false);
        }
    });
}

/// Raise the overlay if it has just gone from visible to covered, and report
/// its current state so the caller can feed it back next time.
///
/// The edge is the point. `orderFrontRegardless` re-registers a transparent GL
/// window with the window server, and a window sharing another app's
/// full-screen Space reports *not* `Visible` for as long as that Space is up --
/// so the previous "only raise what is covered" rule fired on every one of the
/// promote heartbeats for the entire time the game was running, and never once
/// while the overlay sat over the desktop. That is exactly the split the ghost
/// frame follows.
pub(super) fn order_front(title: &str, was_hidden: bool) -> bool {
    let mut hidden = was_hidden;
    for_each_matching(title, |window| {
        hidden = !window
            .occlusionState()
            .contains(NSWindowOcclusionState::Visible);
        if !hidden || was_hidden {
            return;
        }
        // Raises without activating and, unlike winit's focus request,
        // preserves the promoted window level.
        window.orderFrontRegardless();
    });
    hidden
}

/// Order out every on-screen window titled `prefix...` other than `keep`.
///
/// `NSApplication::windows` lists windows AppKit has closed but not yet
/// deallocated, and one of those that is still on screen keeps compositing its
/// last frame forever. Because the live overlay window carries a unique
/// generation suffix, anything else under the same prefix is exactly such a
/// leftover and can be retired on sight.
pub(super) fn dismiss_stale(prefix: &str, keep: &str) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    for window in app.windows().iter() {
        let title = window.title().to_string();
        if title == keep || !title.starts_with(prefix) || !window.isVisible() {
            continue;
        }
        if keep.is_empty() {
            tracing::debug!("[OVERLAY] closing overlay window {title:?}");
        } else {
            // A second window under the same prefix means the previous one
            // outlived the viewport that owned it.
            tracing::info!("[OVERLAY] retiring stale overlay window {title:?}");
        }
        window.orderOut(None);
    }
}

pub(super) fn cursor_position() -> Option<(f32, f32)> {
    let mtm = MainThreadMarker::new()?;
    // AppKit reports screen coordinates in points already, but with the origin
    // at the bottom-left of the *primary* display; winit (and so egui's window
    // geometry) uses a top-left origin.
    let primary = NSScreen::screens(mtm).firstObject()?.frame();
    let point = NSEvent::mouseLocation();
    Some((point.x as f32, (primary.size.height - point.y) as f32))
}

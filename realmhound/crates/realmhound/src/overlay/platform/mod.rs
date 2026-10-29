//! Native window tweaks the cross-platform windowing layer cannot express.
//!
//! egui/winit already give us an undecorated, transparent, always-on-top
//! viewport with mouse pass-through, which is enough for a windowed or
//! borderless-fullscreen game on Windows. Two things still need the native
//! APIs:
//!
//! - **macOS**: winit's always-on-top maps to `NSFloatingWindowLevel`, which is
//!   *below* a native "Full Screen" Space. The overlay has to be promoted to
//!   `NSScreenSaverWindowLevel` with a `CanJoinAllSpaces | FullScreenAuxiliary`
//!   collection behavior to show over a fullscreen game client.
//! - **Windows**: the overlay must never steal focus from the game nor appear in
//!   Alt-Tab, which means `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW` on the extended
//!   style. That same style stops the system move loop winit drags with, so
//!   Windows moves and resizes overlays itself (see [`begin_drag`]).
//! - **Linux (X11/XWayland)**: the window manager has to be told to keep the
//!   overlay above a fullscreen Proton client and out of the taskbar, and a
//!   click-through overlay needs the X server for the pointer position.
//!
//! Windows are located by their title, exactly as the viewport sets it, so no
//! handle plumbing through eframe is needed. That title carries a per-window
//! generation suffix precisely so the lookup can never land on a window the
//! windowing layer has already abandoned; see [`dismiss_stale`].
//!
//! Every entry point is best-effort and infallible: a platform without an
//! implementation (native Wayland, BSD) runs the stubs below and the overlay
//! still works as a normal always-on-top window.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::{event_loop_hook, is_forced_x11_failure, relaunch_on_wayland};

/// What a title-bar or grip drag does to the overlay window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragKind {
    Move,
    /// Resize from the bottom-right corner.
    Resize,
}

/// Apply the always-on-top / all-Spaces / never-steal-focus promotion to the
/// overlay window titled `title`.
///
/// Idempotent, and safe to call before the window exists (it simply finds
/// nothing). Must be called from the UI thread, which on macOS is also the
/// AppKit main thread.
pub fn promote(title: &str) {
    #[cfg(target_os = "macos")]
    macos::promote(title);
    #[cfg(windows)]
    windows::promote(title);
    #[cfg(target_os = "linux")]
    linux::promote(title);
    #[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
    {
        // BSD: winit's always-on-top window level is all we can portably ask
        // for.
        let _ = title;
    }
}

/// Raise the overlay above its same-level peers without changing its level and
/// without taking keyboard focus.
///
/// Edge-triggered: pass back the value returned last time, and the raise only
/// happens on the transition from on-screen to covered. Re-asserting the front
/// position of a transparent window every heartbeat is not free on macOS -- see
/// [`macos::order_front`].
pub fn order_front(title: &str, was_hidden: bool) -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::order_front(title, was_hidden)
    }
    #[cfg(windows)]
    {
        // `SetWindowPos(HWND_TOPMOST)` is a cheap no-op on a window that is
        // already topmost, and other apps claim topmost often enough that it
        // has to be re-asserted unconditionally.
        let _ = was_hidden;
        windows::order_front(title);
        false
    }
    #[cfg(target_os = "linux")]
    {
        // Like Windows: a window manager drops "above" order whenever a
        // fullscreen client takes focus, and a restack request is cheap.
        let _ = was_hidden;
        linux::order_front(title);
        false
    }
    #[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
    {
        let _ = (title, was_hidden);
        false
    }
}

/// Take every on-screen window whose title starts with `prefix` off screen,
/// except the one titled `keep`.
///
/// Each overlay window is opened with a unique generation suffix, so anything
/// else sharing the prefix is a leftover from a window eframe has already
/// replaced or discarded. On macOS those leftovers can linger on screen, where
/// they show through the live overlay's translucent backdrop as a frozen frame
/// from a previous instance. Pass an empty `keep` to retire all of them.
pub fn dismiss_stale(prefix: &str, keep: &str) {
    #[cfg(target_os = "macos")]
    macos::dismiss_stale(prefix, keep);
    #[cfg(not(target_os = "macos"))]
    {
        // Windows destroys an `HWND` synchronously, and an X11/Wayland window
        // goes with its surface, so no other platform can strand one.
        let _ = (prefix, keep);
    }
}

/// Pointer position in logical screen points with a top-left origin, matching
/// [`egui::ViewportInfo::outer_rect`].
///
/// A click-through window receives no mouse events at all, so egui cannot say
/// when the pointer is over the overlay's own title bar. Asking the OS directly
/// is what lets a locked overlay keep its buttons usable.
///
/// `pixels_per_point` converts platforms that report physical pixels; it is
/// unused where the OS already reports points.
pub fn cursor_position(pixels_per_point: f32) -> Option<(f32, f32)> {
    #[cfg(target_os = "macos")]
    {
        let _ = pixels_per_point;
        macos::cursor_position()
    }
    #[cfg(windows)]
    {
        windows::cursor_position(pixels_per_point)
    }
    #[cfg(target_os = "linux")]
    {
        linux::cursor_position(pixels_per_point)
    }
    #[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
    {
        let _ = pixels_per_point;
        None
    }
}

/// Start moving or resizing the overlay under the pointer natively.
///
/// Returns `false` when the platform leaves it to the windowing layer, in
/// which case the caller sends `StartDrag` / `BeginResize` as usual. Once this
/// returns `true`, call [`continue_drag`] every frame until it returns `false`.
/// `min_w` / `min_h` bound a resize, in logical points.
pub fn begin_drag(kind: DragKind, min_w: f32, min_h: f32, pixels_per_point: f32) -> bool {
    #[cfg(windows)]
    {
        // Win32 works in physical pixels.
        let scale = pixels_per_point.max(1.0);
        windows::begin_drag(kind, min_w * scale, min_h * scale)
    }
    #[cfg(not(windows))]
    {
        let _ = (kind, min_w, min_h, pixels_per_point);
        false
    }
}

/// Track the pointer for a drag [`begin_drag`] started. Returns `true` while
/// the drag is still going, so the caller keeps frames coming.
pub fn continue_drag() -> bool {
    #[cfg(windows)]
    {
        windows::continue_drag()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Whether this platform has a native overlay implementation. Surfaced in the
/// settings UI so users on native Wayland (or BSD) know the overlay is a plain
/// window there.
pub fn is_supported() -> bool {
    #[cfg(target_os = "linux")]
    {
        linux::uses_x11()
    }
    #[cfg(not(target_os = "linux"))]
    {
        cfg!(any(target_os = "macos", windows))
    }
}

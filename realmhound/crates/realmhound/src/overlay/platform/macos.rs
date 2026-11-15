//! macOS overlay promotion via AppKit.
//!
//! AppKit requests all-Spaces/fullscreen-auxiliary participation and an elevated
//! window level, which winit cannot express. Native Full Screen behavior remains
//! unverified; windowed or borderless fullscreen is the recommended game mode.
//!
//! All AppKit window mutation must happen on the AppKit main thread. eframe's
//! update callback runs inside the winit event loop, which winit pins to the
//! main thread on macOS, so the [`MainThreadMarker::new`] check below always
//! succeeds from the UI thread and no `libdispatch` hop is needed.

use libproc::{
    bsd_info::BSDInfo,
    proc_pid::pidinfo,
    processes::{pids_by_type, ProcFilter},
};
use objc2::{rc::Retained, runtime::AnyObject, MainThreadMarker};
use objc2_app_kit::{
    NSApplication, NSEvent, NSScreen, NSWindow, NSWindowCollectionBehavior, NSWindowOcclusionState,
    NSWorkspace,
};
use objc2_foundation::{ns_string, NSArray, NSDictionary, NSNumber, NSString};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWindowListCopyWindowInfo(
        options: u32,
        relative_to: u32,
    ) -> *mut NSArray<NSDictionary<NSString, AnyObject>>;
}

fn game_windows() -> Result<Retained<NSArray<NSDictionary<NSString, AnyObject>>>, String> {
    // SAFETY: option 0 lists all windows, including minimized ones. The copied
    // CFArray of CFDictionaries is toll-free bridged; Retained owns its +1.
    unsafe { Retained::from_raw(CGWindowListCopyWindowInfo(0, 0)) }
        .ok_or_else(|| "Cannot inspect macOS game windows in this login session.".to_string())
}

fn number(
    info: &NSDictionary<NSString, AnyObject>,
    key: &NSString,
) -> Result<Retained<NSNumber>, String> {
    info.objectForKey(key)
        .and_then(|value| value.downcast::<NSNumber>().ok())
        .ok_or_else(|| format!("Cannot inspect linked macOS window metadata: {key}."))
}

fn window_visible(info: &NSDictionary<NSString, AnyObject>) -> Result<bool, String> {
    Ok(number(info, ns_string!("kCGWindowLayer"))?.as_isize() == 0
        && number(info, ns_string!("kCGWindowIsOnscreen"))?.as_bool()
        && number(info, ns_string!("kCGWindowAlpha"))?.as_f64() > 0.0)
}

/// Window-server metadata avoids Accessibility and window-title permissions.
/// Multiple visible normal-level windows are ambiguous, so fail closed.
pub(crate) fn game_window(pid: u32) -> Result<Option<isize>, String> {
    let mut windows = Vec::new();
    for info in game_windows()?.iter() {
        if number(&info, ns_string!("kCGWindowOwnerPID"))?.as_isize() != pid as isize
            || !window_visible(&info)?
        {
            continue;
        }
        windows.push(number(&info, ns_string!("kCGWindowNumber"))?.as_isize());
    }
    match windows.as_slice() {
        [] => Ok(None),
        [window] => Ok(Some(*window)),
        _ => {
            Err("Multiple windows belong to the linked client; overlay remains hidden.".to_string())
        }
    }
}

pub(crate) fn game_window_active(window: isize, pid: u32) -> Result<Option<bool>, String> {
    for info in game_windows()?.iter() {
        if number(&info, ns_string!("kCGWindowNumber"))?.as_isize() != window {
            continue;
        }
        if number(&info, ns_string!("kCGWindowOwnerPID"))?.as_isize() != pid as isize {
            return Ok(None);
        }
        // Minimized/hidden windows and windows on an inactive Space are not
        // on screen. Names and pixels are intentionally never requested.
        if !window_visible(&info)? {
            return Ok(Some(false));
        }
        let foreground = NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .ok_or_else(|| "Cannot inspect the foreground macOS application.".to_string())?;
        let foreground_pid = foreground.processIdentifier() as u32;
        return Ok(Some(
            foreground_pid == pid || foreground_pid == std::process::id(),
        ));
    }
    Ok(None)
}

/// libproc creation timestamps prevent PID reuse from replacing the client.
pub(crate) struct Process {
    pid: u32,
    start: (u64, u64),
}

impl Process {
    pub fn open(pid: u32) -> Result<Self, String> {
        let info = pidinfo::<BSDInfo>(pid as i32, 0)
            .map_err(|error| format!("Cannot inspect linked macOS process {pid}: {error}"))?;
        Ok(Self {
            pid,
            start: (info.pbi_start_tvsec, info.pbi_start_tvusec),
        })
    }

    pub fn is_alive(&self) -> Result<bool, String> {
        const SZOMB: u32 = 5; // sys/proc.h.
        match pidinfo::<BSDInfo>(self.pid as i32, 0) {
            Ok(info) => Ok(self.start == (info.pbi_start_tvsec, info.pbi_start_tvusec)
                && info.pbi_status != SZOMB),
            Err(error) => {
                let pids = pids_by_type(ProcFilter::All).map_err(|scan| {
                    format!("Cannot check linked macOS process lifetime: {scan}")
                })?;
                if pids.contains(&self.pid) {
                    Err(format!(
                        "Cannot inspect linked macOS process {}: {error}",
                        self.pid
                    ))
                } else {
                    Ok(false)
                }
            }
        }
    }
}

/// `NSScreenSaverWindowLevel`: the highest practical level for a game overlay.
const SCREEN_SAVER_WINDOW_LEVEL: isize = 1000;

/// Run `f` for every matching window, including one created hidden.
fn for_each_matching(title: &str, mut f: impl FnMut(&NSWindow)) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        tracing::debug!("[OVERLAY] skipping AppKit promotion off the main thread");
        return false;
    };
    let app = NSApplication::sharedApplication(mtm);
    let mut found = false;
    for window in app.windows().iter() {
        if window.title().to_string() == title {
            f(&window);
            found = true;
        }
    }
    found
}

pub(super) fn promote(title: &str) -> bool {
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
    let mut prepared = true;
    let found = for_each_matching(title, |window| {
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
        prepared &= window.collectionBehavior() == behavior
            && window.level() == SCREEN_SAVER_WINDOW_LEVEL
            && !window.hidesOnDeactivate()
            && !window.hasShadow();
    });
    found && prepared
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
        if !window.isVisible() {
            return;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_server_visibility_requires_visible_normal_window_metadata() {
        for (onscreen, alpha, layer, expected) in [
            (true, 1.0, 0, true),
            (false, 1.0, 0, false),
            (true, 0.0, 0, false),
            (true, 1.0, 1, false),
        ] {
            let onscreen = NSNumber::new_bool(onscreen);
            let alpha = NSNumber::new_f64(alpha);
            let layer = NSNumber::new_isize(layer);
            let info: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(
                &[
                    ns_string!("kCGWindowIsOnscreen"),
                    ns_string!("kCGWindowAlpha"),
                    ns_string!("kCGWindowLayer"),
                ],
                &[&*onscreen, &*alpha, &*layer],
            );
            assert_eq!(window_visible(&info), Ok(expected));
        }
        let missing = NSDictionary::<NSString, AnyObject>::new();
        assert!(window_visible(&missing).is_err());
        let invalid: Retained<NSDictionary<NSString, AnyObject>> =
            NSDictionary::from_slices(&[ns_string!("kCGWindowLayer")], &[ns_string!("unknown")]);
        assert!(window_visible(&invalid).is_err());
    }
}

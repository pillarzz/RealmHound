//! Windows overlay chrome.
//!
//! winit's always-on-top already applies `WS_EX_TOPMOST`, which is enough to
//! float over a windowed or borderless-fullscreen game (an exclusive-fullscreen
//! DirectX client cannot be overlaid from user mode at all). What winit does not
//! do is stop the overlay from taking focus and appearing in Alt-Tab, which is
//! what makes an overlay feel broken: one stray click and the game loses input.
//!
//! `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW` fixes both. Click-through itself stays
//! with egui's `ViewportCommand::MousePassthrough`, which applies
//! `WS_EX_LAYERED | WS_EX_TRANSPARENT` for us.

use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetCursorPos, GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE,
    HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

/// Logged once so a missing overlay window doesn't spam the log every frame.
static MISSING_LOGGED: AtomicBool = AtomicBool::new(false);

/// Locate the top-level window with exactly this title.
fn find(title: &str) -> Option<HWND> {
    let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 buffer that outlives the call.
    let hwnd = unsafe { FindWindowW(std::ptr::null(), wide.as_ptr()) };
    if hwnd == 0 {
        if !MISSING_LOGGED.swap(true, Ordering::Relaxed) {
            tracing::debug!("[OVERLAY] no window titled {title:?} yet");
        }
        return None;
    }
    MISSING_LOGGED.store(false, Ordering::Relaxed);
    Some(hwnd)
}

pub(super) fn promote(title: &str) {
    let Some(hwnd) = find(title) else {
        return;
    };
    let wanted = (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW) as isize;
    // SAFETY: `hwnd` came from `FindWindowW` and is only read/written through
    // the documented extended-style index.
    unsafe {
        let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        if current & wanted != wanted {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, current | wanted);
        }
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

pub(super) fn order_front(title: &str) {
    let Some(hwnd) = find(title) else {
        return;
    };
    // `SWP_NOACTIVATE` is what separates this from a focus request: the overlay
    // is raised but the game keeps keyboard input.
    // SAFETY: `hwnd` came from `FindWindowW`; the call only reorders the window.
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

pub(super) fn cursor_position(pixels_per_point: f32) -> Option<(f32, f32)> {
    let mut point = POINT { x: 0, y: 0 };
    // SAFETY: `point` is a valid, writable POINT for the duration of the call.
    if unsafe { GetCursorPos(&mut point) } == 0 {
        return None;
    }
    // Win32 reports physical pixels; window geometry is in logical points.
    let scale = if pixels_per_point > 0.0 {
        pixels_per_point
    } else {
        1.0
    };
    Some((point.x as f32 / scale, point.y as f32 / scale))
}

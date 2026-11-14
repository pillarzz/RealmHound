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

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetAncestor, GetCursorPos, GetLayeredWindowAttributes, GetSystemMetrics,
    GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, SetLayeredWindowAttributes,
    SetWindowLongPtrW, SetWindowPos, WindowFromPoint, GA_ROOT, GWL_EXSTYLE, HWND_TOPMOST,
    LAYERED_WINDOW_ATTRIBUTES_FLAGS, SM_SWAPBUTTON, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

use super::DragKind;

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
    // the documented extended-style index and layered-window calls.
    unsafe {
        let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        if current & wanted != wanted {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, current | wanted);
        }
        if current & WS_EX_LAYERED as isize != 0 {
            init_layered(hwnd);
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

/// Give a freshly layered window its layered attributes, keeping any it
/// already has.
///
/// `GetLayeredWindowAttributes` only succeeds once attributes have been set,
/// and winit drops them every time it rewrites the extended style. Setting
/// neither `LWA_COLORKEY` nor `LWA_ALPHA` keeps the window composited from the
/// GL surface's own alpha, which is what the opacity slider drives.
///
/// # Safety
/// `hwnd` must be a live window handle.
unsafe fn init_layered(hwnd: HWND) {
    let mut key = 0;
    let mut alpha = 0;
    let mut flags: LAYERED_WINDOW_ATTRIBUTES_FLAGS = 0;
    // SAFETY: the out-pointers are valid locals; `hwnd` is live per the caller.
    unsafe {
        if GetLayeredWindowAttributes(hwnd, &mut key, &mut alpha, &mut flags) == 0 {
            SetLayeredWindowAttributes(hwnd, 0, 0, 0);
        }
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

/// An in-flight move or resize, in physical pixels.
#[derive(Clone, Copy)]
struct Drag {
    hwnd: HWND,
    kind: DragKind,
    cursor: POINT,
    rect: RECT,
    min_w: i32,
    min_h: i32,
}

thread_local! {
    /// The overlay currently being dragged. The mouse can only drag one window
    /// at a time, and every overlay call runs on the UI thread.
    static DRAG: Cell<Option<Drag>> = const { Cell::new(None) };
}

/// Whether the button the user drags with is physically held. Asked of the OS
/// directly, so a release outside the window still ends the drag.
fn primary_button_down() -> bool {
    // GetAsyncKeyState reports the *physical* buttons, so honour a left-handed
    // swap.
    // SAFETY: both calls only read global input state.
    unsafe {
        let key = if GetSystemMetrics(SM_SWAPBUTTON) != 0 {
            VK_RBUTTON
        } else {
            VK_LBUTTON
        };
        (GetAsyncKeyState(i32::from(key)) as u16) & 0x8000 != 0
    }
}

pub(super) fn begin_drag(kind: DragKind, min_w: f32, min_h: f32) -> bool {
    let mut cursor = POINT { x: 0, y: 0 };
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: every out-pointer is a valid local; the handles come straight
    // from the OS and are only queried.
    let hwnd = unsafe {
        if GetCursorPos(&mut cursor) == 0 {
            return false;
        }
        // The press that started the drag landed on the overlay itself (it is
        // unlocked, so not click-through), so it is the window under the cursor.
        let hwnd = GetAncestor(WindowFromPoint(cursor), GA_ROOT);
        let mut pid = 0;
        if hwnd == 0
            || GetWindowThreadProcessId(hwnd, &mut pid) == 0
            || pid != GetCurrentProcessId()
            || GetWindowRect(hwnd, &mut rect) == 0
        {
            return false;
        }
        hwnd
    };
    DRAG.with(|drag| {
        drag.set(Some(Drag {
            hwnd,
            kind,
            cursor,
            rect,
            min_w: min_w.ceil() as i32,
            min_h: min_h.ceil() as i32,
        }));
    });
    true
}

pub(super) fn continue_drag() -> bool {
    let Some(drag) = DRAG.with(Cell::get) else {
        return false;
    };
    let mut cursor = POINT { x: 0, y: 0 };
    // SAFETY: `cursor` is a valid local for the duration of the call.
    if !primary_button_down() || unsafe { GetCursorPos(&mut cursor) } == 0 {
        DRAG.with(|d| d.set(None));
        return false;
    }
    let dx = cursor.x - drag.cursor.x;
    let dy = cursor.y - drag.cursor.y;
    let (x, y, w, h, flags) = match drag.kind {
        DragKind::Move => (drag.rect.left + dx, drag.rect.top + dy, 0, 0, SWP_NOSIZE),
        DragKind::Resize => (
            0,
            0,
            (drag.rect.right - drag.rect.left + dx).max(drag.min_w),
            (drag.rect.bottom - drag.rect.top + dy).max(drag.min_h),
            SWP_NOMOVE,
        ),
    };
    // SAFETY: `hwnd` is our own window; a destroyed one just fails the call.
    unsafe {
        SetWindowPos(
            drag.hwnd,
            0,
            x,
            y,
            w,
            h,
            flags | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    true
}

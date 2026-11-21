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

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::{
    Dwm::{
        DwmEnableBlurBehindWindow, DwmGetWindowAttribute, DWMWA_CLOAKED, DWM_BB_BLURREGION,
        DWM_BB_ENABLE, DWM_BLURBEHIND,
    },
    Gdi::{CreateRectRgn, DeleteObject},
};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowW, GetAncestor, GetCursorPos, GetForegroundWindow, GetSystemMetrics,
    GetWindow, GetWindowLongPtrW, GetWindowLongW, GetWindowRect, GetWindowThreadProcessId,
    IsIconic, IsWindowVisible, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos,
    WindowFromPoint, GA_ROOT, GWL_EXSTYLE, GW_OWNER, HWND_TOPMOST, LWA_ALPHA, SM_SWAPBUTTON,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW,
};

use super::DragKind;

/// Logged once so a missing overlay window doesn't spam the log every frame.
static MISSING_LOGGED: AtomicBool = AtomicBool::new(false);
static TRANSPARENCY_WARNED: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// Generation title -> initialized handle and layered state.
    static COMPOSITOR: RefCell<HashMap<String, (HWND, bool)>> = RefCell::new(HashMap::new());
}

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

pub(super) fn promote(title: &str) -> bool {
    let Some(hwnd) = find(title) else {
        return false;
    };
    let wanted = (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW) as isize;
    // SAFETY: `hwnd` came from `FindWindowW` and is only read/written through
    // the documented extended-style index and compositor calls.
    unsafe {
        let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        if current & wanted != wanted {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, current | wanted);
        }
        if GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & wanted != wanted
            || !ensure_transparency(title, hwnd, current & WS_EX_LAYERED as isize != 0)
        {
            return false;
        }
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        ) != 0
    }
}

/// Apply the compositor setup winit performs for a transparent window.
///
/// glutin can strip transparency from secondary Windows viewports
/// (emilk/egui#3632), and winit rewrites the extended style whenever the
/// mouse-passthrough flag changes. Either leaves the overlay composited as an
/// opaque rectangle, so every new `HWND` -- and every layered/unlayered change
/// -- gets DWM composition re-applied.
///
/// `layered` mirrors `WS_EX_LAYERED`: click-through adds it, and a layered
/// window stays invisible until its alpha is initialized.
fn ensure_transparency(title: &str, hwnd: HWND, layered: bool) -> bool {
    let needs_setup = COMPOSITOR.with(|seen| seen.borrow().get(title) != Some(&(hwnd, layered)));
    if !needs_setup {
        return true;
    }
    // SAFETY: `hwnd` came from `FindWindowW` and is only passed to the
    // documented DWM/GDI/layered-window entry points.
    match unsafe { enable_transparency(hwnd, layered) } {
        Ok(()) => {
            COMPOSITOR.with(|seen| {
                seen.borrow_mut().insert(title.to_owned(), (hwnd, layered));
            });
            TRANSPARENCY_WARNED.store(false, Ordering::Relaxed);
            true
        }
        Err(error) => {
            if !TRANSPARENCY_WARNED.swap(true, Ordering::Relaxed) {
                tracing::warn!("[OVERLAY] {error}");
            }
            false
        }
    }
}

pub(super) fn dismiss_stale(prefix: &str, keep: &str) {
    COMPOSITOR.with(|seen| {
        seen.borrow_mut()
            .retain(|title, _| !title.starts_with(prefix) || title == keep);
    });
}

/// Give the window DWM composition with a full-window blur region, then
/// initialize its layered alpha when click-through has made it layered.
///
/// Per-pixel opacity still comes from the GL surface; `255` is the neutral
/// global multiplier, so the opacity slider keeps working.
///
/// # Safety
/// `hwnd` must be a live window handle.
unsafe fn enable_transparency(hwnd: HWND, layered: bool) -> Result<(), String> {
    // SAFETY: `region` is created here and released below.
    let region = unsafe { CreateRectRgn(0, 0, -1, -1) };
    if region == 0 {
        return Err(format!(
            "Cannot create overlay transparency region: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: `hwnd` is live per the caller; `region` is a valid full-window
    // region that DWM copies during this call.
    let result = unsafe {
        DwmEnableBlurBehindWindow(
            hwnd,
            &DWM_BLURBEHIND {
                dwFlags: DWM_BB_ENABLE | DWM_BB_BLURREGION,
                fEnable: 1,
                hRgnBlur: region,
                fTransitionOnMaximized: 0,
            },
        )
    };
    // SAFETY: `region` is no longer used after the DWM call.
    let deleted = unsafe { DeleteObject(region) };
    if result < 0 {
        return Err(format!(
            "Windows could not enable overlay transparency: HRESULT 0x{:08X}",
            result as u32
        ));
    }
    if deleted == 0 {
        return Err(format!(
            "Cannot release overlay transparency region: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: `hwnd` is live and already carries `WS_EX_LAYERED`.
    if layered && unsafe { SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA) } == 0 {
        return Err(format!(
            "Cannot initialize overlay layered-window alpha: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
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
    // Win32 reports physical pixels; egui viewport geometry uses UI points.
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

/// Locate the visible top-level game window owned by `pid`.
///
/// Ownership is checked by process id rather than by window title so a
/// renamed client or a launcher window cannot be mistaken for the game.
pub(crate) fn game_window(pid: u32) -> Result<Option<HWND>, String> {
    struct Search {
        pid: u32,
        windows: Vec<HWND>,
    }
    unsafe extern "system" fn find(window: HWND, param: LPARAM) -> BOOL {
        let search = &mut *(param as *mut Search);
        let mut owner = 0;
        GetWindowThreadProcessId(window, &mut owner);
        if owner == search.pid
            && IsWindowVisible(window) != 0
            && GetWindow(window, GW_OWNER) == 0
            && GetWindowLongW(window, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW == 0
        {
            search.windows.push(window);
        }
        1
    }

    let mut search = Search {
        pid,
        windows: Vec::new(),
    };
    // SAFETY: `find` only reads each enumerated window; the callback data is
    // a valid local for the duration of the call.
    if unsafe { EnumWindows(Some(find), &mut search as *mut Search as LPARAM) } == 0 {
        return Err(format!(
            "Cannot enumerate the linked game's windows: {}",
            std::io::Error::last_os_error()
        ));
    }
    match search.windows.as_slice() {
        [] => Ok(None),
        [window] => Ok(Some(*window)),
        _ => {
            Err("Multiple windows belong to the linked client; overlay remains hidden.".to_string())
        }
    }
}

/// Whether `window` still belongs to `pid` and is visible and unminimized,
/// with either the linked game process or RealmHound foreground.
///
/// Returns `Ok(None)` when the window disappeared or stopped belonging to the
/// process, so the caller can look for a replacement.
pub(crate) fn game_window_active(window: HWND, pid: u32) -> Result<Option<bool>, String> {
    let mut owner = 0;
    // SAFETY: `window` is a handle the caller obtained from `game_window`.
    unsafe {
        GetWindowThreadProcessId(window, &mut owner);
    }
    if owner != pid || unsafe { IsWindowVisible(window) } == 0 {
        return Ok(None);
    }
    let mut cloaked: u32 = 0;
    // SAFETY: `cloaked` is a valid out-pointer; DWM only reads the attribute.
    let result = unsafe {
        DwmGetWindowAttribute(
            window,
            DWMWA_CLOAKED as u32,
            &mut cloaked as *mut u32 as *mut _,
            4,
        )
    };
    if result < 0 {
        return Err(format!(
            "Cannot inspect linked game visibility: HRESULT 0x{:08X}",
            result as u32
        ));
    }
    // A window belonging to a virtual desktop or a suspended UWP app stays
    // "visible" to the window manager but is not actually on screen.
    let foreground = unsafe { GetForegroundWindow() };
    let mut foreground_pid = 0;
    unsafe { GetWindowThreadProcessId(foreground, &mut foreground_pid) };
    let active = unsafe { IsIconic(window) == 0 }
        && cloaked == 0
        && (foreground_pid == pid || foreground_pid == std::process::id());
    Ok(Some(active))
}

/// A retained handle on the game client process.
///
/// Holding the handle is what makes "is this still the same process?" reliable:
/// a recycled PID cannot masquerade as the client we associated with.
pub(crate) struct Process {
    handle: isize,
}

impl Process {
    pub fn open(pid: u32) -> Result<Self, String> {
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        };
        // SAFETY: opening a process by PID only returns a handle; no memory is
        // dereferenced.
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        if handle == 0 {
            return Err(format!(
                "Cannot inspect the linked game process: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self { handle })
    }

    pub fn is_alive(&self) -> Result<bool, String> {
        use windows_sys::Win32::{
            Foundation::{WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT},
            System::Threading::WaitForSingleObject,
        };
        // SAFETY: `handle` came from `OpenProcess` and is released in `Drop`.
        match unsafe { WaitForSingleObject(self.handle, 0) } {
            WAIT_TIMEOUT => Ok(true),
            WAIT_OBJECT_0 => Ok(false),
            WAIT_FAILED => Err(format!(
                "Cannot check the linked process lifetime: {}",
                std::io::Error::last_os_error()
            )),
            result => Err(format!("Unexpected linked-process wait result: {result}")),
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // SAFETY: `handle` is owned by this `Process` and closed exactly once.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::dismiss_stale;
    use super::*;

    #[test]
    fn retiring_generations_forgets_recycled_handles_without_affecting_other_overlays() {
        COMPOSITOR.with(|seen| {
            seen.borrow_mut().extend([
                ("Combat #1".to_string(), (11, true)),
                ("Combat #2".to_string(), (11, true)),
                ("Loot #3".to_string(), (12, false)),
            ]);
        });
        dismiss_stale("Combat", "Combat #2");
        COMPOSITOR.with(|seen| {
            let seen = seen.borrow();
            assert!(!seen.contains_key("Combat #1"));
            assert_eq!(seen.get("Combat #2"), Some(&(11, true)));
            assert_eq!(seen.get("Loot #3"), Some(&(12, false)));
            assert!(!seen.contains_key("Combat #4"));
        });
        dismiss_stale("Combat", "");
        COMPOSITOR.with(|seen| {
            assert!(!seen.borrow().contains_key("Combat #2"));
            assert_eq!(seen.borrow().get("Loot #3"), Some(&(12, false)));
        });
        dismiss_stale("Loot", "");
    }
}

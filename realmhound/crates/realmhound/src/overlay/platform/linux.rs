//! Linux overlay chrome, for X11 sessions and XWayland.
//!
//! Proton runs the game through Wine's X11 driver, so on a Wayland desktop the
//! client is an XWayland window. Native Wayland gives an overlay no way to stay
//! above another app, no fixed position and no global pointer, so RealmHound
//! runs on XWayland too unless the user opts out with `REALMHOUND_WAYLAND=1`
//! (see [`event_loop_hook`]). If XWayland turns out to be unusable, RealmHound
//! falls back to native Wayland instead of failing to start: up front when the
//! X libraries or display cannot be opened ([`uses_x11`]), and by relaunching
//! natively if winit still cannot start on X11 ([`relaunch_on_wayland`]).
//! Everything below is then plain X11:
//!
//! - winit already asks for `_NET_WM_STATE_ABOVE`, a transparent ARGB visual
//!   and an XShape input region for click-through. [`promote`] re-asserts
//!   "above" (window managers drop it when a fullscreen game takes focus), keeps
//!   the overlay out of the taskbar and pager, and clears the `WM_HINTS` input
//!   flag so clicking an overlay never takes keyboard focus from the game.
//! - A click-through window gets no pointer events, so [`cursor_position`] asks
//!   the X server, which is what keeps a locked overlay's title bar usable.
//!
//! Windows are found through `_NET_CLIENT_LIST`, restricted to this process by
//! `_NET_WM_PID`, then matched on the exact generation-suffixed title.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use x11rb::connection::Connection;
use x11rb::properties::WmHints;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, EventMask,
    StackMode, Window,
};
use x11rb::rust_connection::RustConnection;

/// `_NET_WM_STATE` client-message action that adds the listed states.
const NET_WM_STATE_ADD: u32 = 1;
/// `_NET_WM_STATE` source indication for a normal application.
const SOURCE_APPLICATION: u32 = 1;
/// Opt-out that keeps a Wayland session native. Also set on the replacement
/// [`relaunch_on_wayland`] starts, so a fallback can never loop.
const WAYLAND_OPT_OUT: &str = "REALMHOUND_WAYLAND";

/// Set once [`event_loop_hook`] has put winit on XWayland, so a startup failure
/// can be told apart from one native Wayland would have hit as well.
static FORCED_X11: AtomicBool = AtomicBool::new(false);

fn env_set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

/// Whether winit will talk to an X server for this process.
///
/// True on an X11 session and, by default, on a Wayland session with a usable
/// XWayland. `REALMHOUND_WAYLAND=1` keeps a Wayland session native, where the
/// overlays fall back to plain windows, and so does an XWayland that fails
/// [`xwayland_usable`].
pub fn uses_x11() -> bool {
    static USES_X11: OnceLock<bool> = OnceLock::new();
    *USES_X11.get_or_init(|| {
        if !env_set("DISPLAY") {
            return false;
        }
        if !env_set("WAYLAND_DISPLAY") {
            // A plain X11 session: winit picks X11 on its own.
            return true;
        }
        if std::env::var_os(WAYLAND_OPT_OUT).is_some_and(|value| value == "1") {
            return false;
        }
        xwayland_usable()
    })
}

/// Open the X display the way winit's X11 backend will, so a session where it
/// would fail stays on native Wayland instead of not starting at all.
///
/// winit loads Xlib, Xcursor, Xlib-xcb and XInput2 at runtime and then calls
/// `XOpenDisplay`; a Wayland-only install can lack the libraries, and `DISPLAY`
/// can be set with no XWayland behind it.
fn xwayland_usable() -> bool {
    use x11_dl::{xcursor::Xcursor, xinput2::XInput2, xlib::Xlib, xlib_xcb::Xlib_xcb};

    let libraries = Xlib::open().and_then(|xlib| {
        Xcursor::open()?;
        Xlib_xcb::open()?;
        XInput2::open()?;
        Ok(xlib)
    });
    let xlib = match libraries {
        Ok(xlib) => xlib,
        Err(e) => {
            tracing::warn!("[OVERLAY] X11 libraries unavailable ({e}); staying on Wayland");
            return false;
        }
    };
    // SAFETY: plain Xlib calls on a display this function opens and closes.
    // XInitThreads comes first, exactly as winit does, because it must precede
    // every other Xlib call in a multithreaded process; repeating it is a no-op.
    let opened = unsafe {
        (xlib.XInitThreads)();
        let display = (xlib.XOpenDisplay)(std::ptr::null());
        if display.is_null() {
            false
        } else {
            (xlib.XCloseDisplay)(display);
            true
        }
    };
    if !opened {
        tracing::warn!("[OVERLAY] XWayland display could not be opened; staying on Wayland");
    }
    opened
}

/// Event-loop hook that puts winit on XWayland in a Wayland session, or `None`
/// when winit would already choose X11 (or must stay on Wayland).
///
/// winit prefers Wayland whenever `WAYLAND_DISPLAY` is set, and the backend can
/// only be chosen before the event loop exists.
pub fn event_loop_hook() -> Option<eframe::EventLoopBuilderHook> {
    if !(uses_x11() && env_set("WAYLAND_DISPLAY")) {
        return None;
    }
    tracing::info!(
        "[OVERLAY] Wayland session: running on XWayland so overlays can stay above the \
         game client (set REALMHOUND_WAYLAND=1 to run natively)"
    );
    FORCED_X11.store(true, Ordering::Relaxed);
    Some(Box::new(|builder| {
        use winit::platform::x11::EventLoopBuilderExtX11 as _;
        builder.with_x11();
    }))
}

/// Whether `err` is winit failing to start on the XWayland [`event_loop_hook`]
/// forced, which native Wayland may still survive.
pub fn is_forced_x11_failure(err: &eframe::Error) -> bool {
    FORCED_X11.load(Ordering::Relaxed)
        && matches!(
            err,
            eframe::Error::WinitEventLoop(winit::error::EventLoopError::Os(_))
        )
}

/// Start a replacement RealmHound on native Wayland.
///
/// winit allows one event-loop attempt per process, so the retry has to be a
/// new process. It goes through the normal relaunch path, which waits for this
/// one to exit and release its single-instance and profile locks, and carries
/// `REALMHOUND_WAYLAND=1` so it cannot fall back again.
pub fn relaunch_on_wayland() -> std::io::Result<()> {
    use realmhound_core::relaunch::{RELAUNCH_FLAG, WAIT_PID_FLAG};

    std::process::Command::new(std::env::current_exe()?)
        .arg(RELAUNCH_FLAG)
        .arg(WAIT_PID_FLAG)
        .arg(std::process::id().to_string())
        .env(WAYLAND_OPT_OUT, "1")
        .spawn()
        .map(drop)
}

struct Atoms {
    client_list: Atom,
    wm_pid: Atom,
    wm_name: Atom,
    utf8_string: Atom,
    wm_state: Atom,
    above: Atom,
    skip_taskbar: Atom,
    skip_pager: Atom,
}

struct X11 {
    conn: RustConnection,
    root: Window,
    atoms: Atoms,
}

impl X11 {
    fn connect() -> Result<Self, Box<dyn std::error::Error>> {
        let (conn, screen) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen].root;
        let names: [&[u8]; 8] = [
            b"_NET_CLIENT_LIST",
            b"_NET_WM_PID",
            b"_NET_WM_NAME",
            b"UTF8_STRING",
            b"_NET_WM_STATE",
            b"_NET_WM_STATE_ABOVE",
            b"_NET_WM_STATE_SKIP_TASKBAR",
            b"_NET_WM_STATE_SKIP_PAGER",
        ];
        let cookies = names
            .iter()
            .map(|name| conn.intern_atom(false, name))
            .collect::<Result<Vec<_>, _>>()?;
        let mut atoms = cookies
            .into_iter()
            .map(|cookie| Ok(cookie.reply()?.atom))
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?
            .into_iter();
        let mut next = || atoms.next().unwrap_or_default();
        let atoms = Atoms {
            client_list: next(),
            wm_pid: next(),
            wm_name: next(),
            utf8_string: next(),
            wm_state: next(),
            above: next(),
            skip_taskbar: next(),
            skip_pager: next(),
        };
        Ok(Self { conn, root, atoms })
    }

    /// Find this process's top-level window titled exactly `title`.
    fn find(&self, title: &str) -> Option<Window> {
        let clients = self
            .conn
            .get_property(
                false,
                self.root,
                self.atoms.client_list,
                AtomEnum::WINDOW,
                0,
                u32::MAX,
            )
            .ok()?
            .reply()
            .ok()?;
        let clients: Vec<Window> = clients.value32()?.collect();
        // Pipeline the per-window lookups rather than one round trip each.
        let pids: Vec<_> = clients
            .iter()
            .map(|&window| {
                self.conn
                    .get_property(false, window, self.atoms.wm_pid, AtomEnum::CARDINAL, 0, 1)
            })
            .collect();
        let pid = std::process::id();
        let ours: Vec<Window> = clients
            .into_iter()
            .zip(pids)
            .filter_map(|(window, cookie)| {
                let reply = cookie.ok()?.reply().ok()?;
                let owner = reply.value32()?.next()?;
                (owner == pid).then_some(window)
            })
            .collect();
        ours.into_iter().find(|&window| {
            self.conn
                .get_property(
                    false,
                    window,
                    self.atoms.wm_name,
                    self.atoms.utf8_string,
                    0,
                    1024,
                )
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .is_some_and(|reply| reply.value == title.as_bytes())
        })
    }

    /// Ask the window manager to add up to two `_NET_WM_STATE` states.
    fn add_states(&self, window: Window, first: Atom, second: Atom) {
        let event = ClientMessageEvent::new(
            32,
            window,
            self.atoms.wm_state,
            [NET_WM_STATE_ADD, first, second, SOURCE_APPLICATION, 0],
        );
        let _ = self.conn.send_event(
            false,
            self.root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        );
    }
}

/// The shared X connection, or `None` off X11 or if the server refused it.
fn x11() -> Option<&'static X11> {
    static X11_CONN: OnceLock<Option<X11>> = OnceLock::new();
    X11_CONN
        .get_or_init(|| {
            if !uses_x11() {
                return None;
            }
            X11::connect()
                .inspect_err(|e| tracing::warn!("[OVERLAY] X11 connection failed: {e}"))
                .ok()
        })
        .as_ref()
}

thread_local! {
    /// Title -> window cache. Titles carry a per-window generation, so an entry
    /// can never point at a different window than the one it was found for.
    static FOUND: RefCell<HashMap<String, Window>> = RefCell::new(HashMap::new());
}

fn window(x: &X11, title: &str) -> Option<Window> {
    if let Some(window) = FOUND.with(|found| found.borrow().get(title).copied()) {
        return Some(window);
    }
    let window = x.find(title)?;
    FOUND.with(|found| found.borrow_mut().insert(title.to_owned(), window));
    Some(window)
}

pub(super) fn promote(title: &str) {
    let Some(x) = x11() else {
        return;
    };
    let Some(window) = window(x, title) else {
        return;
    };
    // ICCCM "no input" model: the window manager never gives the overlay
    // keyboard focus, so clicking its chrome leaves the game focused.
    if let Ok(cookie) = WmHints::get(&x.conn, window) {
        let mut hints = cookie.reply().ok().flatten().unwrap_or_default();
        if hints.input != Some(false) {
            hints.input = Some(false);
            let _ = hints.set(&x.conn, window);
        }
    }
    x.add_states(window, x.atoms.above, x.atoms.skip_taskbar);
    x.add_states(window, x.atoms.skip_pager, 0);
    let _ = x.conn.flush();
}

pub(super) fn order_front(title: &str) {
    let Some(x) = x11() else {
        return;
    };
    let Some(window) = window(x, title) else {
        return;
    };
    let _ = x.conn.configure_window(
        window,
        &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
    );
    let _ = x.conn.flush();
}

pub(super) fn cursor_position(pixels_per_point: f32) -> Option<(f32, f32)> {
    let x = x11()?;
    let pointer = x.conn.query_pointer(x.root).ok()?.reply().ok()?;
    // X reports physical pixels; window geometry is in logical points.
    let scale = if pixels_per_point > 0.0 {
        pixels_per_point
    } else {
        1.0
    };
    Some((
        f32::from(pointer.root_x) / scale,
        f32::from(pointer.root_y) / scale,
    ))
}

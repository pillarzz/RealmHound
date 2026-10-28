//! Reading and clearing the system clipboard.
//!
//! The Live Feed panel copies a dungeon callout by itself and drops it again once
//! the join window elapses. Dropping it must never wipe text the user copied in
//! the meantime, so the app compares the clipboard against the callout it wrote
//! before clearing.

/// Whether a clipboard currently holding `current` may be emptied by the
/// automatic callout cleanup that put `copied` there.
///
/// Only an exact match qualifies. Unreadable, non-text and empty clipboards all
/// report `None`, which keeps them (and any image, files or rich text the user
/// may have copied) intact.
fn should_clear(current: Option<&str>, copied: &str) -> bool {
    current == Some(copied)
}

/// Upper bound on the units RealmHound will look at. Only a short callout is
/// ever written here, so an implausibly large clipboard block is not worth
/// copying into a `String` even though its size is known.
#[cfg(windows)]
const MAX_UNITS: usize = 1 << 20;

/// `CF_UNICODETEXT` from winuser.h. windows-sys exposes the clipboard functions
/// but not the predefined format constants.
#[cfg(windows)]
const CF_UNICODETEXT: u32 = 13;

/// Decode a UTF-16 clipboard block, given the units the block actually holds.
///
/// Returns `None` when the block carries no null terminator, rather than reading
/// past the memory its owner allocated: a malformed payload must never be
/// scanned out of bounds.
#[cfg(windows)]
fn decode_utf16_block(units: &[u16]) -> Option<String> {
    let len = units.iter().position(|&unit| unit == 0)?;
    Some(String::from_utf16_lossy(&units[..len]))
}

/// The text of an already-open clipboard, or `None` when it is empty or holds
/// something that is not plain text.
///
/// # Safety
/// The caller must hold the clipboard open via `OpenClipboard`.
#[cfg(windows)]
unsafe fn text_while_open() -> Option<String> {
    use windows_sys::Win32::Foundation::HGLOBAL;
    use windows_sys::Win32::System::DataExchange::GetClipboardData;
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

    let handle = GetClipboardData(CF_UNICODETEXT);
    if handle == 0 {
        return None;
    }
    let hglobal: HGLOBAL = handle as HGLOBAL;
    // How many units the owner actually allocated. A size of 0 means the
    // handle is not a memory block we can bound, so nothing is read.
    let bytes = GlobalSize(hglobal);
    let ptr = GlobalLock(hglobal) as *const u16;
    if ptr.is_null() || bytes == 0 {
        None
    } else {
        // Bounded by the allocation, so the scan itself can never run past
        // the block.
        let units = (bytes / std::mem::size_of::<u16>()).min(MAX_UNITS);
        let text = if units == 0 {
            None
        } else {
            decode_utf16_block(std::slice::from_raw_parts(ptr, units))
        };
        GlobalUnlock(hglobal);
        text
    }
}

/// Empty the clipboard, but only while it still holds exactly `copied` -- the
/// callout this app put there. Returns whether it was cleared.
///
/// The clipboard stays open across the comparison and the clear, so no other
/// process can replace its contents in between.
#[cfg(windows)]
pub fn clear_if_ours(copied: &str) -> bool {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, IsClipboardFormatAvailable, OpenClipboard,
    };

    unsafe {
        // Not holding text at all -- e.g. an image or a file list: leave it be.
        if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
            return false;
        }
        // Another process may own the clipboard right now; then we simply can't
        // tell, and the callout is kept rather than risk clobbering anything.
        if OpenClipboard(0) == 0 {
            return false;
        }
        let ours = should_clear(text_while_open().as_deref(), copied);
        if ours {
            EmptyClipboard();
        }
        CloseClipboard();
        ours
    }
}

/// egui-winit's Wayland path drops writes while the window is unfocused, which
/// is exactly when an automatic callout fires. X11 keeps using egui's path.
#[cfg(target_os = "linux")]
pub fn set_text(text: &str) -> bool {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return false;
    }
    match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text)) {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!("[CLIPBOARD] Automatic copy failed: {error}");
            false
        }
    }
}

/// Empty the clipboard, but only while it still holds exactly `copied`.
/// Returns whether it was cleared.
#[cfg(not(windows))]
pub fn clear_if_ours(copied: &str) -> bool {
    // One context for the read and the clear: the callout cleanup runs at most
    // once per join window, and holding an X11/Wayland connection open for the
    // life of the process just to poll it occasionally is the worse trade. A
    // clipboard that is busy, empty, or holding an image reports `Err` here,
    // which keeps the callout rather than risk clobbering anything -- the same
    // outcome the Windows path reaches by bailing out.
    let Ok(mut clipboard) = arboard::Clipboard::new() else {
        return false;
    };
    if !should_clear(clipboard.get_text().ok().as_deref(), copied) {
        return false;
    }
    clipboard.clear().is_ok()
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::decode_utf16_block;
    use super::{clear_if_ours, should_clear};

    #[test]
    fn clears_only_the_callout_we_copied() {
        assert!(should_clear(Some("/p shatts 50 ffa"), "/p shatts 50 ffa"));
    }

    #[test]
    fn keeps_whatever_the_user_copied_since() {
        assert!(!should_clear(Some("something else"), "/p shatts 50 ffa"));
        // Longer, shorter, empty, or nothing at all: all preserved.
        assert!(!should_clear(
            Some("/p shatts 50 ffa and more"),
            "/p shatts 50 ffa"
        ));
        assert!(!should_clear(Some(""), "/p shatts 50 ffa"));
        assert!(!should_clear(None, "/p shatts 50 ffa"));
    }

    /// Reads the machine's real clipboard. Non-destructive, so it is safe to run
    /// anywhere: whatever is on the clipboard now is never this sentinel, so the
    /// clear is never reached, and an unreadable or busy clipboard simply
    /// reports "not ours".
    #[test]
    fn clearing_never_touches_someone_elses_text() {
        assert!(!clear_if_ours(
            "realmhound clipboard sentinel 4f3c1d9a-2b6e-4a58-9c71-0e5d8a6b7c42"
        ));
    }

    #[cfg(windows)]
    #[test]
    fn decodes_text_up_to_the_terminator() {
        assert_eq!(decode_utf16_block(&[104, 105, 0]), Some("hi".to_string()));
        // Anything the block holds after the terminator is not part of the text.
        assert_eq!(decode_utf16_block(&[104, 0, 105, 0]), Some("h".to_string()));
        // A terminated but empty block is valid text (never our own callout).
        assert_eq!(decode_utf16_block(&[0]), Some(String::new()));
    }

    #[cfg(windows)]
    #[test]
    fn rejects_a_block_without_a_terminator() {
        // What a malformed payload looks like: the allocated block ends without
        // a null, so decoding it must stop here instead of walking past the
        // memory the clipboard owner allocated.
        assert_eq!(
            decode_utf16_block(&[u16::from(b'a'), u16::from(b'b')]),
            None
        );
        assert_eq!(decode_utf16_block(&[]), None);
    }
}

//! Dock icon for the plain-binary macOS build.
//!
//! macOS reads an application icon from a bundle's `Info.plist`, which the bare
//! Mach-O published as `RealmHound-macos` does not have, so it lands in the Dock
//! and the app switcher wearing the generic executable icon. There is no Mach-O
//! equivalent of the Windows resource icon `build.rs` embeds, but a running
//! process may override its own Dock tile, so do that instead.
//!
//! `tools/macos/make-app-bundle.sh` builds a real bundle around the same
//! `icon.icns`, and its icon is visible in Finder before launch, so this module
//! stands aside for it.

use objc2::rc::Retained;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSImage};
use objc2_foundation::{NSBundle, NSData};

/// The same icon the `.app` bundle carries, so both packagings look identical.
const ICON: &[u8] = include_bytes!("../assets/icon.icns");

/// Decode the embedded icon. Thread-agnostic, unlike installing it.
fn decode_icon() -> Option<Retained<NSImage>> {
    NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(ICON))
}

/// Point the Dock tile at the RealmHound icon. Best-effort and infallible.
///
/// Must run on the AppKit main thread and after `NSApplication` exists, which is
/// why eframe's creation callback calls this rather than `main`.
pub fn install() {
    let Some(mtm) = MainThreadMarker::new() else {
        tracing::debug!("[ICON] skipping the Dock icon off the main thread");
        return;
    };

    // A bundled build already shows its icon everywhere, including in Finder
    // before launch, so there is nothing to correct.
    if NSBundle::mainBundle().bundleIdentifier().is_some() {
        return;
    }

    let Some(image) = decode_icon() else {
        tracing::warn!("[ICON] AppKit could not decode the embedded icon");
        return;
    };

    // SAFETY: the marker proves we are on the AppKit main thread, and AppKit
    // retains the image itself.
    unsafe {
        NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image));
    }
}

#[cfg(test)]
mod tests {
    use super::decode_icon;

    /// Catches an icon asset that went missing or stopped being a valid `.icns`,
    /// which would otherwise only show up as a generic Dock tile at runtime.
    #[test]
    fn appkit_decodes_the_embedded_icon() {
        let image = decode_icon().expect("AppKit should decode icon.icns");
        let size = image.size();
        assert!(size.width > 0.0 && size.height > 0.0, "got {size:?}");
    }
}

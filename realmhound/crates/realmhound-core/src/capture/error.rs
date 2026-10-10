//! Capture error types.

use thiserror::Error;

/// How to get a working capture device on this platform.
#[cfg(windows)]
const CAPTURE_SETUP_HELP: &str = "Npcap is not installed. Please install from https://npcap.com/";
#[cfg(target_os = "macos")]
const CAPTURE_SETUP_HELP: &str = "RealmHound may not open a capture device: reading /dev/bpf* \
                                  needs elevated access. Install Wireshark from \
                                  https://www.wireshark.org/ and allow its ChmodBPF helper, then \
                                  log out and back in.";
#[cfg(all(unix, not(target_os = "macos")))]
const CAPTURE_SETUP_HELP: &str = "RealmHound may not open a capture device. Install libpcap, then \
                                  grant capture rights with `sudo setcap \
                                  cap_net_raw,cap_net_admin=eip <path to RealmHound>`.";

/// Errors that can occur during packet capture.
#[derive(Error, Debug)]
pub enum CaptureError {
    /// Npcap/pcap is not installed, or its capture devices are not readable.
    #[error("{}", CAPTURE_SETUP_HELP)]
    NpcapNotInstalled,

    /// No network interfaces found
    #[error("No network interfaces found")]
    NoInterfaces,

    /// Failed to open interface
    #[error("Failed to open interface '{name}': {reason}")]
    InterfaceOpenFailed { name: String, reason: String },

    /// Failed to set filter
    #[error("Failed to set capture filter: {0}")]
    FilterFailed(String),

    /// The interface uses an unsupported packet framing format.
    #[error(
        "Unsupported capture link type {0}; supported formats are Ethernet, raw IP, and loopback"
    )]
    UnsupportedLinkType(i32),

    /// Capture error
    #[error("Capture error: {0}")]
    CaptureError(String),

    /// Pcap error
    #[error("Pcap error: {0}")]
    PcapError(#[from] pcap::Error),
}

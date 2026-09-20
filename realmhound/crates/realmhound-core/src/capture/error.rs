//! Capture error types.

use thiserror::Error;

/// Errors that can occur during packet capture.
#[derive(Error, Debug)]
pub enum CaptureError {
    /// Npcap/pcap is not installed
    #[error("Npcap is not installed. Please install from https://npcap.com/")]
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

    /// Capture error
    #[error("Capture error: {0}")]
    CaptureError(String),

    /// Pcap error
    #[error("Pcap error: {0}")]
    PcapError(#[from] pcap::Error),
}

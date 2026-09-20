//! Network packet capture module.
//!
//! This module provides functionality to capture network packets using Npcap/pcap.

pub mod error;
pub mod interface;
pub mod recorder;
pub mod sniffer;

pub use error::CaptureError;
pub use interface::NetworkInterface;
pub use recorder::{captures_dir, read_capture, CaptureWriter};
pub use sniffer::{
    packet_channel, start_capture, start_capture_auto_detect, CaptureHandle, DrainResult,
    PacketReceiver, PacketSender, RawPacket, Sniffer, SnifferConfig, DEFAULT_QUEUE_CAPACITY,
};

/// The port used by Realm of the Mad God
pub const ROTMG_PORT: u16 = 2050;

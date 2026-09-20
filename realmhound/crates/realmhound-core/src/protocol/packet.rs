//! Core packet structures.

use super::PacketType;
use chrono::{DateTime, Utc};

/// Header information for a RotMG packet.
#[derive(Debug, Clone)]
pub struct PacketHeader {
    /// Total size of the packet including header (4 bytes)
    pub size: u32,
    /// Packet type identifier (1 byte)
    pub packet_type: PacketType,
    /// Raw packet id byte as seen on the wire.
    ///
    /// Preserved separately from `packet_type` because unrecognized ids collapse
    /// to `PacketType::Unknown`, losing the original value. Needed so the generic
    /// fallback can report the true id of packets not in the `PacketType` enum.
    pub raw_id: u8,
}

impl PacketHeader {
    /// Size of the packet header in bytes.
    pub const SIZE: usize = 5;

    /// Parse a packet header from at least 5 bytes, or `None` if too short.
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < Self::SIZE {
            return None;
        }

        let size = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        let raw_id = data[4];
        let packet_type = PacketType::from_id(raw_id);

        Some(Self {
            size,
            packet_type,
            raw_id,
        })
    }

    /// Get the size of the payload (excluding header).
    pub fn payload_size(&self) -> usize {
        self.size.saturating_sub(Self::SIZE as u32) as usize
    }
}

/// A decoded RotMG packet.
#[derive(Debug, Clone)]
pub struct Packet {
    /// When the packet was captured
    pub timestamp: DateTime<Utc>,
    /// Whether this packet is incoming (server → client) or outgoing (client → server)
    pub incoming: bool,
    /// The packet header
    pub header: PacketHeader,
    /// The decrypted packet payload
    pub payload: Vec<u8>,
    /// Source IP address
    pub src_ip: std::net::Ipv4Addr,
    /// Destination IP address
    pub dst_ip: std::net::Ipv4Addr,
    /// Source port
    pub src_port: u16,
    /// Destination port
    pub dst_port: u16,
}

impl Packet {
    /// Get the packet type.
    pub fn packet_type(&self) -> PacketType {
        self.header.packet_type
    }

    /// Get the packet type name.
    pub fn type_name(&self) -> &'static str {
        self.header.packet_type.name()
    }

    /// Get the total packet size including header.
    pub fn total_size(&self) -> usize {
        self.header.size as usize
    }

    /// Get the direction as a string.
    pub fn direction(&self) -> &'static str {
        if self.incoming {
            "S>C"
        } else {
            "C>S"
        }
    }
}

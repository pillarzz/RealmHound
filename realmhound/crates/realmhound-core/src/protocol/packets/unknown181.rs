//! UnknownPacket181 implementation.
//!
//! Unknown packet carrying a single boolean.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Unknown181Packet (ID 181) - Incoming
#[derive(Debug, Clone)]
pub struct Unknown181Packet {
    /// Unknown boolean.
    pub unknown: bool,
}

impl RotmgPacket for Unknown181Packet {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let unknown = reader.read_bool()?;

        Ok(Self { unknown })
    }

    fn description(&self) -> String {
        format!("Unknown181: unknown={}", self.unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = [1u8];
        let mut reader = PacketReader::new(&data);
        let packet = Unknown181Packet::deserialize(&mut reader).unwrap();

        assert!(packet.unknown);
        assert!(reader.is_fully_parsed());
    }
}

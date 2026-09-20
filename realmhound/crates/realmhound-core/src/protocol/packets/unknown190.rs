//! UnknownPacket190 implementation.
//!
//! Unknown packet carrying a single byte.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Unknown190Packet (ID 190) - Incoming
#[derive(Debug, Clone)]
pub struct Unknown190Packet {
    /// Unknown byte.
    pub unknown: i8,
}

impl RotmgPacket for Unknown190Packet {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let unknown = reader.read_byte()? as i8;

        Ok(Self { unknown })
    }

    fn description(&self) -> String {
        format!("Unknown190: unknown={}", self.unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = [5u8];
        let mut reader = PacketReader::new(&data);
        let packet = Unknown190Packet::deserialize(&mut reader).unwrap();

        assert_eq!(packet.unknown, 5);
        assert!(reader.is_fully_parsed());
    }
}

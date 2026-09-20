//! UnknownPacket165 implementation.
//!
//! Unknown packet carrying a single string.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Unknown165Packet (ID 165) - Incoming
#[derive(Debug, Clone)]
pub struct Unknown165Packet {
    /// Unknown string.
    pub unknown_string: String,
}

impl RotmgPacket for Unknown165Packet {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let unknown_string = reader.read_string()?;

        Ok(Self { unknown_string })
    }

    fn description(&self) -> String {
        format!("Unknown165: string={}", self.unknown_string)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = [0u8, 2, b'h', b'i'];
        let mut reader = PacketReader::new(&data);
        let packet = Unknown165Packet::deserialize(&mut reader).unwrap();

        assert_eq!(packet.unknown_string, "hi");
        assert!(reader.is_fully_parsed());
    }
}

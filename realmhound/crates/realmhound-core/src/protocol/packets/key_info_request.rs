//! KeyInfoRequestPacket implementation.
//!
//! Requests key info for an item type.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// KeyInfoRequestPacket (ID 94) - Outgoing
#[derive(Debug, Clone)]
pub struct KeyInfoRequestPacket {
    /// The item type being queried.
    pub item_type: i32,
}

impl RotmgPacket for KeyInfoRequestPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let item_type = reader.read_i32()?;

        Ok(Self { item_type })
    }

    fn description(&self) -> String {
        format!("KeyInfoRequest: itemType={}", self.item_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = [0u8, 0, 1, 0];
        let mut reader = PacketReader::new(&data);
        let packet = KeyInfoRequestPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.item_type, 256);
        assert!(reader.is_fully_parsed());
    }
}

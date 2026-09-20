//! BuyPacket implementation.
//!
//! Sent to buy an item.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// BuyPacket (ID 85) - Outgoing
#[derive(Debug, Clone)]
pub struct BuyPacket {
    /// The object id of the item being purchased.
    pub object_id: i32,
    /// The number of items being purchased.
    pub quantity: i32,
}

impl RotmgPacket for BuyPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let object_id = reader.read_i32()?;
        let quantity = reader.read_i32()?;
        Ok(Self {
            object_id,
            quantity,
        })
    }

    fn description(&self) -> String {
        format!(
            "Buy: objectId={}, quantity={}",
            self.object_id, self.quantity
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&777i32.to_be_bytes());
        data.extend_from_slice(&3i32.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = BuyPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.object_id, 777);
        assert_eq!(packet.quantity, 3);
        assert!(reader.is_fully_parsed());
    }
}

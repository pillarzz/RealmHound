//! ForgeRequest packet implementation.
//!
//! Sent when forging an item.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use super::SlotObjectData;
use std::io;

/// ForgeRequest packet (ID 118) - Outgoing
#[derive(Debug, Clone)]
pub struct ForgeRequestPacket {
    /// The item type being forged.
    pub result_item_type: i32,
    /// The items being dismantled to forge the result.
    pub dismantled_items: Vec<SlotObjectData>,
}

impl RotmgPacket for ForgeRequestPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let result_item_type = reader.read_i32()?;
        let count = reader.read_i32()?;
        if count < 0 || count as usize > reader.remaining() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid dismantled item count: {}", count),
            ));
        }
        let mut dismantled_items = Vec::with_capacity(count as usize);
        for _ in 0..count {
            dismantled_items.push(SlotObjectData::deserialize(reader)?);
        }
        Ok(Self {
            result_item_type,
            dismantled_items,
        })
    }

    fn description(&self) -> String {
        format!(
            "ForgeRequest: result={}, dismantled={}",
            self.result_item_type,
            self.dismantled_items.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&999i32.to_be_bytes()); // resultItemType
        data.extend_from_slice(&1i32.to_be_bytes()); // 1 dismantled
        data.extend_from_slice(&1i32.to_be_bytes()); // slot.objectId
        data.extend_from_slice(&2i32.to_be_bytes()); // slot.slotId
        data.extend_from_slice(&3i32.to_be_bytes()); // slot.itemType

        let mut reader = PacketReader::new(&data);
        let packet = ForgeRequestPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.result_item_type, 999);
        assert_eq!(packet.dismantled_items.len(), 1);
        assert_eq!(packet.dismantled_items[0].item_type, 3);
        assert!(reader.is_fully_parsed());
    }
}

//! UseItemPacket implementation.
//!
//! Sent when the player uses an item such as an ability or consumable.
//!

use super::super::data::WorldPosData;
use super::traits::RotmgPacket;
use super::PacketReader;
use super::SlotObjectData;
use std::io;

/// UseItemPacket (ID 13) - Outgoing
#[derive(Debug, Clone)]
pub struct UseItemPacket {
    /// The current client time.
    pub time: i32,
    /// The slot of the item being used.
    pub slot_object: SlotObjectData,
    /// The position of the player in the world using the item.
    pub use_item_position: WorldPosData,
    /// The type of item usage (raw `UseItemType` code).
    pub use_item_type: u8,
    /// Additional item-use flag; for multi-ability items matches the ability attack index.
    pub use_item_flag: i32,
}

impl RotmgPacket for UseItemPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let time = reader.read_i32()?;
        let slot_object = SlotObjectData::deserialize(reader)?;
        let use_item_position = WorldPosData::deserialize(reader)?;
        let use_item_type = reader.read_byte()?;
        let use_item_flag = reader.read_i32()?;
        Ok(Self {
            time,
            slot_object,
            use_item_position,
            use_item_type,
            use_item_flag,
        })
    }

    fn description(&self) -> String {
        format!(
            "UseItem: time={}, item={}, type={}",
            self.time, self.slot_object.item_type, self.use_item_type
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&100i32.to_be_bytes()); // time
        data.extend_from_slice(&1i32.to_be_bytes()); // slot.objectId
        data.extend_from_slice(&2i32.to_be_bytes()); // slot.slotId
        data.extend_from_slice(&3i32.to_be_bytes()); // slot.itemType
        data.extend_from_slice(&4.5f32.to_be_bytes()); // pos.x
        data.extend_from_slice(&6.5f32.to_be_bytes()); // pos.y
        data.push(2); // useItemType
        data.extend_from_slice(&9i32.to_be_bytes()); // useItemFlag

        let mut reader = PacketReader::new(&data);
        let packet = UseItemPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.time, 100);
        assert_eq!(packet.slot_object.item_type, 3);
        assert_eq!(packet.use_item_position.x, 4.5);
        assert_eq!(packet.use_item_type, 2);
        assert_eq!(packet.use_item_flag, 9);
        assert!(reader.is_fully_parsed());
    }
}

//! InvDropPacket implementation.
//!
//! Sent to drop an item from the client's inventory.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use super::SlotObjectData;
use std::io;

/// InvDropPacket (ID 19) - Outgoing
#[derive(Debug, Clone)]
pub struct InvDropPacket {
    /// The slot to drop the item from.
    pub slot_object: SlotObjectData,
    /// Quick slot.
    pub quick_slot: i8,
}

impl RotmgPacket for InvDropPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let slot_object = SlotObjectData::deserialize(reader)?;
        let quick_slot = reader.read_byte()? as i8;
        Ok(Self {
            slot_object,
            quick_slot,
        })
    }

    fn description(&self) -> String {
        format!(
            "InvDrop: obj={}, slot={}, item={}",
            self.slot_object.object_id, self.slot_object.slot_id, self.slot_object.item_type
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&1i32.to_be_bytes()); // objectId
        data.extend_from_slice(&4i32.to_be_bytes()); // slotId
        data.extend_from_slice(&(-1i32).to_be_bytes()); // itemType
        data.push(0xFF); // quickSlot = -1

        let mut reader = PacketReader::new(&data);
        let packet = InvDropPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.slot_object.object_id, 1);
        assert_eq!(packet.slot_object.slot_id, 4);
        assert_eq!(packet.slot_object.item_type, -1);
        assert_eq!(packet.quick_slot, -1);
        assert!(reader.is_fully_parsed());
    }
}

//! BuyRefinement packet implementation.
//!
//! Sent to buy or refund a refinement on an item.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use super::SlotObjectData;
use std::io;

/// BuyRefinement packet (ID 136) - Outgoing
#[derive(Debug, Clone)]
pub struct BuyRefinementPacket {
    /// The slot of the item being refined.
    pub slot_object: SlotObjectData,
    /// The refine action ordinal (`RefineAction`).
    pub action: i32,
}

impl RotmgPacket for BuyRefinementPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let slot_object = SlotObjectData::deserialize(reader)?;
        let action = reader.read_i32()?;
        Ok(Self {
            slot_object,
            action,
        })
    }

    fn description(&self) -> String {
        format!(
            "BuyRefinement: item={}, action={}",
            self.slot_object.item_type, self.action
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&1i32.to_be_bytes()); // slot.objectId
        data.extend_from_slice(&2i32.to_be_bytes()); // slot.slotId
        data.extend_from_slice(&3i32.to_be_bytes()); // slot.itemType
        data.extend_from_slice(&1i32.to_be_bytes()); // action

        let mut reader = PacketReader::new(&data);
        let packet = BuyRefinementPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.slot_object.item_type, 3);
        assert_eq!(packet.action, 1);
        assert!(reader.is_fully_parsed());
    }
}

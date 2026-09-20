//! ClaimChestReward packet implementation.
//!
//! Sent when the client claims a chest reward.
//! Direction verified from live TCP capture.

use super::inv_swap::SlotObjectData;
use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ClaimChestReward packet (ID 171) - Outgoing
#[derive(Debug, Clone)]
pub struct ClaimChestRewardPacket {
    /// Client-provided claim flag.
    pub accepted: bool,
    /// The slot/item selected for the claim.
    pub slot_object: SlotObjectData,
    /// The index that was selected.
    pub selected_idx: u8,
}

impl RotmgPacket for ClaimChestRewardPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let accepted = reader.read_bool()?;
        let slot_object = SlotObjectData::deserialize(reader)?;
        let selected_idx = reader.read_byte()?;

        Ok(Self {
            accepted,
            slot_object,
            selected_idx,
        })
    }

    fn description(&self) -> String {
        format!(
            "ClaimChestReward: accepted={} item={} idx={}",
            self.accepted, self.slot_object.item_type, self.selected_idx
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_bytes(
        accepted: bool,
        object_id: i32,
        slot_id: i32,
        item_type: i32,
        idx: u8,
    ) -> Vec<u8> {
        let mut data = Vec::new();
        data.push(if accepted { 1 } else { 0 });
        data.extend_from_slice(&object_id.to_be_bytes());
        data.extend_from_slice(&slot_id.to_be_bytes());
        data.extend_from_slice(&item_type.to_be_bytes());
        data.push(idx);
        data
    }

    #[test]
    fn test_deserialize() {
        let data = build_bytes(true, 555, 4, 1234, 2);
        let mut reader = PacketReader::new(&data);
        let packet = ClaimChestRewardPacket::deserialize(&mut reader).unwrap();

        assert!(packet.accepted);
        assert_eq!(packet.slot_object.object_id, 555);
        assert_eq!(packet.slot_object.slot_id, 4);
        assert_eq!(packet.slot_object.item_type, 1234);
        assert_eq!(packet.selected_idx, 2);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_empty_slot_item() {
        let data = build_bytes(false, 1, 0, -1, 0);
        let mut reader = PacketReader::new(&data);
        let packet = ClaimChestRewardPacket::deserialize(&mut reader).unwrap();

        assert!(!packet.accepted);
        assert_eq!(packet.slot_object.item_type, -1);
    }
}

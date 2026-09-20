//! InvResult packet implementation.
//!
//! Received as the server's confirmation of an inventory move.

use super::inv_swap::SlotObjectData;
use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// InvResult packet (ID 95) - Incoming
#[derive(Debug, Clone)]
pub struct InvResultPacket {
    /// Whether the inventory move succeeded.
    pub result: bool,
    /// Result type byte.
    pub result_type: u8,
    /// The slot the item was transferred from.
    pub slot_from: SlotObjectData,
    /// The slot the item was transferred to.
    pub slot_to: SlotObjectData,
    /// Unknown condition 1.
    pub condition1: i32,
    /// Unknown condition 2.
    pub condition2: i32,
}

impl RotmgPacket for InvResultPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let result = reader.read_bool()?;
        let result_type = reader.read_byte()?;
        let slot_from = SlotObjectData::deserialize(reader)?;
        let slot_to = SlotObjectData::deserialize(reader)?;
        let condition1 = reader.read_i32()?;
        let condition2 = reader.read_i32()?;
        Ok(Self {
            result,
            result_type,
            slot_from,
            slot_to,
            condition1,
            condition2,
        })
    }

    fn description(&self) -> String {
        format!(
            "InvResult: result={} type={} from(obj={},slot={}) to(obj={},slot={})",
            self.result,
            self.result_type,
            self.slot_from.object_id,
            self.slot_from.slot_id,
            self.slot_to.object_id,
            self.slot_to.slot_id
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_slot(data: &mut Vec<u8>, obj: i32, slot: i32, item: i32) {
        data.extend_from_slice(&obj.to_be_bytes());
        data.extend_from_slice(&slot.to_be_bytes());
        data.extend_from_slice(&item.to_be_bytes());
    }

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.push(1); // result
        data.push(2); // resultType
        push_slot(&mut data, 10, 4, 100); // slotFrom
        push_slot(&mut data, 20, 5, -1); // slotTo
        data.extend_from_slice(&7i32.to_be_bytes()); // condition1
        data.extend_from_slice(&8i32.to_be_bytes()); // condition2

        let mut reader = PacketReader::new(&data);
        let packet = InvResultPacket::deserialize(&mut reader).unwrap();

        assert!(packet.result);
        assert_eq!(packet.result_type, 2);
        assert_eq!(packet.slot_from.object_id, 10);
        assert_eq!(packet.slot_from.slot_id, 4);
        assert_eq!(packet.slot_from.item_type, 100);
        assert_eq!(packet.slot_to.object_id, 20);
        assert_eq!(packet.slot_to.item_type, -1);
        assert_eq!(packet.condition1, 7);
        assert_eq!(packet.condition2, 8);
        assert!(reader.is_fully_parsed());
    }
}

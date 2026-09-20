//! QuestRedeem packet implementation.
//!
//! Sent when redeeming a quest.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use super::SlotObjectData;
use std::io;

/// QuestRedeem packet (ID 58) - Outgoing
#[derive(Debug, Clone)]
pub struct QuestRedeemPacket {
    /// ID of the quest.
    pub quest_id_string: String,
    /// Unknown int.
    pub quest_id_int: i32,
    /// Slots associated with the redemption.
    pub slots: Vec<SlotObjectData>,
}

impl RotmgPacket for QuestRedeemPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let quest_id_string = reader.read_string()?;
        let quest_id_int = reader.read_i32()?;
        let slot_count = reader.read_i16()?;
        if slot_count < 0 || slot_count as usize > reader.remaining() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid slot count: {}", slot_count),
            ));
        }
        let mut slots = Vec::with_capacity(slot_count as usize);
        for _ in 0..slot_count {
            slots.push(SlotObjectData::deserialize(reader)?);
        }
        Ok(Self {
            quest_id_string,
            quest_id_int,
            slots,
        })
    }

    fn description(&self) -> String {
        format!(
            "QuestRedeem: quest={}, slots={}",
            self.quest_id_string,
            self.slots.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&4u16.to_be_bytes());
        data.extend_from_slice(b"q123");
        data.extend_from_slice(&7i32.to_be_bytes()); // questIdInt
        data.extend_from_slice(&1i16.to_be_bytes()); // 1 slot
        data.extend_from_slice(&10i32.to_be_bytes()); // slot.objectId
        data.extend_from_slice(&11i32.to_be_bytes()); // slot.slotId
        data.extend_from_slice(&12i32.to_be_bytes()); // slot.itemType

        let mut reader = PacketReader::new(&data);
        let packet = QuestRedeemPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.quest_id_string, "q123");
        assert_eq!(packet.quest_id_int, 7);
        assert_eq!(packet.slots.len(), 1);
        assert_eq!(packet.slots[0].object_id, 10);
        assert!(reader.is_fully_parsed());
    }
}

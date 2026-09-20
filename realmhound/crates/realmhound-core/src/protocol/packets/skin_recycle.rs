//! SkinRecyclePacket implementation.
//!
//! Sent to recycle a skin.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use super::SlotObjectData;
use std::io;

/// SkinRecyclePacket (ID 146) - Outgoing
#[derive(Debug, Clone)]
pub struct SkinRecyclePacket {
    /// The slot of the skin being recycled.
    pub slot_object: SlotObjectData,
}

impl RotmgPacket for SkinRecyclePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let slot_object = SlotObjectData::deserialize(reader)?;
        Ok(Self { slot_object })
    }

    fn description(&self) -> String {
        format!(
            "SkinRecycle: obj={}, slot={}, item={}",
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
        data.extend_from_slice(&1i32.to_be_bytes());
        data.extend_from_slice(&2i32.to_be_bytes());
        data.extend_from_slice(&3i32.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = SkinRecyclePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.slot_object.object_id, 1);
        assert_eq!(packet.slot_object.item_type, 3);
        assert!(reader.is_fully_parsed());
    }
}

//! ForgeResult packet implementation.
//!
//! Received when the player uses the item forge.

use super::inv_swap::SlotObjectData;
use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ForgeResult packet (ID 119) - Incoming
#[derive(Debug, Clone)]
pub struct ForgeResultPacket {
    /// Whether the forge was successful.
    pub success: bool,
    /// The slot data of the items forged.
    pub results: Vec<SlotObjectData>,
}

impl RotmgPacket for ForgeResultPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let success = reader.read_bool()?;
        // Length is a single byte (0-255), so no further bound check is needed.
        let len = reader.read_byte()? as usize;
        let mut results = Vec::with_capacity(len);
        for _ in 0..len {
            results.push(SlotObjectData::deserialize(reader)?);
        }
        Ok(Self { success, results })
    }

    fn description(&self) -> String {
        format!(
            "ForgeResult: success={} results={}",
            self.success,
            self.results.len()
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
        data.push(1); // success
        data.push(2); // results length (byte)
        push_slot(&mut data, 1, 2, 300);
        push_slot(&mut data, 4, 5, 600);

        let mut reader = PacketReader::new(&data);
        let packet = ForgeResultPacket::deserialize(&mut reader).unwrap();

        assert!(packet.success);
        assert_eq!(packet.results.len(), 2);
        assert_eq!(packet.results[0].item_type, 300);
        assert_eq!(packet.results[1].object_id, 4);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_deserialize_empty() {
        let data = [0u8, 0u8]; // success=false, length=0
        let mut reader = PacketReader::new(&data);
        let packet = ForgeResultPacket::deserialize(&mut reader).unwrap();

        assert!(!packet.success);
        assert!(packet.results.is_empty());
        assert!(reader.is_fully_parsed());
    }
}

//! Goto packet implementation.
//!
//! Received when an entity has moved to a new position.

use super::super::data::WorldPosData;
use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Goto packet (ID 18) - Incoming
#[derive(Debug, Clone)]
pub struct GotoPacket {
    /// The object id of the entity which moved.
    pub object_id: i32,
    /// The new position of the entity.
    pub position: WorldPosData,
    /// Unknown int.
    pub unknown_int: i32,
}

impl RotmgPacket for GotoPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let object_id = reader.read_i32()?;
        let position = WorldPosData::deserialize(reader)?;
        let unknown_int = reader.read_i32()?;

        Ok(Self {
            object_id,
            position,
            unknown_int,
        })
    }

    fn description(&self) -> String {
        format!(
            "Goto: objectId={} pos=({:.1},{:.1})",
            self.object_id, self.position.x, self.position.y
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&1234i32.to_be_bytes()); // objectId
        data.extend_from_slice(&12.5f32.to_be_bytes()); // pos.x
        data.extend_from_slice(&34.5f32.to_be_bytes()); // pos.y
        data.extend_from_slice(&99i32.to_be_bytes()); // unknownInt

        let mut reader = PacketReader::new(&data);
        let packet = GotoPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.object_id, 1234);
        assert_eq!(packet.position.x, 12.5);
        assert_eq!(packet.position.y, 34.5);
        assert_eq!(packet.unknown_int, 99);
        assert!(reader.is_fully_parsed());
    }
}

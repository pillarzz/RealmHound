//! CreepMovePacket implementation.
//!
//! Sent when playing the Summoner class and a spawned creep minion has to move.
//!

use super::super::data::WorldPosData;
use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// CreepMovePacket (ID 126) - Outgoing
#[derive(Debug, Clone)]
pub struct CreepMovePacket {
    /// The object id of the Summoner's creep to move.
    pub object_id: i32,
    /// Server time.
    pub server_time: i32,
    /// The position to move the creep to.
    pub position: WorldPosData,
    /// Whether the Summoner ability key is held down.
    pub hold: bool,
}

impl RotmgPacket for CreepMovePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let object_id = reader.read_i32()?;
        let server_time = reader.read_i32()?;
        let position = WorldPosData::deserialize(reader)?;
        let hold = reader.read_bool()?;

        Ok(Self {
            object_id,
            server_time,
            position,
            hold,
        })
    }

    fn description(&self) -> String {
        format!(
            "CreepMove: objectId={}, pos=({:.1},{:.1}), hold={}",
            self.object_id, self.position.x, self.position.y, self.hold
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&11i32.to_be_bytes());
        data.extend_from_slice(&22i32.to_be_bytes());
        data.extend_from_slice(&3.0f32.to_be_bytes());
        data.extend_from_slice(&4.0f32.to_be_bytes());
        data.push(1);

        let mut reader = PacketReader::new(&data);
        let packet = CreepMovePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.object_id, 11);
        assert_eq!(packet.server_time, 22);
        assert_eq!(packet.position.x, 3.0);
        assert!(packet.hold);
        assert!(reader.is_fully_parsed());
    }
}

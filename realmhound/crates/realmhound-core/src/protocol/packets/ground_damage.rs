//! GroundDamagePacket implementation.
//!
//! Sent when the client takes damage from a ground source, such as lava.
//!

use super::super::data::WorldPosData;
use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// GroundDamagePacket (ID 103) - Outgoing
#[derive(Debug, Clone)]
pub struct GroundDamagePacket {
    /// The current client time.
    pub time: i32,
    /// The current client position.
    pub position: WorldPosData,
}

impl RotmgPacket for GroundDamagePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let time = reader.read_i32()?;
        let position = WorldPosData::deserialize(reader)?;

        Ok(Self { time, position })
    }

    fn description(&self) -> String {
        format!(
            "GroundDamage: time={}, pos=({:.1},{:.1})",
            self.time, self.position.x, self.position.y
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&555i32.to_be_bytes());
        data.extend_from_slice(&12.0f32.to_be_bytes());
        data.extend_from_slice(&34.0f32.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = GroundDamagePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.time, 555);
        assert_eq!(packet.position.x, 12.0);
        assert_eq!(packet.position.y, 34.0);
        assert!(reader.is_fully_parsed());
    }
}

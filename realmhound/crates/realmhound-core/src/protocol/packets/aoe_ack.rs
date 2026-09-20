//! AoeAckPacket implementation.
//!
//! Sent to acknowledge an `AoePacket`.
//!

use super::super::data::WorldPosData;
use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// AoeAckPacket (ID 89) - Outgoing
#[derive(Debug, Clone)]
pub struct AoeAckPacket {
    /// The current client time.
    pub time: i32,
    /// The position of the AoE which this packet is acknowledging.
    pub position: WorldPosData,
}

impl RotmgPacket for AoeAckPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let time = reader.read_i32()?;
        let position = WorldPosData::deserialize(reader)?;

        Ok(Self { time, position })
    }

    fn description(&self) -> String {
        format!(
            "AoeAck: time={}, pos=({:.1},{:.1})",
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
        data.extend_from_slice(&77i32.to_be_bytes());
        data.extend_from_slice(&1.0f32.to_be_bytes());
        data.extend_from_slice(&2.0f32.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = AoeAckPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.time, 77);
        assert_eq!(packet.position.x, 1.0);
        assert_eq!(packet.position.y, 2.0);
        assert!(reader.is_fully_parsed());
    }
}

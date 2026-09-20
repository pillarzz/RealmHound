//! DashPacket implementation.
//!
//! Confirms a Kensei dash to specific coordinates.
//!

use super::super::data::WorldPosData;
use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// DashPacket (ID 137) - Outgoing
#[derive(Debug, Clone)]
pub struct DashPacket {
    /// The current client time.
    pub time: i32,
    /// The dash start position.
    pub start: WorldPosData,
    /// The dash end position.
    pub end: WorldPosData,
}

impl RotmgPacket for DashPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let time = reader.read_i32()?;
        let start = WorldPosData::deserialize(reader)?;
        let end = WorldPosData::deserialize(reader)?;

        Ok(Self { time, start, end })
    }

    fn description(&self) -> String {
        format!(
            "Dash: time={}, start=({:.1},{:.1}), end=({:.1},{:.1})",
            self.time, self.start.x, self.start.y, self.end.x, self.end.y
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&9i32.to_be_bytes());
        data.extend_from_slice(&1.0f32.to_be_bytes());
        data.extend_from_slice(&2.0f32.to_be_bytes());
        data.extend_from_slice(&3.0f32.to_be_bytes());
        data.extend_from_slice(&4.0f32.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = DashPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.time, 9);
        assert_eq!(packet.start.x, 1.0);
        assert_eq!(packet.end.y, 4.0);
        assert!(reader.is_fully_parsed());
    }
}

//! ClaimMission packet implementation.
//!
//! Sent to claim a season mission reward.
//!
//! Note: live protocol widened `missionPositionalIdx` from byte to int.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ClaimMission packet (ID 163) - Outgoing
#[derive(Debug, Clone)]
pub struct ClaimMissionPacket {
    /// The id of the season.
    pub season_id: i32,
    /// Positional index of the mission.
    pub mission_positional_idx: i32,
    /// Request id.
    pub request_id: i8,
    /// Bitmask.
    pub mask: i16,
}

impl RotmgPacket for ClaimMissionPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let season_id = reader.read_i32()?;
        let mission_positional_idx = reader.read_i32()?;
        let request_id = reader.read_byte()? as i8;
        let mask = reader.read_i16()?;
        Ok(Self {
            season_id,
            mission_positional_idx,
            request_id,
            mask,
        })
    }

    fn description(&self) -> String {
        format!(
            "ClaimMission: seasonId={}, missionIdx={}",
            self.season_id, self.mission_positional_idx
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
        data.extend_from_slice(&2i32.to_be_bytes());
        data.push(1);
        data.extend_from_slice(&7i16.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = ClaimMissionPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.season_id, 9);
        assert_eq!(packet.mission_positional_idx, 2);
        assert_eq!(packet.request_id, 1);
        assert_eq!(packet.mask, 7);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_deserialize_live_payload() {
        // Captured from a live "claim mission" action (11-byte payload).
        let data = [
            0x00, 0x00, 0x00, 0x2F, 0x00, 0x00, 0x00, 0x05, 0x09, 0xFF, 0xFF,
        ];

        let mut reader = PacketReader::new(&data);
        let packet = ClaimMissionPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.season_id, 47);
        assert_eq!(packet.mission_positional_idx, 5);
        assert_eq!(packet.request_id, 9);
        assert_eq!(packet.mask, -1);
        assert!(reader.is_fully_parsed());
    }
}

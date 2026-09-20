//! SetTrackedSeason packet implementation.
//!
//! Sent to set the currently tracked season.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// SetTrackedSeason packet (ID 162) - Outgoing
#[derive(Debug, Clone)]
pub struct SetTrackedSeasonPacket {
    /// The id of the season to track.
    pub season_id: i32,
}

impl RotmgPacket for SetTrackedSeasonPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let season_id = reader.read_i32()?;
        Ok(Self { season_id })
    }

    fn description(&self) -> String {
        format!("SetTrackedSeason: seasonId={}", self.season_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = 42i32.to_be_bytes();
        let mut reader = PacketReader::new(&data);
        let packet = SetTrackedSeasonPacket::deserialize(&mut reader).unwrap();
        assert_eq!(packet.season_id, 42);
        assert!(reader.is_fully_parsed());
    }
}

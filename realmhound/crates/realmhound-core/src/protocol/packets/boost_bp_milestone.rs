//! BoostBpMilestone packet implementation.
//!
//! Sent to boost a battle pass milestone.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// BoostBpMilestone packet (ID 151) - Outgoing
#[derive(Debug, Clone)]
pub struct BoostBpMilestonePacket {
    /// The number of milestones to boost.
    pub milestone_count: i8,
}

impl RotmgPacket for BoostBpMilestonePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let milestone_count = reader.read_byte()? as i8;
        Ok(Self { milestone_count })
    }

    fn description(&self) -> String {
        format!("BoostBpMilestone: count={}", self.milestone_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = [3u8];
        let mut reader = PacketReader::new(&data);
        let packet = BoostBpMilestonePacket::deserialize(&mut reader).unwrap();
        assert_eq!(packet.milestone_count, 3);
        assert!(reader.is_fully_parsed());
    }
}

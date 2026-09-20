//! ResetDailyQuests packet implementation.
//!
//! Sent to reset the daily quests currently available.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ResetDailyQuests packet (ID 52) - Outgoing (empty)
#[derive(Debug, Clone)]
pub struct ResetDailyQuestsPacket;

impl RotmgPacket for ResetDailyQuestsPacket {
    fn deserialize(_reader: &mut PacketReader) -> io::Result<Self> {
        Ok(Self)
    }

    fn description(&self) -> String {
        "ResetDailyQuests".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut reader = PacketReader::new(&[]);
        let packet = ResetDailyQuestsPacket::deserialize(&mut reader).unwrap();
        assert_eq!(packet.description(), "ResetDailyQuests");
        assert!(reader.is_fully_parsed());
    }
}

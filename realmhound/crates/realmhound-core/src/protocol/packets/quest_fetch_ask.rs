//! QuestFetchAsk packet implementation.
//!
//! Sent to request the latest quests.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// QuestFetchAsk packet (ID 98) - Outgoing (empty)
#[derive(Debug, Clone)]
pub struct QuestFetchAskPacket;

impl RotmgPacket for QuestFetchAskPacket {
    fn deserialize(_reader: &mut PacketReader) -> io::Result<Self> {
        Ok(Self)
    }

    fn description(&self) -> String {
        "QuestFetchAsk".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut reader = PacketReader::new(&[]);
        let packet = QuestFetchAskPacket::deserialize(&mut reader).unwrap();
        assert_eq!(packet.description(), "QuestFetchAsk");
        assert!(reader.is_fully_parsed());
    }
}

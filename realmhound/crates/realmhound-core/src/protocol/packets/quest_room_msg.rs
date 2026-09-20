//! QuestRoomMsg packet implementation.
//!
//! Sent to prompt the server to send a `ReconnectPacket` containing the
//! reconnect information for the Quest Room.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// QuestRoomMsg packet (ID 48) - Outgoing (empty)
#[derive(Debug, Clone)]
pub struct QuestRoomMsgPacket;

impl RotmgPacket for QuestRoomMsgPacket {
    fn deserialize(_reader: &mut PacketReader) -> io::Result<Self> {
        Ok(Self)
    }

    fn description(&self) -> String {
        "QuestRoomMsg".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut reader = PacketReader::new(&[]);
        let packet = QuestRoomMsgPacket::deserialize(&mut reader).unwrap();
        assert_eq!(packet.description(), "QuestRoomMsg");
        assert!(reader.is_fully_parsed());
    }
}

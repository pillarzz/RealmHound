//! QuestRedeemResponse packet implementation.
//!
//! Received after attempting to redeem a quest.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// QuestRedeemResponse packet (ID 96) - Incoming
#[derive(Debug, Clone)]
pub struct QuestRedeemResponsePacket {
    /// Whether the quest redemption was accepted.
    pub ok: bool,
    /// Message used in the response dialog.
    pub message: String,
}

impl RotmgPacket for QuestRedeemResponsePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let ok = reader.read_bool()?;
        let message = reader.read_string()?;

        Ok(Self { ok, message })
    }

    fn description(&self) -> String {
        format!("QuestRedeemResponse: ok={} msg='{}'", self.ok, self.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_bytes(ok: bool, msg: &str) -> Vec<u8> {
        let mut data = Vec::new();
        data.push(if ok { 1 } else { 0 });
        data.extend_from_slice(&(msg.len() as u16).to_be_bytes());
        data.extend_from_slice(msg.as_bytes());
        data
    }

    #[test]
    fn test_deserialize_ok() {
        let data = build_bytes(true, "Quest accepted");
        let mut reader = PacketReader::new(&data);
        let packet = QuestRedeemResponsePacket::deserialize(&mut reader).unwrap();

        assert!(packet.ok);
        assert_eq!(packet.message, "Quest accepted");
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_deserialize_failed() {
        let data = build_bytes(false, "");
        let mut reader = PacketReader::new(&data);
        let packet = QuestRedeemResponsePacket::deserialize(&mut reader).unwrap();

        assert!(!packet.ok);
        assert_eq!(packet.message, "");
    }
}

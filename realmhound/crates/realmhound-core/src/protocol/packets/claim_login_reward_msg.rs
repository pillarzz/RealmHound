//! ClaimLoginRewardMsg packet implementation.
//!
//! Sent to claim rewards from the login calendar.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ClaimLoginRewardMsg packet (ID 3) - Outgoing
#[derive(Debug, Clone)]
pub struct ClaimLoginRewardMsgPacket {
    /// The key of the item being claimed.
    pub claim_key: String,
    /// The type of claim being made.
    pub claim_type: String,
}

impl RotmgPacket for ClaimLoginRewardMsgPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let claim_key = reader.read_string()?;
        let claim_type = reader.read_string()?;
        Ok(Self {
            claim_key,
            claim_type,
        })
    }

    fn description(&self) -> String {
        format!(
            "ClaimLoginReward: key={}, type={}",
            self.claim_key, self.claim_type
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn str_bytes(s: &str) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&(s.len() as u16).to_be_bytes());
        b.extend_from_slice(s.as_bytes());
        b
    }

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&str_bytes("day1"));
        data.extend_from_slice(&str_bytes("gold"));

        let mut reader = PacketReader::new(&data);
        let packet = ClaimLoginRewardMsgPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.claim_key, "day1");
        assert_eq!(packet.claim_type, "gold");
        assert!(reader.is_fully_parsed());
    }
}

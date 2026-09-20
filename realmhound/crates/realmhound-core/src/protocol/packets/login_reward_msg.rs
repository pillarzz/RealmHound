//! LoginRewardMsg packet implementation.
//!
//! Received in response to a `ClaimDailyRewardMessage`.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// LoginRewardMsg packet (ID 93) - Incoming
#[derive(Debug, Clone)]
pub struct LoginRewardMsgPacket {
    /// The item id of the reward received.
    pub item_id: i32,
    /// The number of items received.
    pub quantity: i32,
    /// Unknown gold value.
    pub gold: i32,
}

impl RotmgPacket for LoginRewardMsgPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let item_id = reader.read_i32()?;
        let quantity = reader.read_i32()?;
        let gold = reader.read_i32()?;
        Ok(Self {
            item_id,
            quantity,
            gold,
        })
    }

    fn description(&self) -> String {
        format!(
            "LoginRewardMsg: itemId={} quantity={} gold={}",
            self.item_id, self.quantity, self.gold
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&100i32.to_be_bytes()); // itemId
        data.extend_from_slice(&3i32.to_be_bytes()); // quantity
        data.extend_from_slice(&50i32.to_be_bytes()); // gold

        let mut reader = PacketReader::new(&data);
        let packet = LoginRewardMsgPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.item_id, 100);
        assert_eq!(packet.quantity, 3);
        assert_eq!(packet.gold, 50);
        assert!(reader.is_fully_parsed());
    }
}

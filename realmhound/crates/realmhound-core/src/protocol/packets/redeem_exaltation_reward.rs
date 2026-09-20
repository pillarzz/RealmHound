//! RedeemExaltationRewardPacket implementation.
//!
//! Sent to redeem an exaltation reward item.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// RedeemExaltationRewardPacket (ID 115) - Outgoing
#[derive(Debug, Clone)]
pub struct RedeemExaltationRewardPacket {
    /// The item id redeemed.
    pub item_id: i32,
}

impl RotmgPacket for RedeemExaltationRewardPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let item_id = reader.read_i32()?;

        Ok(Self { item_id })
    }

    fn description(&self) -> String {
        format!("RedeemExaltationReward: itemId={}", self.item_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = [0u8, 0, 0, 42];
        let mut reader = PacketReader::new(&data);
        let packet = RedeemExaltationRewardPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.item_id, 42);
        assert!(reader.is_fully_parsed());
    }
}

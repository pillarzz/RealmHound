//! ClaimBpMilestoneResult packet implementation.
//!
//! Received when redeeming battle pass milestone items.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ClaimBpMilestoneResult packet (ID 150) - Incoming
#[derive(Debug, Clone)]
pub struct ClaimBpMilestoneResultPacket {
    /// Whether the milestone item was redeemed.
    pub item_redeemed: bool,
}

impl RotmgPacket for ClaimBpMilestoneResultPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let item_redeemed = reader.read_bool()?;
        Ok(Self { item_redeemed })
    }

    fn description(&self) -> String {
        format!("ClaimBpMilestoneResult: redeemed={}", self.item_redeemed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = [1u8];
        let mut reader = PacketReader::new(&data);
        let packet = ClaimBpMilestoneResultPacket::deserialize(&mut reader).unwrap();

        assert!(packet.item_redeemed);
        assert!(reader.is_fully_parsed());
    }
}

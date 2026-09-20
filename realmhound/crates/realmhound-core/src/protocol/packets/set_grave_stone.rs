//! SetGraveStonePacket implementation.
//!
//! Sent to set the player's gravestone customization.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// SetGraveStonePacket (ID 156) - Outgoing
#[derive(Debug, Clone)]
pub struct SetGraveStonePacket {
    /// The gravestone object type.
    pub grave_stone_type: i32,
    /// The gravestone tier.
    pub tier: i32,
}

impl RotmgPacket for SetGraveStonePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let grave_stone_type = reader.read_i32()?;
        let tier = reader.read_i32()?;

        Ok(Self {
            grave_stone_type,
            tier,
        })
    }

    fn description(&self) -> String {
        format!(
            "SetGraveStone: type={}, tier={}",
            self.grave_stone_type, self.tier
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = [0u8, 0, 7, 35, 0, 0, 0, 1];
        let mut reader = PacketReader::new(&data);
        let packet = SetGraveStonePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.grave_stone_type, 1827);
        assert_eq!(packet.tier, 1);
        assert!(reader.is_fully_parsed());
    }
}

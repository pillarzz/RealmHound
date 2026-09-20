//! UpgradeEnchantment packet implementation.
//!
//! Sent to upgrade an enchantment in a slot.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// UpgradeEnchantment packet (ID 187) - Outgoing
#[derive(Debug, Clone)]
pub struct UpgradeEnchantmentPacket {
    /// Unknown short.
    pub unknown: i16,
    /// The slot index being upgraded.
    pub slot_idx: i8,
}

impl RotmgPacket for UpgradeEnchantmentPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let unknown = reader.read_i16()?;
        let slot_idx = reader.read_byte()? as i8;
        Ok(Self { unknown, slot_idx })
    }

    fn description(&self) -> String {
        format!("UpgradeEnchantment: slot={}", self.slot_idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&1i16.to_be_bytes());
        data.push(4);

        let mut reader = PacketReader::new(&data);
        let packet = UpgradeEnchantmentPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.unknown, 1);
        assert_eq!(packet.slot_idx, 4);
        assert!(reader.is_fully_parsed());
    }
}

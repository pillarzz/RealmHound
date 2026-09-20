//! UnlockEnchantment packet implementation.
//!
//! Sent to unlock an enchantment.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// UnlockEnchantment packet (ID 175) - Outgoing
#[derive(Debug, Clone)]
pub struct UnlockEnchantmentPacket {
    /// Unknown short.
    pub unknown: i16,
    /// The enchantment type.
    pub enchantment_type: i16,
}

impl RotmgPacket for UnlockEnchantmentPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let unknown = reader.read_i16()?;
        let enchantment_type = reader.read_i16()?;
        Ok(Self {
            unknown,
            enchantment_type,
        })
    }

    fn description(&self) -> String {
        format!("UnlockEnchantment: type={}", self.enchantment_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&1i16.to_be_bytes());
        data.extend_from_slice(&8i16.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = UnlockEnchantmentPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.unknown, 1);
        assert_eq!(packet.enchantment_type, 8);
        assert!(reader.is_fully_parsed());
    }
}

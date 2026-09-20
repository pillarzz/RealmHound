//! ResetEnchantmentRerollCount packet implementation.
//!
//! Sent to reset the enchantment reroll count.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ResetEnchantmentRerollCount packet (ID 191) - Outgoing
#[derive(Debug, Clone)]
pub struct ResetEnchantmentRerollCountPacket {
    /// The enchantment id.
    pub enchantment_id: i16,
}

impl RotmgPacket for ResetEnchantmentRerollCountPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let enchantment_id = reader.read_i16()?;
        Ok(Self { enchantment_id })
    }

    fn description(&self) -> String {
        format!("ResetEnchantmentRerollCount: id={}", self.enchantment_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = 5i16.to_be_bytes();
        let mut reader = PacketReader::new(&data);
        let packet = ResetEnchantmentRerollCountPacket::deserialize(&mut reader).unwrap();
        assert_eq!(packet.enchantment_id, 5);
        assert!(reader.is_fully_parsed());
    }
}

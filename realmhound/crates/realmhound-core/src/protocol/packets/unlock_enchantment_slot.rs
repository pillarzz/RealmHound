//! UnlockEnchantmentSlot packet implementation.
//!
//! Sent to unlock an enchantment slot on an item.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// UnlockEnchantmentSlot packet (ID 173) - Outgoing
#[derive(Debug, Clone)]
pub struct UnlockEnchantmentSlotPacket {
    /// The enchantment id.
    pub enchantment_id: i16,
    /// The slot index being unlocked.
    pub slot_idx: i8,
}

impl RotmgPacket for UnlockEnchantmentSlotPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let enchantment_id = reader.read_i16()?;
        let slot_idx = reader.read_byte()? as i8;
        Ok(Self {
            enchantment_id,
            slot_idx,
        })
    }

    fn description(&self) -> String {
        format!(
            "UnlockEnchantmentSlot: id={}, slot={}",
            self.enchantment_id, self.slot_idx
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&5i16.to_be_bytes());
        data.push(2);

        let mut reader = PacketReader::new(&data);
        let packet = UnlockEnchantmentSlotPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.enchantment_id, 5);
        assert_eq!(packet.slot_idx, 2);
        assert!(reader.is_fully_parsed());
    }
}

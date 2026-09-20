//! ApplyEnchantment packet implementation.
//!
//! Sent to apply or remove an enchantment on an item.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ApplyEnchantment packet (ID 177) - Outgoing
#[derive(Debug, Clone)]
pub struct ApplyEnchantmentPacket {
    /// Unknown short.
    pub unknown: i16,
    /// The enchantment type.
    pub enchantment_type: i16,
    /// Whether the enchantment is being added (true) or removed (false).
    pub add: bool,
    /// The enchantment slot index.
    pub enchantment_slot_idx: i8,
}

impl RotmgPacket for ApplyEnchantmentPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let unknown = reader.read_i16()?;
        let enchantment_type = reader.read_i16()?;
        let add = reader.read_bool()?;
        let enchantment_slot_idx = reader.read_byte()? as i8;
        Ok(Self {
            unknown,
            enchantment_type,
            add,
            enchantment_slot_idx,
        })
    }

    fn description(&self) -> String {
        format!(
            "ApplyEnchantment: type={}, add={}, slot={}",
            self.enchantment_type, self.add, self.enchantment_slot_idx
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&0i16.to_be_bytes());
        data.extend_from_slice(&8i16.to_be_bytes());
        data.push(1); // add = true
        data.push(3); // slot

        let mut reader = PacketReader::new(&data);
        let packet = ApplyEnchantmentPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.enchantment_type, 8);
        assert!(packet.add);
        assert_eq!(packet.enchantment_slot_idx, 3);
        assert!(reader.is_fully_parsed());
    }
}

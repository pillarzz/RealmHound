//! RerollAllEnchantments packet implementation.
//!
//! Sent to reroll all enchantments.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// RerollAllEnchantments packet (ID 189) - Outgoing
///
/// The reference implementation reads only a single `i16`, but live captures from
/// current clients carry a variable-length payload. Field layout was recovered
/// from real captures:
///
/// ```text
/// no lock:    01 00 | 01 | FF FF | 00        locked_slots = []
/// 1st locked: 01 00 | 06 | FF FF | 01 00     locked_slots = [0]
/// 2nd locked: 01 00 | 05 | FF FF | 01 01     locked_slots = [1]
/// ```
///
/// - `i16` enchantment_id (stable per item)
/// - `i8` reroll value (changes on every reroll; exact meaning unknown)
/// - `i16` slot_index, `-1` = "all slots" (matches "reroll all")
/// - `i8`-length-prefixed array of zero-based locked slot indices
#[derive(Debug, Clone)]
pub struct RerollAllEnchantmentsPacket {
    /// The enchantment/item id.
    pub enchantment_id: i16,
    /// Per-reroll value byte; changes on each reroll (exact meaning unknown).
    pub reroll_value: i8,
    /// Slot index; `-1` indicates all slots (matches "reroll all").
    pub slot_index: i16,
    /// Zero-based indices of slots the player locked before rerolling.
    pub locked_slots: Vec<u8>,
}

impl RotmgPacket for RerollAllEnchantmentsPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let enchantment_id = reader.read_i16()?;
        let reroll_value = reader.read_byte()? as i8;
        let slot_index = reader.read_i16()?;
        let count = reader.read_byte()? as usize;
        if count > reader.remaining() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "RerollAllEnchantments locked-slot count exceeds payload",
            ));
        }
        let mut locked_slots = Vec::with_capacity(count);
        for _ in 0..count {
            locked_slots.push(reader.read_byte()?);
        }
        Ok(Self {
            enchantment_id,
            reroll_value,
            slot_index,
            locked_slots,
        })
    }

    fn description(&self) -> String {
        format!(
            "RerollAllEnchantments: id={}, slot={}, locked={:?}",
            self.enchantment_id, self.slot_index, self.locked_slots
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_no_lock() {
        // Real capture: enchantment_id=256, slot_index=-1, no locked slots
        let data = [0x01u8, 0x00, 0x01, 0xFF, 0xFF, 0x00];
        let mut reader = PacketReader::new(&data);
        let packet = RerollAllEnchantmentsPacket::deserialize(&mut reader).unwrap();
        assert_eq!(packet.enchantment_id, 256);
        assert_eq!(packet.slot_index, -1);
        assert!(packet.locked_slots.is_empty());
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_deserialize_first_slot_locked() {
        // Real capture: locking the first slot -> locked_slots = [0]
        let data = [0x01u8, 0x00, 0x06, 0xFF, 0xFF, 0x01, 0x00];
        let mut reader = PacketReader::new(&data);
        let packet = RerollAllEnchantmentsPacket::deserialize(&mut reader).unwrap();
        assert_eq!(packet.enchantment_id, 256);
        assert_eq!(packet.slot_index, -1);
        assert_eq!(packet.locked_slots, vec![0]);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_deserialize_second_slot_locked() {
        // Real capture: locking the second slot -> locked_slots = [1]
        let data = [0x01u8, 0x00, 0x05, 0xFF, 0xFF, 0x01, 0x01];
        let mut reader = PacketReader::new(&data);
        let packet = RerollAllEnchantmentsPacket::deserialize(&mut reader).unwrap();
        assert_eq!(packet.enchantment_id, 256);
        assert_eq!(packet.slot_index, -1);
        assert_eq!(packet.locked_slots, vec![1]);
        assert!(reader.is_fully_parsed());
    }
}

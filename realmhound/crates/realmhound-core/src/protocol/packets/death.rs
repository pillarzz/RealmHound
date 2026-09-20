//! Death packet implementation.
//!
//! Received when the player's character dies.
//! Contains death details including killer, fame earned, and fame bonuses.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Fame bonus data awarded on death.
#[derive(Debug, Clone)]
pub struct FameBonus {
    /// Name of the achievement/bonus
    pub achievement: String,
    /// Fame level tier
    pub fame_level: i32,
    /// Fame amount added by this bonus
    pub fame_added: i32,
}

/// Death packet (ID 46) - Incoming
///
/// Sent by server when the player's character dies.
/// Contains full death information: who killed them, gravestone type,
/// total fame earned, and all fame bonuses.
#[derive(Debug, Clone)]
pub struct DeathPacket {
    /// Account ID of the dead player
    pub account_id: String,
    /// Character ID of the dead player
    pub char_id: i32,
    /// Name of the entity that killed the player
    pub killed_by: String,
    /// Gravestone type ID
    pub gravestone_type: i32,
    /// Total death fame earned
    pub total_fame: i32,
    /// Fame bonuses (achievement name, level, fame added)
    pub fame_data: Vec<FameBonus>,
    /// Stats string of the dead player
    pub stats: String,
}

impl RotmgPacket for DeathPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let account_id = reader.read_string()?;
        let char_id = reader.read_compressed_int()?;
        let killed_by = reader.read_string()?;
        let gravestone_type = reader.read_i32()?;
        let total_fame = reader.read_compressed_int()?;

        // NEW FIELDS (added after the reference implementation was written):
        // Field 1: compressed int (purpose unknown, possibly base fame or xp?)
        let _unknown_compressed = reader.read_compressed_int()?;
        // Field 2: i32 (purpose unknown, possibly timestamp or char creation time?)
        let _unknown_i32 = reader.read_i32()?;

        // Fame bonuses array
        let fame_count = reader.read_compressed_int()? as usize;
        let mut fame_data = Vec::with_capacity(fame_count.min(50));
        for _ in 0..fame_count.min(50) {
            let achievement = reader.read_string()?;
            let fame_level = reader.read_compressed_int()?;
            let fame_added = reader.read_compressed_int()?;
            fame_data.push(FameBonus {
                achievement,
                fame_level,
                fame_added,
            });
        }

        // Stats string (6-bit encoded PCStats)
        let stats = reader.read_string()?;

        Ok(Self {
            account_id,
            char_id,
            killed_by,
            gravestone_type,
            total_fame,
            fame_data,
            stats,
        })
    }

    fn description(&self) -> String {
        format!(
            "Death: charId={}, killedBy='{}', fame={}",
            self.char_id, self.killed_by, self.total_fame
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_death_packet_deserialize() {
        let mut data = Vec::new();

        // account_id: string
        let account_id = "ABC123";
        data.extend_from_slice(&(account_id.len() as u16).to_be_bytes());
        data.extend_from_slice(account_id.as_bytes());

        // char_id: compressed int (42 = 0x2A, fits in one byte with no sign bit)
        data.push(42);

        // killed_by: string
        let killed_by = "Oryx the Mad God";
        data.extend_from_slice(&(killed_by.len() as u16).to_be_bytes());
        data.extend_from_slice(killed_by.as_bytes());

        // gravestone_type: i32
        data.extend_from_slice(&5i32.to_be_bytes());

        // total_fame: compressed int (100)
        // 100 = 0b1100100, needs 7 bits, but first byte only has 6 value bits
        // first byte: 0b10_100100 = 0x80 | (100 & 63) = 0x80 | 36 = 0xA4
        // second byte: 100 >> 6 = 1, no continuation = 0x01
        data.push(0xA4);
        data.push(0x01);

        // NEW FIELDS added to packet format:
        // unknown_compressed: compressed int (0)
        data.push(0);
        // unknown_i32: i32 (0)
        data.extend_from_slice(&0i32.to_be_bytes());

        // fame_data count: compressed int (0 = no bonuses)
        data.push(0);

        // stats: string
        let stats = "test_stats";
        data.extend_from_slice(&(stats.len() as u16).to_be_bytes());
        data.extend_from_slice(stats.as_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = DeathPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.account_id, "ABC123");
        assert_eq!(packet.char_id, 42);
        assert_eq!(packet.killed_by, "Oryx the Mad God");
        assert_eq!(packet.gravestone_type, 5);
        assert_eq!(packet.total_fame, 100);
        assert!(packet.fame_data.is_empty());
        assert_eq!(packet.stats, "test_stats");
    }

    #[test]
    fn test_death_packet_description() {
        let packet = DeathPacket {
            account_id: "test".to_string(),
            char_id: 1,
            killed_by: "Oryx".to_string(),
            gravestone_type: 5,
            total_fame: 500,
            fame_data: vec![],
            stats: String::new(),
        };
        assert!(packet.description().contains("charId=1"));
        assert!(packet.description().contains("Oryx"));
        assert!(packet.description().contains("500"));
    }
}

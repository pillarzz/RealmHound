//! ExaltationBonusChanged packet implementation.
//!
//! Received when the player's exaltation stats are updated (e.g., after
//! completing an exaltation dungeon).

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ExaltationBonusChanged packet (ID 114) - Incoming
///
/// Sent by server when exaltation progress changes for a class.
/// Contains the updated raw progress values for all 8 stats.
#[derive(Debug, Clone)]
pub struct ExaltationUpdatePacket {
    /// The object type (class ID) of the player's class
    pub obj_type: i16,
    /// Dexterity exaltation progress (dungeon completions)
    pub dexterity_progress: i32,
    /// Speed exaltation progress
    pub speed_progress: i32,
    /// Vitality exaltation progress
    pub vitality_progress: i32,
    /// Wisdom exaltation progress
    pub wisdom_progress: i32,
    /// Defense exaltation progress
    pub defense_progress: i32,
    /// Attack exaltation progress
    pub attack_progress: i32,
    /// Mana (MP) exaltation progress
    pub mana_progress: i32,
    /// Health (HP) exaltation progress
    pub health_progress: i32,
}

impl RotmgPacket for ExaltationUpdatePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        // Packet order: objType, then DEX, SPD, VIT, WIS, DEF, ATT, MP, HP
        let obj_type = reader.read_i16()?;
        let dexterity_progress = reader.read_compressed_int()?;
        let speed_progress = reader.read_compressed_int()?;
        let vitality_progress = reader.read_compressed_int()?;
        let wisdom_progress = reader.read_compressed_int()?;
        let defense_progress = reader.read_compressed_int()?;
        let attack_progress = reader.read_compressed_int()?;
        let mana_progress = reader.read_compressed_int()?;
        let health_progress = reader.read_compressed_int()?;

        Ok(Self {
            obj_type,
            dexterity_progress,
            speed_progress,
            vitality_progress,
            wisdom_progress,
            defense_progress,
            attack_progress,
            mana_progress,
            health_progress,
        })
    }

    fn description(&self) -> String {
        format!(
            "ExaltationUpdate: class={} (DEX={}, SPD={}, VIT={}, WIS={}, DEF={}, ATT={}, MP={}, HP={})",
            self.obj_type,
            self.dexterity_progress,
            self.speed_progress,
            self.vitality_progress,
            self.wisdom_progress,
            self.defense_progress,
            self.attack_progress,
            self.mana_progress,
            self.health_progress
        )
    }
}

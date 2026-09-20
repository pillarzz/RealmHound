//! DamagePacket implementation.
//!
//! Received to tell the player about damage done to other players and enemies.
//! Used for loot attribution - only entities the player hit can be considered as droppers.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// DamagePacket (ID 75) - Incoming
///
/// Received to tell the player about damage done to other players and enemies.
/// Contains the target entity ID and attacker ID for loot attribution tracking.
#[derive(Debug, Clone)]
pub struct DamagePacket {
    /// The object id of the entity receiving the damage.
    pub target_id: i32,
    /// An array of status effects which were applied with the damage.
    pub effects: Vec<u8>,
    /// The amount of damage taken.
    pub damage_amount: u16,
    /// Damage properties.
    pub damage_properties: bool,
    /// The id of the bullet which caused the damage.
    pub bullet_id: u16,
    /// The object id of the entity which owned the bullet that caused the damage.
    pub object_id: i32,
    /// Unknown byte (possibly kill flag or critical hit indicator)
    pub unknown: u8,
}

impl RotmgPacket for DamagePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let target_id = reader.read_i32()?;

        // Read effects array
        let effects_len = reader.read_byte()? as usize;
        let mut effects = Vec::with_capacity(effects_len);
        for _ in 0..effects_len {
            effects.push(reader.read_byte()?);
        }

        let damage_amount = reader.read_u16()?;
        let damage_properties = reader.read_bool()?;
        let bullet_id = reader.read_u16()?;
        let object_id = reader.read_i32()?;

        // Unknown trailing byte (added in recent protocol update)
        let unknown = if reader.remaining() >= 1 {
            reader.read_byte()?
        } else {
            0
        };

        Ok(Self {
            target_id,
            effects,
            damage_amount,
            damage_properties,
            bullet_id,
            object_id,
            unknown,
        })
    }

    fn description(&self) -> String {
        format!(
            "Damage: target={}, attacker={}, damage={}",
            self.target_id, self.object_id, self.damage_amount
        )
    }
}

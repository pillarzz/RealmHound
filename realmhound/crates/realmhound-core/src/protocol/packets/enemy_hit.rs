//! EnemyHitPacket implementation.
//!
//! Sent when an enemy has been hit by the player.
//! Used for loot attribution - only entities the player hit can be considered as droppers.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// EnemyHitPacket (ID 25) - Outgoing
///
/// Sent when an enemy has been hit by the player.
/// Contains the target entity ID which is used for loot attribution tracking.
#[derive(Debug, Clone)]
pub struct EnemyHitPacket {
    /// The current client time.
    pub time: i32,
    /// The id of the bullet which hit the enemy.
    pub bullet_id: i16,
    /// ID of the shooter hitting the target.
    pub shooter_id: i32,
    /// The object id of the enemy which was hit.
    pub target_id: i32,
    /// Whether the projectile will kill the enemy.
    pub kill: bool,
    /// Id of the main player hitting the target.
    pub main_id: i32,
}

impl RotmgPacket for EnemyHitPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let time = reader.read_i32()?;
        let bullet_id = reader.read_i16()?;
        let shooter_id = reader.read_i32()?;
        let target_id = reader.read_i32()?;
        let kill = reader.read_bool()?;
        let main_id = reader.read_i32()?;

        Ok(Self {
            time,
            bullet_id,
            shooter_id,
            target_id,
            kill,
            main_id,
        })
    }

    fn description(&self) -> String {
        format!(
            "EnemyHit: target={}, shooter={}, kill={}",
            self.target_id, self.shooter_id, self.kill
        )
    }
}

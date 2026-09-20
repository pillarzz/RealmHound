//! EnemyShoot packet implementation.
//!
//! Received when a visible enemy shoots a projectile.

use super::super::data::WorldPosData;
use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// EnemyShoot packet (ID 35) - Incoming
#[derive(Debug, Clone)]
pub struct EnemyShootPacket {
    /// The id of the bullet which was fired.
    pub bullet_id: i16,
    /// The object id of the enemy which fired the projectile.
    pub owner_id: i32,
    /// Bullet type.
    pub bullet_type: u8,
    /// The position at which the projectile was fired.
    pub starting_pos: WorldPosData,
    /// The angle at which the projectile was fired.
    pub angle: f32,
    /// The damage which the projectile will cause.
    pub damage: i16,
    /// The number of projectiles fired.
    pub num_shots: u8,
    /// The angle in degrees between the projectiles if `num_shots > 1`.
    pub angle_inc: f32,
}

impl RotmgPacket for EnemyShootPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let bullet_id = reader.read_i16()?;
        let owner_id = reader.read_i32()?;
        let bullet_type = reader.read_byte()?;
        let starting_pos = WorldPosData::deserialize(reader)?;
        let angle = reader.read_f32()?;
        let damage = reader.read_i16()?;

        // Trailing fields are optional; a single-shot packet omits them.
        let (num_shots, angle_inc) = if reader.remaining() > 0 {
            let num_shots = reader.read_byte()?;
            let angle_inc = reader.read_f32()?;
            (num_shots, angle_inc)
        } else {
            (255, 0.0)
        };

        Ok(Self {
            bullet_id,
            owner_id,
            bullet_type,
            starting_pos,
            angle,
            damage,
            num_shots,
            angle_inc,
        })
    }

    fn description(&self) -> String {
        format!(
            "EnemyShoot: bulletId={} owner={} type={} pos=({:.1},{:.1}) angle={:.2} dmg={} shots={} angleInc={:.2}",
            self.bullet_id,
            self.owner_id,
            self.bullet_type,
            self.starting_pos.x,
            self.starting_pos.y,
            self.angle,
            self.damage,
            self.num_shots,
            self.angle_inc
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_bytes() -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&7i16.to_be_bytes()); // bulletId
        data.extend_from_slice(&1234i32.to_be_bytes()); // ownerId
        data.push(2); // bulletType
        data.extend_from_slice(&5.0f32.to_be_bytes()); // pos.x
        data.extend_from_slice(&6.0f32.to_be_bytes()); // pos.y
        data.extend_from_slice(&1.0f32.to_be_bytes()); // angle
        data.extend_from_slice(&100i16.to_be_bytes()); // damage
        data
    }

    #[test]
    fn test_deserialize_full() {
        let mut data = base_bytes();
        data.push(3); // numShots
        data.extend_from_slice(&15.0f32.to_be_bytes()); // angleInc

        let mut reader = PacketReader::new(&data);
        let packet = EnemyShootPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.bullet_id, 7);
        assert_eq!(packet.owner_id, 1234);
        assert_eq!(packet.bullet_type, 2);
        assert_eq!(packet.starting_pos.x, 5.0);
        assert_eq!(packet.damage, 100);
        assert_eq!(packet.num_shots, 3);
        assert_eq!(packet.angle_inc, 15.0);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_deserialize_single_shot() {
        let data = base_bytes();
        let mut reader = PacketReader::new(&data);
        let packet = EnemyShootPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.num_shots, 255);
        assert_eq!(packet.angle_inc, 0.0);
        assert!(reader.is_fully_parsed());
    }
}

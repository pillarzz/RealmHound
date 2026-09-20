//! ServerPlayerShoot packet implementation.
//!
//! Received when another player shoots.

use super::super::data::WorldPosData;
use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ServerPlayerShoot packet (ID 12) - Incoming
#[derive(Debug, Clone)]
pub struct ServerPlayerShootPacket {
    /// The id of the bullet that was produced.
    pub bullet_id: i16,
    /// The object id of the player who fired the projectile.
    pub owner_id: i32,
    /// The item id of the weapon used to fire the projectile.
    pub container_type: i32,
    /// The starting position of the projectile.
    pub starting_pos: WorldPosData,
    /// The angle at which the projectile was fired.
    pub angle: f32,
    /// The damage which will be dealt by the projectile.
    pub damage: i16,
    /// Summoner id of the summoned entity shooting.
    pub summoner_id: i32,
    /// Bullet type.
    pub bullet_type: i8,
    /// Number of bullets from the spell used.
    pub bullet_count: i8,
    /// The angle between the two neighboring bullets shot from the spell.
    pub angles_between_bullets: f32,
}

impl RotmgPacket for ServerPlayerShootPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let bullet_id = reader.read_i16()?;
        let owner_id = reader.read_i32()?;
        let container_type = reader.read_i32()?;
        let starting_pos = WorldPosData::deserialize(reader)?;
        let angle = reader.read_f32()?;
        let damage = reader.read_i16()?;
        let summoner_id = reader.read_i32()?;

        // Trailing fields are optional and arrive progressively.
        let mut bullet_type = 0i8;
        let mut bullet_count = 0i8;
        let mut angles_between_bullets = 0.0f32;
        if reader.remaining() > 0 {
            bullet_type = reader.read_byte()? as i8;
            if reader.remaining() > 0 {
                bullet_count = reader.read_byte()? as i8;
                if reader.remaining() > 0 {
                    angles_between_bullets = reader.read_f32()?;
                }
            }
        }

        Ok(Self {
            bullet_id,
            owner_id,
            container_type,
            starting_pos,
            angle,
            damage,
            summoner_id,
            bullet_type,
            bullet_count,
            angles_between_bullets,
        })
    }

    fn description(&self) -> String {
        format!(
            "ServerPlayerShoot: bulletId={} owner={} weapon={} pos=({:.1},{:.1}) angle={:.2} dmg={} summoner={} type={} count={}",
            self.bullet_id,
            self.owner_id,
            self.container_type,
            self.starting_pos.x,
            self.starting_pos.y,
            self.angle,
            self.damage,
            self.summoner_id,
            self.bullet_type,
            self.bullet_count
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
        data.extend_from_slice(&999i32.to_be_bytes()); // containerType
        data.extend_from_slice(&5.0f32.to_be_bytes()); // pos.x
        data.extend_from_slice(&6.0f32.to_be_bytes()); // pos.y
        data.extend_from_slice(&1.0f32.to_be_bytes()); // angle
        data.extend_from_slice(&100i16.to_be_bytes()); // damage
        data.extend_from_slice(&0i32.to_be_bytes()); // summonerId
        data
    }

    #[test]
    fn test_deserialize_full() {
        let mut data = base_bytes();
        data.push(2u8); // bulletType
        data.push(4u8); // bulletCount
        data.extend_from_slice(&12.0f32.to_be_bytes()); // anglesBetweenBullets

        let mut reader = PacketReader::new(&data);
        let packet = ServerPlayerShootPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.bullet_id, 7);
        assert_eq!(packet.container_type, 999);
        assert_eq!(packet.starting_pos.x, 5.0);
        assert_eq!(packet.damage, 100);
        assert_eq!(packet.bullet_type, 2);
        assert_eq!(packet.bullet_count, 4);
        assert_eq!(packet.angles_between_bullets, 12.0);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_deserialize_minimal() {
        let data = base_bytes();
        let mut reader = PacketReader::new(&data);
        let packet = ServerPlayerShootPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.bullet_type, 0);
        assert_eq!(packet.bullet_count, 0);
        assert_eq!(packet.angles_between_bullets, 0.0);
        assert!(reader.is_fully_parsed());
    }
}

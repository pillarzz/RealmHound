//! PlayerShootPacket implementation.
//!
//! Sent when the player shoots a projectile.
//!

use super::super::data::WorldPosData;
use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// PlayerShootPacket (ID 30) - Outgoing
#[derive(Debug, Clone)]
pub struct PlayerShootPacket {
    /// The current client time.
    pub time: i32,
    /// Counts the number of bullets since entering the dungeon.
    pub bullet_id: i16,
    /// The item id of the weapon used to fire the projectile.
    pub weapon_id: u16,
    /// The projectile id; may be -1 and should then be treated as 0.
    pub projectile_id: i8,
    /// The position the projectile was fired from.
    pub starting_pos: WorldPosData,
    /// The angle at which the projectile was fired.
    pub angle: f32,
    /// Whether the projectile is part of a burst weapon.
    pub is_burst: bool,
    /// Unknown.
    pub pattern_idx: i8,
    /// Unknown.
    pub attack_type: i8,
    /// Position of the player shooting.
    pub player_position: WorldPosData,
}

impl RotmgPacket for PlayerShootPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let time = reader.read_i32()?;
        let bullet_id = reader.read_i16()?;
        let weapon_id = reader.read_u16()?;
        let projectile_id = reader.read_byte()? as i8;
        let starting_pos = WorldPosData::deserialize(reader)?;
        let angle = reader.read_f32()?;
        let is_burst = reader.read_bool()?;
        let pattern_idx = reader.read_byte()? as i8;
        let attack_type = reader.read_byte()? as i8;
        let player_position = WorldPosData::deserialize(reader)?;

        Ok(Self {
            time,
            bullet_id,
            weapon_id,
            projectile_id,
            starting_pos,
            angle,
            is_burst,
            pattern_idx,
            attack_type,
            player_position,
        })
    }

    fn description(&self) -> String {
        format!(
            "PlayerShoot: bulletId={}, weaponId={}, angle={:.2}",
            self.bullet_id, self.weapon_id, self.angle
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&111i32.to_be_bytes()); // time
        data.extend_from_slice(&22i16.to_be_bytes()); // bulletId
        data.extend_from_slice(&333u16.to_be_bytes()); // weaponId
        data.push(0xFF); // projectileId = -1
        data.extend_from_slice(&1.0f32.to_be_bytes()); // startingPos.x
        data.extend_from_slice(&2.0f32.to_be_bytes()); // startingPos.y
        data.extend_from_slice(&1.57f32.to_be_bytes()); // angle
        data.push(1); // isBurst
        data.push(3); // patternIdx
        data.push(4); // attackType
        data.extend_from_slice(&5.0f32.to_be_bytes()); // playerPosition.x
        data.extend_from_slice(&6.0f32.to_be_bytes()); // playerPosition.y

        let mut reader = PacketReader::new(&data);
        let packet = PlayerShootPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.time, 111);
        assert_eq!(packet.bullet_id, 22);
        assert_eq!(packet.weapon_id, 333);
        assert_eq!(packet.projectile_id, -1);
        assert_eq!(packet.starting_pos.x, 1.0);
        assert!(packet.is_burst);
        assert_eq!(packet.pattern_idx, 3);
        assert_eq!(packet.attack_type, 4);
        assert_eq!(packet.player_position.y, 6.0);
        assert!(reader.is_fully_parsed());
    }
}

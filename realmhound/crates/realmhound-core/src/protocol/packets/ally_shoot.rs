//! AllyShoot packet implementation.
//!
//! Received when another player shoots a projectile.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// AllyShoot packet (ID 49) - Incoming
#[derive(Debug, Clone)]
pub struct AllyShootPacket {
    /// The bullet id of the projectile which was produced.
    pub bullet_id: u16,
    /// The object id of the player who fired the projectile.
    pub owner_id: i32,
    /// The item id of the weapon used to fire the projectile.
    pub container_type: i32,
    /// The angle at which the projectile was fired.
    pub angle: f32,
    /// Inspired buff increasing the range of the weapon.
    pub inspired_buff: bool,
}

impl RotmgPacket for AllyShootPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let bullet_id = reader.read_u16()?;
        let owner_id = reader.read_i32()?;
        let container_type = reader.read_i32()?;
        let angle = reader.read_f32()?;
        let inspired_buff = reader.read_bool()?;

        Ok(Self {
            bullet_id,
            owner_id,
            container_type,
            angle,
            inspired_buff,
        })
    }

    fn description(&self) -> String {
        format!(
            "AllyShoot: bulletId={} owner={} weapon={} angle={:.2} inspired={}",
            self.bullet_id, self.owner_id, self.container_type, self.angle, self.inspired_buff
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_bytes(
        bullet_id: u16,
        owner_id: i32,
        container_type: i32,
        angle: f32,
        inspired_buff: bool,
    ) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&bullet_id.to_be_bytes());
        data.extend_from_slice(&owner_id.to_be_bytes());
        data.extend_from_slice(&container_type.to_be_bytes());
        data.extend_from_slice(&angle.to_be_bytes());
        data.push(inspired_buff as u8);
        data
    }

    #[test]
    fn test_deserialize() {
        let data = build_bytes(123, 456, 789, 1.5, true);
        let mut reader = PacketReader::new(&data);
        let packet = AllyShootPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.bullet_id, 123);
        assert_eq!(packet.owner_id, 456);
        assert_eq!(packet.container_type, 789);
        assert_eq!(packet.angle, 1.5);
        assert!(packet.inspired_buff);
        assert!(reader.is_fully_parsed());
    }
}

//! OtherHitPacket implementation.
//!
//! Sent when an object or other player has been hit by an enemy projectile.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// OtherHitPacket (ID 20) - Outgoing
#[derive(Debug, Clone)]
pub struct OtherHitPacket {
    /// The current client time.
    pub time: i32,
    /// The id of the bullet which hit the object.
    pub bullet_id: i16,
    /// The object id of the player who fired the projectile.
    pub object_id: i32,
    /// The object id of the object which was hit.
    pub target_id: i32,
}

impl RotmgPacket for OtherHitPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let time = reader.read_i32()?;
        let bullet_id = reader.read_i16()?;
        let object_id = reader.read_i32()?;
        let target_id = reader.read_i32()?;

        Ok(Self {
            time,
            bullet_id,
            object_id,
            target_id,
        })
    }

    fn description(&self) -> String {
        format!(
            "OtherHit: bulletId={}, object={}, target={}",
            self.bullet_id, self.object_id, self.target_id
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&10i32.to_be_bytes());
        data.extend_from_slice(&20i16.to_be_bytes());
        data.extend_from_slice(&30i32.to_be_bytes());
        data.extend_from_slice(&40i32.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = OtherHitPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.time, 10);
        assert_eq!(packet.bullet_id, 20);
        assert_eq!(packet.object_id, 30);
        assert_eq!(packet.target_id, 40);
        assert!(reader.is_fully_parsed());
    }
}

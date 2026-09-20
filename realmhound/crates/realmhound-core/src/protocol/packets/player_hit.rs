//! PlayerHitPacket implementation.
//!
//! Sent when the player is hit.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// PlayerHitPacket (ID 90) - Outgoing
#[derive(Debug, Clone)]
pub struct PlayerHitPacket {
    /// The id of the bullet which hit the player.
    pub bullet_id: i16,
    /// The object id of the enemy that hit the player.
    pub object_id: i32,
}

impl RotmgPacket for PlayerHitPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let bullet_id = reader.read_i16()?;
        let object_id = reader.read_i32()?;

        Ok(Self {
            bullet_id,
            object_id,
        })
    }

    fn description(&self) -> String {
        format!(
            "PlayerHit: bulletId={}, object={}",
            self.bullet_id, self.object_id
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&20i16.to_be_bytes());
        data.extend_from_slice(&30i32.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = PlayerHitPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.bullet_id, 20);
        assert_eq!(packet.object_id, 30);
        assert!(reader.is_fully_parsed());
    }
}

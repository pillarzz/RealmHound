//! CreepHitPacket implementation.
//!
//! Sent when a Summoner's creep hits a target.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// CreepHitPacket (ID 133) - Outgoing
#[derive(Debug, Clone)]
pub struct CreepHitPacket {
    /// The current client time.
    pub time: i32,
    /// The id of the bullet which hit the target.
    pub bullet_id: i16,
    /// The id of the object shooting.
    pub object_id: i32,
    /// The object id of the target which was hit.
    pub target_id: i32,
}

impl RotmgPacket for CreepHitPacket {
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
            "CreepHit: bulletId={}, object={}, target={}",
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
        let packet = CreepHitPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.time, 10);
        assert_eq!(packet.bullet_id, 20);
        assert_eq!(packet.object_id, 30);
        assert_eq!(packet.target_id, 40);
        assert!(reader.is_fully_parsed());
    }
}

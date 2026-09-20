//! PlaySound packet implementation.
//!
//! Received to tell the client to play a sound.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// PlaySound packet (ID 38) - Incoming
#[derive(Debug, Clone)]
pub struct PlaySoundPacket {
    /// The object id of the origin of the sound.
    pub owner_id: i32,
    /// The id of the sound to play.
    pub sound_id: u8,
}

impl RotmgPacket for PlaySoundPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let owner_id = reader.read_i32()?;
        let sound_id = reader.read_byte()?;

        Ok(Self { owner_id, sound_id })
    }

    fn description(&self) -> String {
        format!(
            "PlaySound: ownerId={} soundId={}",
            self.owner_id, self.sound_id
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&555i32.to_be_bytes()); // ownerId
        data.push(7); // soundId

        let mut reader = PacketReader::new(&data);
        let packet = PlaySoundPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.owner_id, 555);
        assert_eq!(packet.sound_id, 7);
        assert!(reader.is_fully_parsed());
    }
}

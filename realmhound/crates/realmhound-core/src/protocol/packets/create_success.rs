//! CreateSuccess packet implementation.
//!
//! Received when the player successfully enters a map/area.
//! Signals that the player is fully loaded and ready.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// CreateSuccess packet (ID 101) - Incoming
///
/// Sent by server when player successfully loads into a map.
/// This is the signal to trigger character list API calls in Pet Yard or Daily Quest Room.
#[derive(Debug, Clone)]
pub struct CreateSuccessPacket {
    /// The object ID of the player's character in this map
    pub object_id: i32,
    /// The character ID of the player's character
    pub char_id: i32,
    /// PCStats string (encoded character statistics)
    pub pc_stats: String,
}

impl RotmgPacket for CreateSuccessPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let object_id = reader.read_i32()?;
        let char_id = reader.read_i32()?;
        let pc_stats = reader.read_string()?;

        Ok(Self {
            object_id,
            char_id,
            pc_stats,
        })
    }

    fn description(&self) -> String {
        format!(
            "CreateSuccess: charId={}, objectId={}",
            self.char_id, self.object_id
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_success_deserialize() {
        let mut data = Vec::new();

        // object_id: i32
        data.extend_from_slice(&42i32.to_be_bytes());
        // char_id: i32
        data.extend_from_slice(&12345i32.to_be_bytes());
        // pc_stats: string
        let stats = "ABC123";
        data.extend_from_slice(&(stats.len() as u16).to_be_bytes());
        data.extend_from_slice(stats.as_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = CreateSuccessPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.object_id, 42);
        assert_eq!(packet.char_id, 12345);
        assert_eq!(packet.pc_stats, "ABC123");
    }

    #[test]
    fn test_create_success_description() {
        let mut data = Vec::new();
        data.extend_from_slice(&100i32.to_be_bytes());
        data.extend_from_slice(&999i32.to_be_bytes());
        data.extend_from_slice(&0u16.to_be_bytes()); // empty string

        let mut reader = PacketReader::new(&data);
        let packet = CreateSuccessPacket::deserialize(&mut reader).unwrap();

        assert_eq!(
            packet.description(),
            "CreateSuccess: charId=999, objectId=100"
        );
    }
}

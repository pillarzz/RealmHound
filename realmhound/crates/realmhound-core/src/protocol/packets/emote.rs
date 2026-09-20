//! EmotePacket implementation.
//!
//! Sent when the player uses an emote.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// EmotePacket (ID 159) - Outgoing
#[derive(Debug, Clone)]
pub struct EmotePacket {
    /// The emote id.
    pub emote_id: i32,
    /// The current client time.
    pub emote_time: i32,
    /// Unknown trailing byte.
    pub unknown_byte: i8,
}

impl RotmgPacket for EmotePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let emote_id = reader.read_i32()?;
        let emote_time = reader.read_i32()?;
        let unknown_byte = reader.read_byte()? as i8;

        Ok(Self {
            emote_id,
            emote_time,
            unknown_byte,
        })
    }

    fn description(&self) -> String {
        format!("Emote: id={}, time={}", self.emote_id, self.emote_time)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = [0u8, 0, 0, 5, 0, 0, 0, 100, 0];
        let mut reader = PacketReader::new(&data);
        let packet = EmotePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.emote_id, 5);
        assert_eq!(packet.emote_time, 100);
        assert_eq!(packet.unknown_byte, 0);
        assert!(reader.is_fully_parsed());
    }
}

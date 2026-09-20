//! LoadPacket implementation.
//!
//! Sent in response to a `MapInfoPacket` to load a character into the map.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// LoadPacket (ID 61) - Outgoing
#[derive(Debug, Clone)]
pub struct LoadPacket {
    /// The id of the character to load.
    pub char_id: i32,
    /// Whether the load is from an arena.
    pub is_from_arena: bool,
}

impl RotmgPacket for LoadPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let char_id = reader.read_i32()?;
        let is_from_arena = reader.read_bool()?;

        Ok(Self {
            char_id,
            is_from_arena,
        })
    }

    fn description(&self) -> String {
        format!(
            "Load: charId={}, isFromArena={}",
            self.char_id, self.is_from_arena
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = [0u8, 0, 0, 5, 1];
        let mut reader = PacketReader::new(&data);
        let packet = LoadPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.char_id, 5);
        assert!(packet.is_from_arena);
        assert!(reader.is_fully_parsed());
    }
}

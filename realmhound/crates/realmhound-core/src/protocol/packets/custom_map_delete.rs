//! CustomMapDeletePacket implementation.
//!
//! Sent to delete a custom map.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// CustomMapDeletePacket (ID 129) - Outgoing
#[derive(Debug, Clone)]
pub struct CustomMapDeletePacket {
    /// The id of the custom map to delete.
    pub game_id: i32,
}

impl RotmgPacket for CustomMapDeletePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let game_id = reader.read_compressed_int()?;

        Ok(Self { game_id })
    }

    fn description(&self) -> String {
        format!("CustomMapDelete: gameId={}", self.game_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        // compressed int 42 = single byte 42
        let data = [42u8];
        let mut reader = PacketReader::new(&data);
        let packet = CustomMapDeletePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.game_id, 42);
        assert!(reader.is_fully_parsed());
    }
}

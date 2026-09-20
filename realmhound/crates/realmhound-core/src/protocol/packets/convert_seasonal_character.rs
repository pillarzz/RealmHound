//! ConvertSeasonalCharacter packet implementation.
//!
//! Sent when converting a seasonal character to a regular character.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ConvertSeasonalCharacter packet (ID 154) - Outgoing (empty)
#[derive(Debug, Clone)]
pub struct ConvertSeasonalCharacterPacket;

impl RotmgPacket for ConvertSeasonalCharacterPacket {
    fn deserialize(_reader: &mut PacketReader) -> io::Result<Self> {
        Ok(Self)
    }

    fn description(&self) -> String {
        "ConvertSeasonalCharacter".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut reader = PacketReader::new(&[]);
        let packet = ConvertSeasonalCharacterPacket::deserialize(&mut reader).unwrap();
        assert_eq!(packet.description(), "ConvertSeasonalCharacter");
        assert!(reader.is_fully_parsed());
    }
}

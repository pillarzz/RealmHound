//! GetPlayersListPacket implementation.
//!
//! Despite the name, the packet also carries class/skin/challenger fields.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// GetPlayersListPacket (ID 123) - Outgoing
#[derive(Debug, Clone)]
pub struct GetPlayersListPacket {
    /// The class to use for the new character.
    pub class_type: i16,
    /// The skin id (default `0`).
    pub skin_type: i16,
    /// Whether the character is in challenger mode.
    pub is_challenger: bool,
}

impl RotmgPacket for GetPlayersListPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let class_type = reader.read_i16()?;
        let skin_type = reader.read_i16()?;
        let is_challenger = reader.read_bool()?;

        Ok(Self {
            class_type,
            skin_type,
            is_challenger,
        })
    }

    fn description(&self) -> String {
        format!(
            "GetPlayersList: class={}, skin={}, challenger={}",
            self.class_type, self.skin_type, self.is_challenger
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = [0u8, 10, 0, 0, 1];
        let mut reader = PacketReader::new(&data);
        let packet = GetPlayersListPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.class_type, 10);
        assert_eq!(packet.skin_type, 0);
        assert!(packet.is_challenger);
        assert!(reader.is_fully_parsed());
    }
}

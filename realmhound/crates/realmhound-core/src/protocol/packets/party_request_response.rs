//! PartyRequestResponse packet implementation.
//!
//! Received in response to a party join request, carrying the requesting
//! player's info and the invite state.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Returns the human-readable name of an invite state ordinal, if known.
pub fn invite_state_name(state: u8) -> Option<&'static str> {
    match state {
        0 => Some("None"),
        1 => Some("Pending"),
        2 => Some("Cancelled"),
        3 => Some("Accepted"),
        4 => Some("Declined"),
        5 => Some("PartyFull"),
        6 => Some("Blacklisted"),
        _ => None,
    }
}

/// PartyRequestResponse packet (ID 217) - Incoming
#[derive(Debug, Clone)]
pub struct PartyRequestResponsePacket {
    /// The name of the player the response concerns.
    pub player_name: String,
    /// The player's character class id.
    pub class_id: i16,
    /// The player's skin id.
    pub skin_id: i16,
    /// The raw invite state ordinal (see [`invite_state_name`]).
    pub state: u8,
}

impl PartyRequestResponsePacket {
    /// Returns the human-readable name of the invite state, if known.
    pub fn state_name(&self) -> Option<&'static str> {
        invite_state_name(self.state)
    }
}

impl RotmgPacket for PartyRequestResponsePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let player_name = reader.read_string()?;
        let class_id = reader.read_i16()?;
        let skin_id = reader.read_i16()?;
        let state = reader.read_byte()?;

        Ok(Self {
            player_name,
            class_id,
            skin_id,
            state,
        })
    }

    fn description(&self) -> String {
        let state = self
            .state_name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| self.state.to_string());
        format!(
            "PartyRequestResponse: '{}' state={}",
            self.player_name, state
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_bytes(name: &str, class_id: i16, skin_id: i16, state: u8) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&(name.len() as u16).to_be_bytes());
        data.extend_from_slice(name.as_bytes());
        data.extend_from_slice(&class_id.to_be_bytes());
        data.extend_from_slice(&skin_id.to_be_bytes());
        data.push(state);
        data
    }

    #[test]
    fn test_deserialize() {
        let data = build_bytes("Bob", 768, 1, 3);
        let mut reader = PacketReader::new(&data);
        let packet = PartyRequestResponsePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.player_name, "Bob");
        assert_eq!(packet.class_id, 768);
        assert_eq!(packet.skin_id, 1);
        assert_eq!(packet.state_name(), Some("Accepted"));
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_unknown_state() {
        let data = build_bytes("X", 0, 0, 99);
        let mut reader = PacketReader::new(&data);
        let packet = PartyRequestResponsePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.state, 99);
        assert_eq!(packet.state_name(), None);
        assert_eq!(packet.description(), "PartyRequestResponse: 'X' state=99");
    }
}

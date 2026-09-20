//! PartyActionResult packet implementation.
//!
//! Sent to report a party action (kick, promote, leave).
//! Direction verified from live TCP capture.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Returns the human-readable name of a party action id, if known.
pub fn party_action_name(action_id: u8) -> Option<&'static str> {
    match action_id {
        0 => Some("None"),
        1 => Some("Failed"),
        2 => Some("Kicked"),
        3 => Some("KickNotFound"),
        4 => Some("PromotedToLeader"),
        5 => Some("PromoteNotFound"),
        6 => Some("LeftParty"),
        _ => None,
    }
}

/// PartyActionResult packet (ID 204) - Outgoing
#[derive(Debug, Clone)]
pub struct PartyActionResultPacket {
    /// The player the action applied to.
    pub player_id: i16,
    /// The raw party action type ordinal (see [`party_action_name`]).
    pub action_id: u8,
}

impl PartyActionResultPacket {
    /// Returns the human-readable name of the action, if known.
    pub fn action_name(&self) -> Option<&'static str> {
        party_action_name(self.action_id)
    }
}

impl RotmgPacket for PartyActionResultPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let player_id = reader.read_i16()?;
        let action_id = reader.read_byte()?;

        Ok(Self {
            player_id,
            action_id,
        })
    }

    fn description(&self) -> String {
        match self.action_name() {
            Some(name) => format!(
                "PartyActionResult: player={} action={}",
                self.player_id, name
            ),
            None => format!(
                "PartyActionResult: player={} action={}",
                self.player_id, self.action_id
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_bytes(player_id: i16, action_id: u8) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&player_id.to_be_bytes());
        data.push(action_id);
        data
    }

    #[test]
    fn test_deserialize() {
        let data = build_bytes(1234, 2);
        let mut reader = PacketReader::new(&data);
        let packet = PartyActionResultPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.player_id, 1234);
        assert_eq!(packet.action_id, 2);
        assert_eq!(packet.action_name(), Some("Kicked"));
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_negative_player_id_preserved() {
        let data = build_bytes(-2, 6);
        let mut reader = PacketReader::new(&data);
        let packet = PartyActionResultPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.player_id, -2);
        assert_eq!(packet.action_name(), Some("LeftParty"));
    }

    #[test]
    fn test_unknown_action() {
        let data = build_bytes(0, 200);
        let mut reader = PacketReader::new(&data);
        let packet = PartyActionResultPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.action_id, 200);
        assert_eq!(packet.action_name(), None);
        assert_eq!(
            packet.description(),
            "PartyActionResult: player=0 action=200"
        );
    }
}

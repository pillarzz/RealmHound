//! PartyActionPacket implementation.
//!
//! Received when the server applies a party action (kick, promote, leave).
//!
//! Direction verified from live TCP capture.

use super::party_action_result::party_action_name;
use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// PartyActionPacket (ID 207) - Incoming
#[derive(Debug, Clone)]
pub struct PartyActionPacket {
    /// The player the action targets.
    pub player_id: i16,
    /// The raw party action type ordinal (see [`party_action_name`]).
    pub action_id: u8,
}

impl PartyActionPacket {
    /// Returns the human-readable name of the action, if known.
    pub fn action_name(&self) -> Option<&'static str> {
        party_action_name(self.action_id)
    }
}

impl RotmgPacket for PartyActionPacket {
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
            Some(name) => format!("PartyAction: player={}, action={}", self.player_id, name),
            None => format!(
                "PartyAction: player={}, action={}",
                self.player_id, self.action_id
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&1234i16.to_be_bytes());
        data.push(2);

        let mut reader = PacketReader::new(&data);
        let packet = PartyActionPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.player_id, 1234);
        assert_eq!(packet.action_id, 2);
        assert_eq!(packet.action_name(), Some("Kicked"));
        assert!(reader.is_fully_parsed());
    }
}

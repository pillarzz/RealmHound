//! IncomingPartyInvite packet implementation.
//!
//! Received when another player invites you to their party.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// IncomingPartyInvite packet (ID 208) - Incoming
///
/// Sent by the server when the local player receives a party invitation.
#[derive(Debug, Clone)]
pub struct IncomingPartyInvitePacket {
    /// The ID of the party the invite is for.
    pub party_id: i32,
    /// The name of the player who sent the invite.
    pub inviter_name: String,
}

impl RotmgPacket for IncomingPartyInvitePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let party_id = reader.read_i32()?;
        let inviter_name = reader.read_string()?;

        Ok(Self {
            party_id,
            inviter_name,
        })
    }

    fn description(&self) -> String {
        format!(
            "IncomingPartyInvite: '{}' invited you to party {}",
            self.inviter_name, self.party_id
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_bytes(party_id: i32, inviter: &str) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&party_id.to_be_bytes());
        data.extend_from_slice(&(inviter.len() as u16).to_be_bytes());
        data.extend_from_slice(inviter.as_bytes());
        data
    }

    #[test]
    fn test_deserialize() {
        let data = build_bytes(987654, "Alice");
        let mut reader = PacketReader::new(&data);
        let packet = IncomingPartyInvitePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.party_id, 987654);
        assert_eq!(packet.inviter_name, "Alice");
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_empty_name() {
        let data = build_bytes(0, "");
        let mut reader = PacketReader::new(&data);
        let packet = IncomingPartyInvitePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.inviter_name, "");
    }
}

//! PartyInviteResponsePacket implementation.
//!
//! Sent in response to a party invite.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// PartyInviteResponsePacket (ID 209) - Outgoing
#[derive(Debug, Clone)]
pub struct PartyInviteResponsePacket {
    /// The id of the party invited to.
    pub party_id: i32,
    /// Whether the invite was accepted (non-zero) or declined.
    pub accept_invite: i8,
}

impl RotmgPacket for PartyInviteResponsePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let party_id = reader.read_i32()?;
        let accept_invite = reader.read_byte()? as i8;

        Ok(Self {
            party_id,
            accept_invite,
        })
    }

    fn description(&self) -> String {
        format!(
            "PartyInviteResponse: partyId={}, accept={}",
            self.party_id, self.accept_invite
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&55i32.to_be_bytes());
        data.push(1);

        let mut reader = PacketReader::new(&data);
        let packet = PartyInviteResponsePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.party_id, 55);
        assert_eq!(packet.accept_invite, 1);
        assert!(reader.is_fully_parsed());
    }
}

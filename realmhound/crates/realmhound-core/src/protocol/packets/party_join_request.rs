//! PartyJoinRequest packet implementation.
//!
//! Sent when the client requests to join a party.
//! Direction verified from live TCP capture.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// PartyJoinRequest packet (ID 215) - Outgoing
#[derive(Debug, Clone)]
pub struct PartyJoinRequestPacket {
    /// The ID of the party being requested to join.
    pub party_id: i32,
    /// Unknown trailing byte (preserved verbatim).
    pub unknown: u8,
}

impl RotmgPacket for PartyJoinRequestPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let party_id = reader.read_i32()?;
        let unknown = reader.read_byte()?;

        Ok(Self { party_id, unknown })
    }

    fn description(&self) -> String {
        format!("PartyJoinRequest: party={}", self.party_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&42i32.to_be_bytes());
        data.push(0xFF);
        let mut reader = PacketReader::new(&data);
        let packet = PartyJoinRequestPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.party_id, 42);
        assert_eq!(packet.unknown, 255);
        assert!(reader.is_fully_parsed());
    }
}

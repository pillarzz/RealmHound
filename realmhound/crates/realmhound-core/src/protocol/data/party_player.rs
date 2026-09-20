//! Party player data used in party-related packets.

use crate::protocol::PacketReader;
use std::io;

/// Data about a single party member.
///
/// Used in `IncomingPartyMemberInfoPacket` to describe each player in the party.
#[derive(Debug, Clone)]
pub struct PartyPlayerData {
    /// Player's internal ID
    pub id: i16,
    /// Player's name
    pub name: String,
    /// Player's object ID in the game world
    pub object_id: i16,
    /// Unknown field
    pub unknown: i16,
}

impl PartyPlayerData {
    /// Deserialize party player data from a packet reader.
    pub fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let id = reader.read_i16()?;
        let name = reader.read_string()?;
        let object_id = reader.read_i16()?;
        let unknown = reader.read_i16()?;

        Ok(Self {
            id,
            name,
            object_id,
            unknown,
        })
    }
}

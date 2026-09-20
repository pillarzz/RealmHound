//! IncomingPartyMemberInfoPacket implementation.
//!
//! Received when party member information is updated (joining a party, member changes).

use super::traits::RotmgPacket;
use super::PacketReader;
use crate::protocol::data::PartyPlayerData;
use std::io;

/// IncomingPartyMemberInfoPacket (ID 210) - Incoming
///
/// Sent by server with full party member list and party information.
/// This packet contains the party name (description) and all current members.
#[derive(Debug, Clone)]
pub struct IncomingPartyMemberInfoPacket {
    /// Party ID
    pub party_id: i32,
    /// Player ID of the party leader
    pub leader_id: i16,
    /// Maximum party size
    pub max_size: u8,
    /// List of party members (first member is typically the leader)
    pub party_players: Vec<PartyPlayerData>,
    /// Party description/name set by the party leader
    pub description: String,
}

impl RotmgPacket for IncomingPartyMemberInfoPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let party_id = reader.read_i32()?;
        let leader_id = reader.read_i16()?;
        let max_size = reader.read_byte()?;

        // Read party players array
        let players_len = reader.read_i16()?;
        if players_len < 0 || players_len > 200 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid party players count: {}", players_len),
            ));
        }
        let players_len = players_len as usize;
        let mut party_players = Vec::with_capacity(players_len);
        for _ in 0..players_len {
            party_players.push(PartyPlayerData::deserialize(reader)?);
        }

        let description = reader.read_string()?;

        Ok(Self {
            party_id,
            leader_id,
            max_size,
            party_players,
            description,
        })
    }

    fn description(&self) -> String {
        format!(
            "PartyMemberInfo: {} players in '{}' (id={})",
            self.party_players.len(),
            self.description,
            self.party_id
        )
    }
}

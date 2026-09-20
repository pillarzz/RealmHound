//! Party listing data used in party browser packets.

use crate::protocol::PacketReader;
use std::io;

/// Data about a single party in the party browser/listing.
///
/// Used in `PartyListMessagePacket` (ID 214).
#[derive(Debug, Clone)]
pub struct PartyData {
    /// Party description/name set by the leader.
    pub description: String,
    /// Party ID.
    pub party_id: i32,
    /// Party power level.
    pub power_level: i16,
    /// Current number of players in the party.
    pub current_players: u8,
    /// Maximum number of players allowed.
    pub max_players: u8,
    /// Party type.
    pub party_type: u8,
    /// Privacy setting (1 = public, 2 = private).
    pub privacy: u8,
    /// Required maxed stats to join.
    pub required_maxed_stats: u8,
    /// Server identifier.
    pub server: u8,
}

impl PartyData {
    /// Deserialize party listing data from a packet reader.
    pub fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let description = reader.read_string()?;
        let party_id = reader.read_i32()?;
        let power_level = reader.read_i16()?;
        let current_players = reader.read_byte()?;
        let max_players = reader.read_byte()?;
        let party_type = reader.read_byte()?;
        let privacy = reader.read_byte()?;
        let required_maxed_stats = reader.read_byte()?;
        let server = reader.read_byte()?;

        Ok(Self {
            description,
            party_id,
            power_level,
            current_players,
            max_players,
            party_type,
            privacy,
            required_maxed_stats,
            server,
        })
    }
}

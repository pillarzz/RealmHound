//! PartyMemberAddedPacket implementation.
//!
//! Received when a new player joins the party.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// PartyMemberAddedPacket (ID 212) - Incoming
///
/// Sent by server when a new player joins the party.
#[derive(Debug, Clone)]
pub struct PartyMemberAddedPacket {
    /// Player ID of the new member
    pub player_id: i16,
    /// Name of the new member
    pub name: String,
    /// Class ID of the new member's character
    pub class_id: i16,
    /// Skin ID of the new member's character
    pub skin_id: i16,
}

impl RotmgPacket for PartyMemberAddedPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let player_id = reader.read_i16()?;
        let name = reader.read_string()?;
        let class_id = reader.read_i16()?;
        let skin_id = reader.read_i16()?;

        Ok(Self {
            player_id,
            name,
            class_id,
            skin_id,
        })
    }

    fn description(&self) -> String {
        format!(
            "PartyMemberAdded: {} (id={}, class={})",
            self.name, self.player_id, self.class_id
        )
    }
}

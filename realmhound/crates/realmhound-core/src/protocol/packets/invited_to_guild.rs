//! InvitedToGuild packet implementation.
//!
//! Received when the player is invited to a guild.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// InvitedToGuild packet (ID 77) - Incoming
#[derive(Debug, Clone)]
pub struct InvitedToGuildPacket {
    /// The name of the player who sent the invite.
    pub name: String,
    /// The name of the guild which the invite is for.
    pub guild_name: String,
}

impl RotmgPacket for InvitedToGuildPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let name = reader.read_string()?;
        let guild_name = reader.read_string()?;
        Ok(Self { name, guild_name })
    }

    fn description(&self) -> String {
        format!(
            "InvitedToGuild: from={} guild={}",
            self.name, self.guild_name
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_string(data: &mut Vec<u8>, s: &str) {
        data.extend_from_slice(&(s.len() as u16).to_be_bytes());
        data.extend_from_slice(s.as_bytes());
    }

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        push_string(&mut data, "Bob");
        push_string(&mut data, "Knights");

        let mut reader = PacketReader::new(&data);
        let packet = InvitedToGuildPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.name, "Bob");
        assert_eq!(packet.guild_name, "Knights");
        assert!(reader.is_fully_parsed());
    }
}

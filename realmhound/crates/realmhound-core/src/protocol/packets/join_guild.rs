//! JoinGuildPacket implementation.
//!
//! Sent to accept a pending guild invite.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// JoinGuildPacket (ID 7) - Outgoing
#[derive(Debug, Clone)]
pub struct JoinGuildPacket {
    /// The name of the guild for which there is a pending invite.
    pub guild_name: String,
}

impl RotmgPacket for JoinGuildPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let guild_name = reader.read_string()?;
        Ok(Self { guild_name })
    }

    fn description(&self) -> String {
        format!("JoinGuild: guildName={}", self.guild_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&4u16.to_be_bytes());
        data.extend_from_slice(b"Clan");

        let mut reader = PacketReader::new(&data);
        let packet = JoinGuildPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.guild_name, "Clan");
        assert!(reader.is_fully_parsed());
    }
}

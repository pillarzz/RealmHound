//! GuildInvitePacket implementation.
//!
//! Sent to invite a player to the client's current guild.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// GuildInvitePacket (ID 104) - Outgoing
#[derive(Debug, Clone)]
pub struct GuildInvitePacket {
    /// The name of the player to invite.
    pub name: String,
}

impl RotmgPacket for GuildInvitePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let name = reader.read_string()?;
        Ok(Self { name })
    }

    fn description(&self) -> String {
        format!("GuildInvite: name={}", self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&5u16.to_be_bytes());
        data.extend_from_slice(b"Carol");

        let mut reader = PacketReader::new(&data);
        let packet = GuildInvitePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.name, "Carol");
        assert!(reader.is_fully_parsed());
    }
}

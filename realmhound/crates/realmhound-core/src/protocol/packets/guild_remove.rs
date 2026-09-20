//! GuildRemovePacket implementation.
//!
//! Sent to remove a player from the client's current guild.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// GuildRemovePacket (ID 15) - Outgoing
#[derive(Debug, Clone)]
pub struct GuildRemovePacket {
    /// The name of the player to remove.
    pub name: String,
}

impl RotmgPacket for GuildRemovePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let name = reader.read_string()?;
        Ok(Self { name })
    }

    fn description(&self) -> String {
        format!("GuildRemove: name={}", self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&3u16.to_be_bytes());
        data.extend_from_slice(b"Bob");

        let mut reader = PacketReader::new(&data);
        let packet = GuildRemovePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.name, "Bob");
        assert!(reader.is_fully_parsed());
    }
}

//! CreateGuildPacket implementation.
//!
//! Sent to create a new guild.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// CreateGuildPacket (ID 59) - Outgoing
#[derive(Debug, Clone)]
pub struct CreateGuildPacket {
    /// The name of the guild being created.
    pub name: String,
}

impl RotmgPacket for CreateGuildPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let name = reader.read_string()?;
        Ok(Self { name })
    }

    fn description(&self) -> String {
        format!("CreateGuild: name={}", self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&6u16.to_be_bytes());
        data.extend_from_slice(b"Wizard");

        let mut reader = PacketReader::new(&data);
        let packet = CreateGuildPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.name, "Wizard");
        assert!(reader.is_fully_parsed());
    }
}

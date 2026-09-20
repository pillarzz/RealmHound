//! KeyInfoResponse packet implementation.
//!
//! Received with metadata about a dungeon key.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// KeyInfoResponse packet (ID 63) - Incoming
#[derive(Debug, Clone)]
pub struct KeyInfoResponsePacket {
    /// The key/dungeon name.
    pub name: String,
    /// The description text.
    pub description: String,
    /// The creator credit.
    pub creator: String,
}

impl RotmgPacket for KeyInfoResponsePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let name = reader.read_string()?;
        let description = reader.read_string()?;
        let creator = reader.read_string()?;
        Ok(Self {
            name,
            description,
            creator,
        })
    }

    fn description(&self) -> String {
        format!(
            "KeyInfoResponse: name={} creator={}",
            self.name, self.creator
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
        push_string(&mut data, "Oryx's Castle");
        push_string(&mut data, "A key to the castle");
        push_string(&mut data, "DECA");

        let mut reader = PacketReader::new(&data);
        let packet = KeyInfoResponsePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.name, "Oryx's Castle");
        assert_eq!(packet.description, "A key to the castle");
        assert_eq!(packet.creator, "DECA");
        assert!(reader.is_fully_parsed());
    }
}

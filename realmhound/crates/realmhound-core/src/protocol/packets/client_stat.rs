//! ClientStat packet implementation.
//!
//! Received to inform the player about one of their account/character stats
//! (e.g. fame totals, counters).

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ClientStat packet (ID 69) - Incoming
#[derive(Debug, Clone)]
pub struct ClientStatPacket {
    /// The name of the stat.
    pub name: String,
    /// The value of the stat.
    pub value: i32,
}

impl RotmgPacket for ClientStatPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let name = reader.read_string()?;
        let value = reader.read_i32()?;

        Ok(Self { name, value })
    }

    fn description(&self) -> String {
        format!("ClientStat: {}={}", self.name, self.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_bytes(name: &str, value: i32) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&(name.len() as u16).to_be_bytes());
        data.extend_from_slice(name.as_bytes());
        data.extend_from_slice(&value.to_be_bytes());
        data
    }

    #[test]
    fn test_deserialize() {
        let data = build_bytes("FameStat", 12345);
        let mut reader = PacketReader::new(&data);
        let packet = ClientStatPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.name, "FameStat");
        assert_eq!(packet.value, 12345);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_description() {
        let data = build_bytes("Kills", 7);
        let mut reader = PacketReader::new(&data);
        let packet = ClientStatPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.description(), "ClientStat: Kills=7");
    }
}

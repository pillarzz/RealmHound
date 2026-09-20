//! TradeRequested packet implementation.
//!
//! Received when a trade is requested.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// TradeRequested packet (ID 88) - Incoming
#[derive(Debug, Clone)]
pub struct TradeRequestedPacket {
    /// The name of the player who requested the trade.
    pub name: String,
}

impl RotmgPacket for TradeRequestedPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let name = reader.read_string()?;
        Ok(Self { name })
    }

    fn description(&self) -> String {
        format!("TradeRequested: name={}", self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        let name = "Alice";
        data.extend_from_slice(&(name.len() as u16).to_be_bytes());
        data.extend_from_slice(name.as_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = TradeRequestedPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.name, "Alice");
        assert!(reader.is_fully_parsed());
    }
}

//! RequestTradePacket implementation.
//!
//! Sent to request a trade with a player, as well as to accept a pending trade.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// RequestTradePacket (ID 5) - Outgoing
#[derive(Debug, Clone)]
pub struct RequestTradePacket {
    /// The name of the player to request the trade with.
    pub name: String,
}

impl RotmgPacket for RequestTradePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let name = reader.read_string()?;
        Ok(Self { name })
    }

    fn description(&self) -> String {
        format!("RequestTrade: name={}", self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&5u16.to_be_bytes());
        data.extend_from_slice(b"Alice");

        let mut reader = PacketReader::new(&data);
        let packet = RequestTradePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.name, "Alice");
        assert!(reader.is_fully_parsed());
    }
}

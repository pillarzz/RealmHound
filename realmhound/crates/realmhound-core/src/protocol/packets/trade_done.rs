//! TradeDone packet implementation.
//!
//! Received when the active trade has completed, regardless of whether it was
//! accepted or cancelled.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// The result of a completed trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeResult {
    /// Trade completed successfully (code 0).
    Successful,
    /// A player cancelled the trade (code 1).
    PlayerCancelled,
    /// Unrecognized result code.
    Unknown(i32),
}

impl TradeResult {
    fn from_code(code: i32) -> Self {
        match code {
            0 => TradeResult::Successful,
            1 => TradeResult::PlayerCancelled,
            other => TradeResult::Unknown(other),
        }
    }
}

/// TradeDone packet (ID 34) - Incoming
#[derive(Debug, Clone)]
pub struct TradeDonePacket {
    /// The result of the trade.
    pub code: TradeResult,
    /// Unknown description string.
    pub description: String,
}

impl RotmgPacket for TradeDonePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let code = TradeResult::from_code(reader.read_i32()?);
        let description = reader.read_string()?;
        Ok(Self { code, description })
    }

    fn description(&self) -> String {
        format!("TradeDone: code={:?} desc={}", self.code, self.description)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_successful() {
        let mut data = Vec::new();
        data.extend_from_slice(&0i32.to_be_bytes());
        let desc = "ok";
        data.extend_from_slice(&(desc.len() as u16).to_be_bytes());
        data.extend_from_slice(desc.as_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = TradeDonePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.code, TradeResult::Successful);
        assert_eq!(packet.description, "ok");
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_deserialize_unknown_code() {
        let mut data = Vec::new();
        data.extend_from_slice(&7i32.to_be_bytes());
        data.extend_from_slice(&0u16.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = TradeDonePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.code, TradeResult::Unknown(7));
        assert!(reader.is_fully_parsed());
    }
}

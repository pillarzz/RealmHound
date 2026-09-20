//! For-reconnect packet implementation.
//!
//! Received carrying an opaque reconnect token used when the client is
//! instructed to reconnect.
//!
//! Note: this is distinct from the `Reconnect` packet (ID 45), which carries a
//! full host/port/key payload. This packet (ID 218) carries only a single
//! string.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// For-reconnect packet (ID 218) - Incoming
///
/// Carries an opaque reconnect-info string (a 16-bit length-prefixed string).
#[derive(Debug, Clone)]
pub struct ForReconnectPacket {
    /// Opaque reconnect information token.
    pub reconnect_info: String,
}

impl RotmgPacket for ForReconnectPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let reconnect_info = reader.read_string()?;

        Ok(Self { reconnect_info })
    }

    fn description(&self) -> String {
        format!("ForReconnect: {}", self.reconnect_info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_bytes(info: &str) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&(info.len() as u16).to_be_bytes());
        data.extend_from_slice(info.as_bytes());
        data
    }

    #[test]
    fn test_for_reconnect_deserialize() {
        let data = build_bytes("token-abc-123");
        let mut reader = PacketReader::new(&data);
        let packet = ForReconnectPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.reconnect_info, "token-abc-123");
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_for_reconnect_empty() {
        let data = build_bytes("");
        let mut reader = PacketReader::new(&data);
        let packet = ForReconnectPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.reconnect_info, "");
    }
}

//! EscapePacket implementation.
//!
//! Sent to prompt the server to send a `ReconnectPacket` (back to the Nexus).
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// EscapePacket (ID 105) - Outgoing (empty)
#[derive(Debug, Clone)]
pub struct EscapePacket;

impl RotmgPacket for EscapePacket {
    fn deserialize(_reader: &mut PacketReader) -> io::Result<Self> {
        Ok(Self)
    }

    fn description(&self) -> String {
        "Escape".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut reader = PacketReader::new(&[]);
        let packet = EscapePacket::deserialize(&mut reader).unwrap();
        assert_eq!(packet.description(), "Escape");
        assert!(reader.is_fully_parsed());
    }
}

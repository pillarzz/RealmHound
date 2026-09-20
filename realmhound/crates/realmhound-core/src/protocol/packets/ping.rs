//! Ping packet implementation.
//!
//! Received periodically by the server to prompt a response (Pong) from the
//! client.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Ping packet (ID 8) - Incoming
///
/// Sent occasionally by the server. The `serial` is a nonce the client is
/// expected to echo back in its Pong reply (signed int).
#[derive(Debug, Clone)]
pub struct PingPacket {
    /// A nonce value which is expected to be present in the reply.
    pub serial: i32,
}

impl RotmgPacket for PingPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let serial = reader.read_i32()?;

        Ok(Self { serial })
    }

    fn description(&self) -> String {
        format!("Ping: serial={}", self.serial)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ping_deserialize() {
        let data = 123456i32.to_be_bytes();
        let mut reader = PacketReader::new(&data);
        let packet = PingPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.serial, 123456);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_ping_description() {
        let data = 42i32.to_be_bytes();
        let mut reader = PacketReader::new(&data);
        let packet = PingPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.description(), "Ping: serial=42");
    }
}

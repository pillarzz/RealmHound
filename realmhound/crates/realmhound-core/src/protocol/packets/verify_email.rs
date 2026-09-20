//! VerifyEmail packet implementation.
//!
//! Received to prompt the player to verify their email. Carries no payload.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// VerifyEmail packet (ID 39) - Incoming
#[derive(Debug, Clone)]
pub struct VerifyEmailPacket;

impl RotmgPacket for VerifyEmailPacket {
    fn deserialize(_reader: &mut PacketReader) -> io::Result<Self> {
        Ok(Self)
    }

    fn description(&self) -> String {
        "VerifyEmail".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data: [u8; 0] = [];
        let mut reader = PacketReader::new(&data);
        let packet = VerifyEmailPacket::deserialize(&mut reader).unwrap();
        let _ = packet;
        assert!(reader.is_fully_parsed());
    }
}

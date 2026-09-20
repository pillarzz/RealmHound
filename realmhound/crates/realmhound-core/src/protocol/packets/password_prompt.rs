//! Password prompt packet implementation.
//!
//! Received to prompt the player to enter their password.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Password prompt packet (ID 79) - Incoming
///
/// Sent by the server to prompt the player for their password. The single
/// field is read as an unsigned 32-bit integer; its exact meaning is unknown.
#[derive(Debug, Clone)]
pub struct PasswordPromptPacket {
    /// Status value (purpose unknown).
    pub clean_password_status: u32,
}

impl RotmgPacket for PasswordPromptPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let clean_password_status = reader.read_u32()?;

        Ok(Self {
            clean_password_status,
        })
    }

    fn description(&self) -> String {
        format!("PasswordPrompt: status={}", self.clean_password_status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_password_prompt_deserialize() {
        let data = 0xDEADBEEFu32.to_be_bytes();
        let mut reader = PacketReader::new(&data);
        let packet = PasswordPromptPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.clean_password_status, 0xDEAD_BEEF);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_password_prompt_description() {
        let data = 1u32.to_be_bytes();
        let mut reader = PacketReader::new(&data);
        let packet = PasswordPromptPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.description(), "PasswordPrompt: status=1");
    }
}

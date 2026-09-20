//! PlayerTextPacket implementation.
//!
//! Sent when the client sends a chat message.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// PlayerTextPacket (ID 9) - Outgoing
#[derive(Debug, Clone)]
pub struct PlayerTextPacket {
    /// The message to send.
    pub text: String,
}

impl RotmgPacket for PlayerTextPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let text = reader.read_string()?;

        Ok(Self { text })
    }

    fn description(&self) -> String {
        format!("PlayerText: text={}", self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        // short length-prefixed string "hi"
        let data = [0u8, 2, b'h', b'i'];
        let mut reader = PacketReader::new(&data);
        let packet = PlayerTextPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.text, "hi");
        assert!(reader.is_fully_parsed());
    }
}

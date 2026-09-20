//! Create packet implementation.
//!
//! Sent by the client to create a new character.
//! This is an outgoing packet that we intercept to detect new character creation.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Create packet (ID 57) - Outgoing
///
/// Sent by the client when creating a new character.
/// We intercept this to know that the next CreateSuccess is for a brand-new character
/// (as opposed to loading an existing one via Load packet).
#[derive(Debug, Clone)]
pub struct CreatePacket {
    /// Class type ID for the new character
    pub class_type: u16,
    /// Skin ID (0 = default skin)
    pub skin_type: u16,
    /// Whether the character is in challenger mode
    pub is_challenger: bool,
    /// Whether the character is in seasonal mode
    pub is_seasonal: bool,
}

impl RotmgPacket for CreatePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let class_type = reader.read_u16()?;
        let skin_type = reader.read_u16()?;
        let is_challenger = reader.read_bool()?;
        let is_seasonal = reader.read_bool()?;

        Ok(Self {
            class_type,
            skin_type,
            is_challenger,
            is_seasonal,
        })
    }

    fn description(&self) -> String {
        format!(
            "Create: classType={}, skinType={}, challenger={}, seasonal={}",
            self.class_type, self.skin_type, self.is_challenger, self.is_seasonal
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_packet_deserialize() {
        let mut data = Vec::new();

        // class_type: u16 (782 = Wizard)
        data.extend_from_slice(&782u16.to_be_bytes());
        // skin_type: u16 (0 = default)
        data.extend_from_slice(&0u16.to_be_bytes());
        // is_challenger: bool
        data.push(0);
        // is_seasonal: bool
        data.push(1);

        let mut reader = PacketReader::new(&data);
        let packet = CreatePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.class_type, 782);
        assert_eq!(packet.skin_type, 0);
        assert!(!packet.is_challenger);
        assert!(packet.is_seasonal);
    }

    #[test]
    fn test_create_packet_description() {
        let packet = CreatePacket {
            class_type: 782,
            skin_type: 0,
            is_challenger: false,
            is_seasonal: true,
        };
        let desc = packet.description();
        assert!(desc.contains("classType=782"));
        assert!(desc.contains("seasonal=true"));
    }
}

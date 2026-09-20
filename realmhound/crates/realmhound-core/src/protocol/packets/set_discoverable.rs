//! SetDiscoverablePacket implementation.
//!
//! Sent to toggle the player's discoverable status and icon.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// SetDiscoverablePacket (ID 167) - Outgoing
#[derive(Debug, Clone)]
pub struct SetDiscoverablePacket {
    /// Whether the player is discoverable.
    pub is_discoverable: bool,
    /// The discoverable icon id.
    pub icon: i16,
}

impl RotmgPacket for SetDiscoverablePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let is_discoverable = reader.read_bool()?;
        let icon = reader.read_i16()?;

        Ok(Self {
            is_discoverable,
            icon,
        })
    }

    fn description(&self) -> String {
        format!(
            "SetDiscoverable: discoverable={}, icon={}",
            self.is_discoverable, self.icon
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = [1u8, 0, 4];
        let mut reader = PacketReader::new(&data);
        let packet = SetDiscoverablePacket::deserialize(&mut reader).unwrap();

        assert!(packet.is_discoverable);
        assert_eq!(packet.icon, 4);
        assert!(reader.is_fully_parsed());
    }
}

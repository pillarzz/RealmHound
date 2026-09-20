//! TeleportPacket implementation.
//!
//! Sent to teleport to another player.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// TeleportPacket (ID 1) - Outgoing
#[derive(Debug, Clone)]
pub struct TeleportPacket {
    /// The object id of the player to teleport to.
    pub object_id: i32,
    /// The object name of the player to teleport to.
    pub name: String,
}

impl RotmgPacket for TeleportPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let object_id = reader.read_i32()?;
        let name = reader.read_string()?;

        Ok(Self { object_id, name })
    }

    fn description(&self) -> String {
        format!("Teleport: objectId={}, name={}", self.object_id, self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&1234i32.to_be_bytes());
        data.extend_from_slice(&5u16.to_be_bytes());
        data.extend_from_slice(b"Hello");

        let mut reader = PacketReader::new(&data);
        let packet = TeleportPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.object_id, 1234);
        assert_eq!(packet.name, "Hello");
        assert!(reader.is_fully_parsed());
    }
}

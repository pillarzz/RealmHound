//! ActivePetUpdate packet implementation.
//!
//! Received to notify the player of a new active pet.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ActivePetUpdate packet (ID 76) - Incoming
#[derive(Debug, Clone)]
pub struct ActivePetUpdatePacket {
    /// The instance id of the active pet.
    pub instance_id: i32,
}

impl RotmgPacket for ActivePetUpdatePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let instance_id = reader.read_i32()?;
        Ok(Self { instance_id })
    }

    fn description(&self) -> String {
        format!("ActivePetUpdate: instanceId={}", self.instance_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = 9001i32.to_be_bytes();
        let mut reader = PacketReader::new(&data);
        let packet = ActivePetUpdatePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.instance_id, 9001);
        assert!(reader.is_fully_parsed());
    }
}

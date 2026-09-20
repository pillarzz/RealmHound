//! ActivePetUpdateRequestPacket implementation.
//!
//! Sent to make an update to the pet currently following the player.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ActivePetUpdateRequestPacket (ID 24) - Outgoing
#[derive(Debug, Clone)]
pub struct ActivePetUpdateRequestPacket {
    /// The type of update to perform (raw `ActivePetUpdateType` ordinal).
    pub command_type: u8,
    /// The instance id of the pet to update.
    pub instance_id: i32,
}

impl RotmgPacket for ActivePetUpdateRequestPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let command_type = reader.read_byte()?;
        let instance_id = reader.read_i32()?;
        Ok(Self {
            command_type,
            instance_id,
        })
    }

    fn description(&self) -> String {
        format!(
            "ActivePetUpdateRequest: cmd={}, instanceId={}",
            self.command_type, self.instance_id
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.push(3);
        data.extend_from_slice(&999i32.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = ActivePetUpdateRequestPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.command_type, 3);
        assert_eq!(packet.instance_id, 999);
        assert!(reader.is_fully_parsed());
    }
}

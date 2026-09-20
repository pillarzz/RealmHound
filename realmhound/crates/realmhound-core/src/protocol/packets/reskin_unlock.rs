//! ReskinUnlock packet implementation.
//!
//! Received to notify the player that a new object (skin) has been unlocked.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ReskinUnlock packet (ID 107) - Incoming
#[derive(Debug, Clone)]
pub struct ReskinUnlockPacket {
    /// The type of unlocked object.
    pub unlock_type: u8,
    /// The id of the object that was unlocked.
    pub unlock_id: i32,
}

impl RotmgPacket for ReskinUnlockPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let unlock_type = reader.read_byte()?;
        let unlock_id = reader.read_i32()?;
        Ok(Self {
            unlock_type,
            unlock_id,
        })
    }

    fn description(&self) -> String {
        format!(
            "ReskinUnlock: type={} id={}",
            self.unlock_type, self.unlock_id
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.push(1); // unlockType
        data.extend_from_slice(&5050i32.to_be_bytes()); // unlockId

        let mut reader = PacketReader::new(&data);
        let packet = ReskinUnlockPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.unlock_type, 1);
        assert_eq!(packet.unlock_id, 5050);
        assert!(reader.is_fully_parsed());
    }
}

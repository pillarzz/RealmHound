//! DeletePet packet implementation.
//!
//! Received to notify the player that a pet has been deleted.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// DeletePet packet (ID 4) - Incoming
#[derive(Debug, Clone)]
pub struct DeletePetPacket {
    /// The id of the pet which has been deleted.
    pub pet_id: i32,
}

impl RotmgPacket for DeletePetPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let pet_id = reader.read_i32()?;
        Ok(Self { pet_id })
    }

    fn description(&self) -> String {
        format!("DeletePet: petId={}", self.pet_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = 4242i32.to_be_bytes();
        let mut reader = PacketReader::new(&data);
        let packet = DeletePetPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.pet_id, 4242);
        assert!(reader.is_fully_parsed());
    }
}

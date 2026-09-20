//! FavourPetPacket implementation.
//!
//! Sent to favour (favourite) a pet.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// FavourPetPacket (ID 145) - Outgoing
#[derive(Debug, Clone)]
pub struct FavourPetPacket {
    /// The id of the pet to favour.
    pub pet_id: i32,
}

impl RotmgPacket for FavourPetPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let pet_id = reader.read_i32()?;
        Ok(Self { pet_id })
    }

    fn description(&self) -> String {
        format!("FavourPet: petId={}", self.pet_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&555i32.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = FavourPetPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.pet_id, 555);
        assert!(reader.is_fully_parsed());
    }
}

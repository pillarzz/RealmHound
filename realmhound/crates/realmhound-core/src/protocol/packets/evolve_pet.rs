//! EvolvePet packet implementation.
//!
//! Received to give the player information about a newly evolved pet.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// EvolvePet packet (ID 87) - Incoming
#[derive(Debug, Clone)]
pub struct EvolvePetPacket {
    /// The id of the pet which has evolved.
    pub pet_id: i32,
    /// The current skin id of the pet.
    pub initial_skin: i32,
    /// The skin id of the pet after its evolution.
    pub final_skin: i32,
}

impl RotmgPacket for EvolvePetPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let pet_id = reader.read_i32()?;
        let initial_skin = reader.read_i32()?;
        let final_skin = reader.read_i32()?;
        Ok(Self {
            pet_id,
            initial_skin,
            final_skin,
        })
    }

    fn description(&self) -> String {
        format!(
            "EvolvePet: petId={} initialSkin={} finalSkin={}",
            self.pet_id, self.initial_skin, self.final_skin
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&10i32.to_be_bytes()); // petId
        data.extend_from_slice(&20i32.to_be_bytes()); // initialSkin
        data.extend_from_slice(&30i32.to_be_bytes()); // finalSkin

        let mut reader = PacketReader::new(&data);
        let packet = EvolvePetPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.pet_id, 10);
        assert_eq!(packet.initial_skin, 20);
        assert_eq!(packet.final_skin, 30);
        assert!(reader.is_fully_parsed());
    }
}

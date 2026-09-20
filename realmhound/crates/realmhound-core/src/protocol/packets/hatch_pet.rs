//! HatchPet packet implementation.
//!
//! Received to give the player information about a newly hatched pet.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// HatchPet packet (ID 23) - Incoming
#[derive(Debug, Clone)]
pub struct HatchPetPacket {
    /// The name of the hatched pet.
    pub pet_name: String,
    /// The skin id of the hatched pet.
    pub pet_skin: i32,
    /// The object type of the pet.
    pub pet_type: i32,
}

impl RotmgPacket for HatchPetPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let pet_name = reader.read_string()?;
        let pet_skin = reader.read_i32()?;
        let pet_type = reader.read_i32()?;
        Ok(Self {
            pet_name,
            pet_skin,
            pet_type,
        })
    }

    fn description(&self) -> String {
        format!(
            "HatchPet: name={} skin={} type={}",
            self.pet_name, self.pet_skin, self.pet_type
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        let name = "Fluffy";
        data.extend_from_slice(&(name.len() as u16).to_be_bytes());
        data.extend_from_slice(name.as_bytes());
        data.extend_from_slice(&100i32.to_be_bytes()); // skin
        data.extend_from_slice(&200i32.to_be_bytes()); // type

        let mut reader = PacketReader::new(&data);
        let packet = HatchPetPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.pet_name, "Fluffy");
        assert_eq!(packet.pet_skin, 100);
        assert_eq!(packet.pet_type, 200);
        assert!(reader.is_fully_parsed());
    }
}

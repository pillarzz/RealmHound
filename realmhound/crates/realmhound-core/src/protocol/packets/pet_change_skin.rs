//! ChangePetSkinPacket implementation.
//!
//! Sent to change the skin of a pet.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// PetChangeSkinPacket (ID 33) - Outgoing
#[derive(Debug, Clone)]
pub struct PetChangeSkinPacket {
    /// The id of the pet whose skin is changing.
    pub pet_id: i32,
    /// The id of the new skin for the pet.
    pub skin_type: i32,
    /// The currency used (raw `PaymentType` ordinal, read as an `i32`).
    pub currency: i32,
}

impl RotmgPacket for PetChangeSkinPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let pet_id = reader.read_i32()?;
        let skin_type = reader.read_i32()?;
        let currency = reader.read_i32()?;
        Ok(Self {
            pet_id,
            skin_type,
            currency,
        })
    }

    fn description(&self) -> String {
        format!(
            "PetChangeSkin: petId={}, skinType={}, currency={}",
            self.pet_id, self.skin_type, self.currency
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&111i32.to_be_bytes());
        data.extend_from_slice(&222i32.to_be_bytes());
        data.extend_from_slice(&1i32.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = PetChangeSkinPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.pet_id, 111);
        assert_eq!(packet.skin_type, 222);
        assert_eq!(packet.currency, 1);
        assert!(reader.is_fully_parsed());
    }
}

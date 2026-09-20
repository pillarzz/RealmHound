//! PetYardUpdate packet implementation.
//!
//! Received when the pet yard is updated to a new type of yard.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// The type of pet yard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PetYardType {
    /// Common yard (1).
    Common,
    /// Uncommon yard (2).
    Uncommon,
    /// Rare yard (3).
    Rare,
    /// Legendary yard (4).
    Legendary,
    /// Divine yard (5).
    Divine,
    /// Unrecognized yard type code.
    Unknown(i32),
}

impl PetYardType {
    fn from_code(code: i32) -> Self {
        match code {
            1 => PetYardType::Common,
            2 => PetYardType::Uncommon,
            3 => PetYardType::Rare,
            4 => PetYardType::Legendary,
            5 => PetYardType::Divine,
            other => PetYardType::Unknown(other),
        }
    }
}

/// PetYardUpdate packet (ID 78) - Incoming
#[derive(Debug, Clone)]
pub struct PetYardUpdatePacket {
    /// The type of the new yard.
    pub yard_type: PetYardType,
}

impl RotmgPacket for PetYardUpdatePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let yard_type = PetYardType::from_code(reader.read_i32()?);
        Ok(Self { yard_type })
    }

    fn description(&self) -> String {
        format!("PetYardUpdate: yardType={:?}", self.yard_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_known() {
        let data = 4i32.to_be_bytes();
        let mut reader = PacketReader::new(&data);
        let packet = PetYardUpdatePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.yard_type, PetYardType::Legendary);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_deserialize_unknown() {
        let data = 99i32.to_be_bytes();
        let mut reader = PacketReader::new(&data);
        let packet = PetYardUpdatePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.yard_type, PetYardType::Unknown(99));
        assert!(reader.is_fully_parsed());
    }
}

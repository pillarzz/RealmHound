//! NewAbility packet implementation.
//!
//! Received when a new ability has been unlocked by the player.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// NewAbility packet (ID 41) - Incoming
#[derive(Debug, Clone)]
pub struct NewAbilityPacket {
    /// The type of ability which has been unlocked.
    pub ability_type: i32,
}

impl RotmgPacket for NewAbilityPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let ability_type = reader.read_i32()?;
        Ok(Self { ability_type })
    }

    fn description(&self) -> String {
        format!("NewAbility: abilityType={}", self.ability_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = 5i32.to_be_bytes();
        let mut reader = PacketReader::new(&data);
        let packet = NewAbilityPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.ability_type, 5);
        assert!(reader.is_fully_parsed());
    }
}

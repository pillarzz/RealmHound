//! SetAbilityPacket implementation.
//!
//! Sent to set the active ability slot.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// SetAbilityPacket (ID 157) - Outgoing
#[derive(Debug, Clone)]
pub struct SetAbilityPacket {
    /// The current client time.
    pub time: i32,
    /// The ability index.
    pub index: i8,
}

impl RotmgPacket for SetAbilityPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let time = reader.read_i32()?;
        let index = reader.read_byte()? as i8;

        Ok(Self { time, index })
    }

    fn description(&self) -> String {
        format!("SetAbility: index={}, time={}", self.index, self.time)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = [0u8, 0, 0, 100, 3];
        let mut reader = PacketReader::new(&data);
        let packet = SetAbilityPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.time, 100);
        assert_eq!(packet.index, 3);
        assert!(reader.is_fully_parsed());
    }
}

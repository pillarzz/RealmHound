//! SetConditionPacket implementation.
//!
//! Sent when the player inflicts a condition effect.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// SetConditionPacket (ID 60) - Outgoing
#[derive(Debug, Clone)]
pub struct SetConditionPacket {
    /// The condition effect being inflicted.
    pub condition_effect: i8,
    /// The duration of the condition effect.
    pub condition_duration: f32,
}

impl RotmgPacket for SetConditionPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let condition_effect = reader.read_byte()? as i8;
        let condition_duration = reader.read_f32()?;

        Ok(Self {
            condition_effect,
            condition_duration,
        })
    }

    fn description(&self) -> String {
        format!(
            "SetCondition: effect={}, duration={:.1}",
            self.condition_effect, self.condition_duration
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.push(5);
        data.extend_from_slice(&2.5f32.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = SetConditionPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.condition_effect, 5);
        assert_eq!(packet.condition_duration, 2.5);
        assert!(reader.is_fully_parsed());
    }
}

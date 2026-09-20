//! ShowEffect packet implementation.
//!
//! Received to tell the player to display an effect such as an AoE grenade.
//! Fields are gated behind a
//! bitmask byte; absent fields use the same defaults as the reference client.

use super::super::data::WorldPosData;
use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

const EFFECT_BIT_COLOR: u8 = 1;
const EFFECT_BIT_POS1X: u8 = 2;
const EFFECT_BIT_POS1Y: u8 = 4;
const EFFECT_BIT_POS2X: u8 = 8;
const EFFECT_BIT_POS2Y: u8 = 16;
const EFFECT_BIT_DURATION: u8 = 32;
const EFFECT_BIT_ID: u8 = 64;
const UNKNOWN_BIT_ID: u8 = 128;

/// ShowEffect packet (ID 11) - Incoming
#[derive(Debug, Clone)]
pub struct ShowEffectPacket {
    /// The type of effect to display.
    pub effect_type: u8,
    /// The objectId the effect is targeting.
    pub target_object_id: i32,
    /// Unknown. Probably the start position of the effect.
    pub pos1: WorldPosData,
    /// Unknown. Probably the end position of the effect.
    pub pos2: WorldPosData,
    /// The color of the effect.
    pub color: i32,
    /// The duration of the effect.
    pub duration: f32,
    /// Unknown.
    pub unknown_byte: u8,
}

impl RotmgPacket for ShowEffectPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let mut pos1 = WorldPosData { x: 0.0, y: 0.0 };
        let mut pos2 = WorldPosData { x: 0.0, y: 0.0 };

        let effect_type = reader.read_byte()?;
        let bitmask = reader.read_byte()?;

        let target_object_id = if bitmask & EFFECT_BIT_ID != 0 {
            reader.read_compressed_int()?
        } else {
            0
        };
        if bitmask & EFFECT_BIT_POS1X != 0 {
            pos1.x = reader.read_f32()?;
        }
        if bitmask & EFFECT_BIT_POS1Y != 0 {
            pos1.y = reader.read_f32()?;
        }
        if bitmask & EFFECT_BIT_POS2X != 0 {
            pos2.x = reader.read_f32()?;
        }
        if bitmask & EFFECT_BIT_POS2Y != 0 {
            pos2.y = reader.read_f32()?;
        }
        let color = if bitmask & EFFECT_BIT_COLOR != 0 {
            reader.read_i32()?
        } else {
            0xFFFFFF
        };
        let duration = if bitmask & EFFECT_BIT_DURATION != 0 {
            reader.read_f32()?
        } else {
            1.0
        };
        let unknown_byte = if bitmask & UNKNOWN_BIT_ID != 0 {
            reader.read_byte()?
        } else {
            100
        };

        Ok(Self {
            effect_type,
            target_object_id,
            pos1,
            pos2,
            color,
            duration,
            unknown_byte,
        })
    }

    fn description(&self) -> String {
        format!(
            "ShowEffect: type={} target={} pos1=({:.1},{:.1}) pos2=({:.1},{:.1}) color={:#08X} dur={:.1}",
            self.effect_type,
            self.target_object_id,
            self.pos1.x,
            self.pos1.y,
            self.pos2.x,
            self.pos2.y,
            self.color,
            self.duration
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_all_fields() {
        let bitmask = EFFECT_BIT_COLOR
            | EFFECT_BIT_POS1X
            | EFFECT_BIT_POS1Y
            | EFFECT_BIT_POS2X
            | EFFECT_BIT_POS2Y
            | EFFECT_BIT_DURATION
            | EFFECT_BIT_ID
            | UNKNOWN_BIT_ID;

        let mut data = Vec::new();
        data.push(3); // effectType
        data.push(bitmask);
        // target object id via compressed int: 300
        data.extend_from_slice(&encode_compressed_int(300));
        data.extend_from_slice(&1.0f32.to_be_bytes()); // pos1.x
        data.extend_from_slice(&2.0f32.to_be_bytes()); // pos1.y
        data.extend_from_slice(&3.0f32.to_be_bytes()); // pos2.x
        data.extend_from_slice(&4.0f32.to_be_bytes()); // pos2.y
        data.extend_from_slice(&0x00ABCDEFi32.to_be_bytes()); // color
        data.extend_from_slice(&5.0f32.to_be_bytes()); // duration
        data.push(42); // unknown byte

        let mut reader = PacketReader::new(&data);
        let packet = ShowEffectPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.effect_type, 3);
        assert_eq!(packet.target_object_id, 300);
        assert_eq!(packet.pos1.x, 1.0);
        assert_eq!(packet.pos1.y, 2.0);
        assert_eq!(packet.pos2.x, 3.0);
        assert_eq!(packet.pos2.y, 4.0);
        assert_eq!(packet.color, 0x00ABCDEF);
        assert_eq!(packet.duration, 5.0);
        assert_eq!(packet.unknown_byte, 42);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_deserialize_no_optional_fields() {
        let data = [7u8, 0u8]; // effectType=7, bitmask=0
        let mut reader = PacketReader::new(&data);
        let packet = ShowEffectPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.effect_type, 7);
        assert_eq!(packet.target_object_id, 0);
        assert_eq!(packet.pos1.x, 0.0);
        assert_eq!(packet.pos1.y, 0.0);
        assert_eq!(packet.pos2.x, 0.0);
        assert_eq!(packet.pos2.y, 0.0);
        assert_eq!(packet.color, 0xFFFFFF);
        assert_eq!(packet.duration, 1.0);
        assert_eq!(packet.unknown_byte, 100);
        assert!(reader.is_fully_parsed());
    }

    /// Encode a non-negative int using RotMG's compressed-int scheme so it
    /// round-trips through `read_compressed_int` (6 value bits + sign/continuation
    /// in the first byte, 7 value bits per following byte).
    fn encode_compressed_int(value: i32) -> Vec<u8> {
        assert!(value >= 0, "test helper only encodes non-negative values");
        let mut first = (value & 63) as u8;
        let mut rest = value >> 6;
        if rest != 0 {
            first |= 128;
        }
        let mut out = vec![first];
        while rest != 0 {
            let mut byte = (rest & 127) as u8;
            rest >>= 7;
            if rest != 0 {
                byte |= 128;
            }
            out.push(byte);
        }
        out
    }
}

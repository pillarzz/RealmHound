//! Damage-with-effect packet implementation.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// DamageWithEffect packet (ID 166) - Incoming
///
/// Extends the regular damage payload with a condition-effect identifier and
/// magnitude. For stasis, the magnitude is the remaining duration in seconds.
#[derive(Debug, Clone)]
pub struct DamageWithEffectPacket {
    /// The object id of the entity receiving the damage or effect.
    pub target_id: i32,
    /// Status effects applied with the damage.
    pub effects: Vec<u8>,
    /// The amount of damage taken.
    pub damage_amount: u16,
    /// Raw damage flags byte.
    pub info: u8,
    /// The id of the bullet which caused the damage.
    pub bullet_id: u16,
    /// The object id of the entity which owned the bullet.
    pub object_id: i32,
    /// The condition-effect index associated with the magnitude.
    pub effect_id: u8,
    /// Effect magnitude.
    pub value: f32,
}

impl RotmgPacket for DamageWithEffectPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let target_id = reader.read_i32()?;

        let effects_len = reader.read_byte()? as usize;
        let mut effects = Vec::with_capacity(effects_len);
        for _ in 0..effects_len {
            effects.push(reader.read_byte()?);
        }

        let damage_amount = reader.read_u16()?;
        let info = reader.read_byte()?;
        let bullet_id = reader.read_u16()?;
        let object_id = reader.read_i32()?;
        let effect_id = reader.read_byte()?;
        let value = reader.read_f32()?;

        tracing::debug!(
            "[DAMAGE_WITH_EFFECT] target_id={}, effects={:?}, damage_amount={}, info={}, bullet_id={}, object_id={}, effect_id={}, value={}",
            target_id,
            effects,
            damage_amount,
            info,
            bullet_id,
            object_id,
            effect_id,
            value,
        );

        Ok(Self {
            target_id,
            effects,
            damage_amount,
            info,
            bullet_id,
            object_id,
            effect_id,
            value,
        })
    }

    fn description(&self) -> String {
        format!(
            "DamageWithEffect: target={} effect={} value={:.2}",
            self.target_id, self.effect_id, self.value
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stasis_with_leading_effect() {
        let data = [
            0x00, 0x00, 0x03, 0xaf, 0x01, 0x16, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff,
            0xff, 0x16, 0x40, 0xe0, 0x00, 0x00,
        ];

        let mut reader = PacketReader::new(&data);
        let packet = DamageWithEffectPacket::deserialize(&mut reader).unwrap();

        assert!(reader.is_fully_parsed());
        assert_eq!(packet.target_id, 943);
        assert_eq!(packet.effects, vec![22]);
        assert_eq!(packet.damage_amount, 0);
        assert_eq!(packet.info, 0);
        assert_eq!(packet.bullet_id, 0);
        assert_eq!(packet.object_id, 0x00ff_ffff);
        assert_eq!(packet.effect_id, 22);
        assert_eq!(packet.value, 7.0);
    }

    #[test]
    fn parses_payload_without_leading_effects() {
        let data = [
            0x00, 0x00, 0x03, 0xaf, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff,
            0x16, 0x40, 0xe0, 0x00, 0x00,
        ];

        let mut reader = PacketReader::new(&data);
        let packet = DamageWithEffectPacket::deserialize(&mut reader).unwrap();

        assert!(reader.is_fully_parsed());
        assert_eq!(packet.target_id, 943);
        assert!(packet.effects.is_empty());
        assert_eq!(packet.effect_id, 22);
        assert_eq!(packet.value, 7.0);
    }
}

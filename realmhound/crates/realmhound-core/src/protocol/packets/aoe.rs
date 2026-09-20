//! Aoe packet implementation.
//!
//! Received when an AoE grenade has hit the ground.

use super::super::data::WorldPosData;
use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Aoe packet (ID 64) - Incoming
#[derive(Debug, Clone)]
pub struct AoePacket {
    /// The position which the grenade landed at.
    pub pos: WorldPosData,
    /// The radius of the grenade's area of effect, in game tiles.
    pub radius: f32,
    /// The damage dealt by the grenade.
    pub damage: u16,
    /// The condition effect applied by the grenade.
    pub effect: u8,
    /// The duration of the effect applied.
    pub duration: f32,
    /// Unknown.
    pub orig_type: u16,
    /// The color of the grenade's explosion particles.
    pub color: i32,
    /// Whether or not the damage of this grenade pierces armor.
    pub armor_piercing: bool,
}

impl RotmgPacket for AoePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let pos = WorldPosData::deserialize(reader)?;
        let radius = reader.read_f32()?;
        let damage = reader.read_u16()?;
        let effect = reader.read_byte()?;
        let duration = reader.read_f32()?;
        let orig_type = reader.read_u16()?;
        let color = reader.read_i32()?;
        let armor_piercing = reader.read_bool()?;

        Ok(Self {
            pos,
            radius,
            damage,
            effect,
            duration,
            orig_type,
            color,
            armor_piercing,
        })
    }

    fn description(&self) -> String {
        format!(
            "Aoe: pos=({:.1},{:.1}) radius={:.1} dmg={} effect={} dur={:.1} armorPiercing={}",
            self.pos.x,
            self.pos.y,
            self.radius,
            self.damage,
            self.effect,
            self.duration,
            self.armor_piercing
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&10.0f32.to_be_bytes()); // pos.x
        data.extend_from_slice(&20.0f32.to_be_bytes()); // pos.y
        data.extend_from_slice(&3.5f32.to_be_bytes()); // radius
        data.extend_from_slice(&150u16.to_be_bytes()); // damage
        data.push(5); // effect
        data.extend_from_slice(&2.0f32.to_be_bytes()); // duration
        data.extend_from_slice(&99u16.to_be_bytes()); // origType
        data.extend_from_slice(&0x00FF00FFi32.to_be_bytes()); // color
        data.push(1); // armorPiercing

        let mut reader = PacketReader::new(&data);
        let packet = AoePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.pos.x, 10.0);
        assert_eq!(packet.pos.y, 20.0);
        assert_eq!(packet.radius, 3.5);
        assert_eq!(packet.damage, 150);
        assert_eq!(packet.effect, 5);
        assert_eq!(packet.duration, 2.0);
        assert_eq!(packet.orig_type, 99);
        assert_eq!(packet.color, 0x00FF00FF);
        assert!(packet.armor_piercing);
        assert!(reader.is_fully_parsed());
    }
}

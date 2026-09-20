//! Stats packet implementation.
//!
//! Received to report a character's base stat state (the `StatsStateData` payload).

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// The eight base stats carried by a [`StatsPacket`].
///
/// Each value is a signed byte.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatsStateData {
    pub hp: i8,
    pub mp: i8,
    pub attack: i8,
    pub defense: i8,
    pub speed: i8,
    pub vitality: i8,
    pub wisdom: i8,
    pub dexterity: i8,
}

impl StatsStateData {
    /// Deserialize the eight signed stat bytes.
    pub fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        Ok(Self {
            hp: reader.read_byte()? as i8,
            mp: reader.read_byte()? as i8,
            attack: reader.read_byte()? as i8,
            defense: reader.read_byte()? as i8,
            speed: reader.read_byte()? as i8,
            vitality: reader.read_byte()? as i8,
            wisdom: reader.read_byte()? as i8,
            dexterity: reader.read_byte()? as i8,
        })
    }
}

/// Stats packet (ID 139) - Incoming
#[derive(Debug, Clone)]
pub struct StatsPacket {
    /// The character ID the stats are for.
    pub char_id: i32,
    /// The base stat state.
    pub state: StatsStateData,
}

impl RotmgPacket for StatsPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let char_id = reader.read_compressed_int()?;
        let state = StatsStateData::deserialize(reader)?;

        Ok(Self { char_id, state })
    }

    fn description(&self) -> String {
        format!(
            "Stats: char={} hp={} mp={} atk={} def={} spd={} vit={} wis={} dex={}",
            self.char_id,
            self.state.hp,
            self.state.mp,
            self.state.attack,
            self.state.defense,
            self.state.speed,
            self.state.vitality,
            self.state.wisdom,
            self.state.dexterity
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.push(5); // char_id as compressed int (small positive)
        data.extend_from_slice(&[40, 30, 10, 8, 6, 4, 2, 1]);

        let mut reader = PacketReader::new(&data);
        let packet = StatsPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.char_id, 5);
        assert_eq!(packet.state.hp, 40);
        assert_eq!(packet.state.dexterity, 1);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_signed_values() {
        let mut data = Vec::new();
        data.push(2);
        data.extend_from_slice(&[0xFF, 0xFE, 0, 0, 0, 0, 0, 0]); // -1, -2, ...

        let mut reader = PacketReader::new(&data);
        let packet = StatsPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.state.hp, -1);
        assert_eq!(packet.state.mp, -2);
    }
}

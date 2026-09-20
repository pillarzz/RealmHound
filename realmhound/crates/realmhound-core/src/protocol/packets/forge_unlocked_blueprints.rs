//! ForgeUnlockedBlueprints packet implementation.
//!
//! Received when the player enters the nexus, listing unlocked forge
//! blueprints.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ForgeUnlockedBlueprints packet (ID 120) - Incoming
#[derive(Debug, Clone)]
pub struct ForgeUnlockedBlueprintsPacket {
    /// Seasonal forge flag.
    pub seasonal_forge: u8,
    /// The item ids of unlocked blueprints.
    pub unlocked_blueprints: Vec<i32>,
}

impl RotmgPacket for ForgeUnlockedBlueprintsPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let seasonal_forge = reader.read_byte()?;
        // Length is a compressed int; read_array_length bound-checks it.
        let len = reader.read_array_length()?;
        let mut unlocked_blueprints = Vec::with_capacity(len);
        for _ in 0..len {
            unlocked_blueprints.push(reader.read_compressed_int()?);
        }
        Ok(Self {
            seasonal_forge,
            unlocked_blueprints,
        })
    }

    fn description(&self) -> String {
        format!(
            "ForgeUnlockedBlueprints: seasonal={} count={}",
            self.seasonal_forge,
            self.unlocked_blueprints.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode a non-negative int using RotMG's compressed-int scheme.
    fn encode_compressed_int(value: i32) -> Vec<u8> {
        assert!(value >= 0);
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

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.push(1); // seasonalForge
        data.extend_from_slice(&encode_compressed_int(3)); // length
        data.extend_from_slice(&encode_compressed_int(100));
        data.extend_from_slice(&encode_compressed_int(2000));
        data.extend_from_slice(&encode_compressed_int(30000));

        let mut reader = PacketReader::new(&data);
        let packet = ForgeUnlockedBlueprintsPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.seasonal_forge, 1);
        assert_eq!(packet.unlocked_blueprints, vec![100, 2000, 30000]);
        assert!(reader.is_fully_parsed());
    }
}

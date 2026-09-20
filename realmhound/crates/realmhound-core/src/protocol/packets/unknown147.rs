//! UnknownPacket147 implementation.
//!
//! Nothing is known about this packet; fields are undocumented.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Unknown147Packet (ID 147) - Incoming
#[derive(Debug, Clone)]
pub struct Unknown147Packet {
    /// Unknown byte (sign-extended to int).
    pub unknown_byte: i32,
    /// Unknown int.
    pub unknown_int1: i32,
    /// Unknown int.
    pub unknown_int2: i32,
    /// Unknown int.
    pub unknown_int3: i32,
}

impl RotmgPacket for Unknown147Packet {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let unknown_byte = (reader.read_byte()? as i8) as i32;
        let unknown_int1 = reader.read_i32()?;
        let unknown_int2 = reader.read_i32()?;
        let unknown_int3 = reader.read_i32()?;

        Ok(Self {
            unknown_byte,
            unknown_int1,
            unknown_int2,
            unknown_int3,
        })
    }

    fn description(&self) -> String {
        format!(
            "Unknown147: byte={}, int1={}, int2={}, int3={}",
            self.unknown_byte, self.unknown_int1, self.unknown_int2, self.unknown_int3
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = [1u8, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4];
        let mut reader = PacketReader::new(&data);
        let packet = Unknown147Packet::deserialize(&mut reader).unwrap();

        assert_eq!(packet.unknown_byte, 1);
        assert_eq!(packet.unknown_int1, 2);
        assert_eq!(packet.unknown_int2, 3);
        assert_eq!(packet.unknown_int3, 4);
        assert!(reader.is_fully_parsed());
    }
}

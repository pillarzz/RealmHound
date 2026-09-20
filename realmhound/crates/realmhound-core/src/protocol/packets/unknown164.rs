//! UnknownPacket164 implementation.
//!
//! Packet related to battle pass missions; fields are undocumented.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Unknown164Packet (ID 164) - Incoming
#[derive(Debug, Clone)]
pub struct Unknown164Packet {
    /// Unknown byte.
    pub unknown_byte1: i8,
    /// Unknown byte.
    pub unknown_byte2: i8,
    /// Unknown short.
    pub unknown_short: i16,
}

impl RotmgPacket for Unknown164Packet {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let unknown_byte1 = reader.read_byte()? as i8;
        let unknown_byte2 = reader.read_byte()? as i8;
        let unknown_short = reader.read_i16()?;

        Ok(Self {
            unknown_byte1,
            unknown_byte2,
            unknown_short,
        })
    }

    fn description(&self) -> String {
        format!(
            "Unknown164: byte1={}, byte2={}, short={}",
            self.unknown_byte1, self.unknown_byte2, self.unknown_short
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = [1u8, 2, 0, 9];
        let mut reader = PacketReader::new(&data);
        let packet = Unknown164Packet::deserialize(&mut reader).unwrap();

        assert_eq!(packet.unknown_byte1, 1);
        assert_eq!(packet.unknown_byte2, 2);
        assert_eq!(packet.unknown_short, 9);
        assert!(reader.is_fully_parsed());
    }
}

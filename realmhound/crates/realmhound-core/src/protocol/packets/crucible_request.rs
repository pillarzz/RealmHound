//! CrucibleRequest packet implementation.
//!
//! Sent to request crucible enchantment rolls.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// CrucibleRequest packet (ID 182) - Outgoing
#[derive(Debug, Clone)]
pub struct CrucibleRequestPacket {
    /// The requested enchantment types.
    pub types: Vec<i32>,
}

impl RotmgPacket for CrucibleRequestPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let count = reader.read_i16()?;
        if count < 0 || (count as usize) * 4 > reader.remaining() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid crucible type count: {}", count),
            ));
        }
        let mut types = Vec::with_capacity(count as usize);
        for _ in 0..count {
            types.push(reader.read_i32()?);
        }
        Ok(Self { types })
    }

    fn description(&self) -> String {
        format!("CrucibleRequest: types={}", self.types.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&2i16.to_be_bytes());
        data.extend_from_slice(&10i32.to_be_bytes());
        data.extend_from_slice(&20i32.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = CrucibleRequestPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.types, vec![10, 20]);
        assert!(reader.is_fully_parsed());
    }
}

//! DamageBoost packet implementation.
//!
//! Nothing is known about this packet; it carries 8 opaque bytes.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// DamageBoost packet (ID 148) - Incoming
#[derive(Debug, Clone)]
pub struct DamageBoostPacket {
    /// Unknown 8-byte blob.
    pub unknown_bytes: Vec<u8>,
}

impl RotmgPacket for DamageBoostPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let unknown_bytes = reader.read_bytes(8)?;

        Ok(Self { unknown_bytes })
    }

    fn description(&self) -> String {
        format!("DamageBoost: bytes={:02X?}", self.unknown_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let mut reader = PacketReader::new(&data);
        let packet = DamageBoostPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.unknown_bytes, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(reader.is_fully_parsed());
    }
}

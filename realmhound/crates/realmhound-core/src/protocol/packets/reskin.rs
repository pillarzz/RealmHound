//! ReskinPacket implementation.
//!
//! Sent to activate a new skin for the current character.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ReskinPacket (ID 51) - Outgoing
#[derive(Debug, Clone)]
pub struct ReskinPacket {
    /// The id of the skin to activate.
    pub skin_id: i32,
}

impl RotmgPacket for ReskinPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let skin_id = reader.read_i32()?;
        Ok(Self { skin_id })
    }

    fn description(&self) -> String {
        format!("Reskin: skinId={}", self.skin_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&1234i32.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = ReskinPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.skin_id, 1234);
        assert!(reader.is_fully_parsed());
    }
}

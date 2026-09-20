//! RetitlePacket implementation.
//!
//! Sent to change the player's title (prefix/suffix).
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// RetitlePacket (ID 155) - Outgoing
#[derive(Debug, Clone)]
pub struct RetitlePacket {
    /// The title prefix id.
    pub prefix: i32,
    /// The title suffix id.
    pub suffix: i32,
}

impl RotmgPacket for RetitlePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let prefix = reader.read_i32()?;
        let suffix = reader.read_i32()?;

        Ok(Self { prefix, suffix })
    }

    fn description(&self) -> String {
        format!("Retitle: prefix={}, suffix={}", self.prefix, self.suffix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = [0u8, 0, 0, 1, 0, 0, 0, 2];
        let mut reader = PacketReader::new(&data);
        let packet = RetitlePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.prefix, 1);
        assert_eq!(packet.suffix, 2);
        assert!(reader.is_fully_parsed());
    }
}

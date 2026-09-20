//! ChangeAllyShootPacket implementation.
//!
//! Sent to tell the server whether to receive ally (other player) projectiles.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ChangeAllyShootPacket (ID 122) - Outgoing
#[derive(Debug, Clone)]
pub struct ChangeAllyShootPacket {
    /// Whether the server will send ally projectiles (0 = disable, 1 = enable).
    pub toggle: i32,
}

impl RotmgPacket for ChangeAllyShootPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let toggle = reader.read_i32()?;

        Ok(Self { toggle })
    }

    fn description(&self) -> String {
        format!("ChangeAllyShoot: toggle={}", self.toggle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = 1i32.to_be_bytes();
        let mut reader = PacketReader::new(&data);
        let packet = ChangeAllyShootPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.toggle, 1);
        assert!(reader.is_fully_parsed());
    }
}

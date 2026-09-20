//! PongPacket implementation.
//!
//! Sent to acknowledge the `PingPacket`.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// PongPacket (ID 31) - Outgoing
#[derive(Debug, Clone)]
pub struct PongPacket {
    /// The serial value received in the `PingPacket` which this acknowledges.
    pub serial: i32,
    /// The current client time.
    pub time: i32,
}

impl RotmgPacket for PongPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let serial = reader.read_i32()?;
        let time = reader.read_i32()?;

        Ok(Self { serial, time })
    }

    fn description(&self) -> String {
        format!("Pong: serial={}, time={}", self.serial, self.time)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = [0u8, 0, 0, 7, 0, 0, 0, 100];
        let mut reader = PacketReader::new(&data);
        let packet = PongPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.serial, 7);
        assert_eq!(packet.time, 100);
        assert!(reader.is_fully_parsed());
    }
}

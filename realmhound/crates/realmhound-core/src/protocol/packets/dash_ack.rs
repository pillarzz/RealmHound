//! DashAckPacket implementation.
//!
//! Sent to acknowledge a `DashPacket`.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// DashAckPacket (ID 138) - Outgoing
#[derive(Debug, Clone)]
pub struct DashAckPacket {
    /// The current client time.
    pub time: i32,
}

impl RotmgPacket for DashAckPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let time = reader.read_i32()?;

        Ok(Self { time })
    }

    fn description(&self) -> String {
        format!("DashAck: time={}", self.time)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = 1234i32.to_be_bytes();
        let mut reader = PacketReader::new(&data);
        let packet = DashAckPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.time, 1234);
        assert!(reader.is_fully_parsed());
    }
}

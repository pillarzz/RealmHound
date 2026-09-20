//! GotoAckPacket implementation.
//!
//! Sent to acknowledge a `GotoPacket`.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// GotoAckPacket (ID 65) - Outgoing
#[derive(Debug, Clone)]
pub struct GotoAckPacket {
    /// The current client time.
    pub time: i32,
    /// Reset flag.
    pub reset: bool,
}

impl RotmgPacket for GotoAckPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let time = reader.read_i32()?;
        let reset = reader.read_bool()?;

        Ok(Self { time, reset })
    }

    fn description(&self) -> String {
        format!("GotoAck: time={}, reset={}", self.time, self.reset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&999i32.to_be_bytes());
        data.push(1);

        let mut reader = PacketReader::new(&data);
        let packet = GotoAckPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.time, 999);
        assert!(packet.reset);
        assert!(reader.is_fully_parsed());
    }
}

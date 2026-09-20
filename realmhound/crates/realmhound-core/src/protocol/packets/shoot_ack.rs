//! ShootAckPacket implementation.
//!
//! Likely used by the server to track the total count of ShootAckPackets sent.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ShootAckPacket (ID 121) - Outgoing
#[derive(Debug, Clone)]
pub struct ShootAckPacket {
    /// The current client time.
    pub time: i32,
    /// Enemy shots acknowledged.
    pub ack: i16,
}

impl RotmgPacket for ShootAckPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let time = reader.read_i32()?;
        let ack = reader.read_i16()?;

        Ok(Self { time, ack })
    }

    fn description(&self) -> String {
        format!("ShootAck: time={}, ack={}", self.time, self.ack)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&500i32.to_be_bytes());
        data.extend_from_slice(&3i16.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = ShootAckPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.time, 500);
        assert_eq!(packet.ack, 3);
        assert!(reader.is_fully_parsed());
    }
}

//! Queue information packet implementation.
//!
//! Received when the client connects to a server that has a login queue.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Queue information packet (ID 112) - Incoming
///
/// Sent while the client is waiting in a server login queue. Both fields are
/// unsigned 16-bit integers.
#[derive(Debug, Clone)]
pub struct QueueInfoPacket {
    /// The current position of the client in the queue.
    pub current_position: u16,
    /// The maximum number of clients allowed in the queue.
    pub max_position: u16,
}

impl RotmgPacket for QueueInfoPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let current_position = reader.read_u16()?;
        let max_position = reader.read_u16()?;

        Ok(Self {
            current_position,
            max_position,
        })
    }

    fn description(&self) -> String {
        format!("QueueInfo: {}/{}", self.current_position, self.max_position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_bytes(current: u16, max: u16) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&current.to_be_bytes());
        data.extend_from_slice(&max.to_be_bytes());
        data
    }

    #[test]
    fn test_queue_info_deserialize() {
        let data = build_bytes(42, 1000);
        let mut reader = PacketReader::new(&data);
        let packet = QueueInfoPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.current_position, 42);
        assert_eq!(packet.max_position, 1000);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_queue_info_description() {
        let data = build_bytes(5, 50);
        let mut reader = PacketReader::new(&data);
        let packet = QueueInfoPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.description(), "QueueInfo: 5/50");
    }
}

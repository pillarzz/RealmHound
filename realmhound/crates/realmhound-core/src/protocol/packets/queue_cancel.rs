//! QueueCancelPacket implementation.
//!
//! Sent when the client's position in the queue should be cancelled.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// QueueCancelPacket (ID 113) - Outgoing
#[derive(Debug, Clone)]
pub struct QueueCancelPacket {
    /// The guild associated with the cancelled queue.
    pub guild: String,
}

impl RotmgPacket for QueueCancelPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let guild = reader.read_string()?;

        Ok(Self { guild })
    }

    fn description(&self) -> String {
        format!("QueueCancel: guild={}", self.guild)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = [0u8, 0];
        let mut reader = PacketReader::new(&data);
        let packet = QueueCancelPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.guild, "");
        assert!(reader.is_fully_parsed());
    }
}

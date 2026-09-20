//! AcceleratorActivated packet implementation.
//!
//! Received (id 153) right after the client consumes an account accelerator
//! (e.g. a dust-drop potion). The payload is the accelerator's object type;
//! it carries no duration, so the remaining time is estimated from the known
//! accelerator's fixed duration at the moment of activation.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// AcceleratorActivated packet (ID 153) - Incoming
#[derive(Debug, Clone)]
pub struct AcceleratorActivatedPacket {
    /// Object type of the activated accelerator.
    pub object_type: i32,
}

impl RotmgPacket for AcceleratorActivatedPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let object_type = reader.read_i32()?;
        Ok(Self { object_type })
    }

    fn description(&self) -> String {
        format!("AcceleratorActivated: object_type={}", self.object_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        // 0x000002ef = 751 (Acc Dust Chance Day)
        let data = [0x00, 0x00, 0x02, 0xef];
        let mut reader = PacketReader::new(&data);
        let packet = AcceleratorActivatedPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.object_type, 0x2ef);
        assert!(reader.is_fully_parsed());
    }
}

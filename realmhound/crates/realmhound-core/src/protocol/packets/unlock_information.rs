//! UnlockInformation packet implementation.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// UnlockInformation packet (ID 109) - Incoming
#[derive(Debug, Clone)]
pub struct UnlockInformationPacket {
    /// Unknown unlock type.
    pub unlock_type: i32,
}

impl RotmgPacket for UnlockInformationPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let unlock_type = reader.read_i32()?;
        Ok(Self { unlock_type })
    }

    fn description(&self) -> String {
        format!("UnlockInformation: unlockType={}", self.unlock_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = 42i32.to_be_bytes();
        let mut reader = PacketReader::new(&data);
        let packet = UnlockInformationPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.unlock_type, 42);
        assert!(reader.is_fully_parsed());
    }
}

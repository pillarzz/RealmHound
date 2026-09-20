//! ClaimBattlePass packet implementation.
//!
//! Sent when redeeming battle pass items.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ClaimBattlePass packet (ID 149) - Outgoing
#[derive(Debug, Clone)]
pub struct ClaimBattlePassPacket {
    /// Unknown string (item key).
    pub unknown_string: String,
    /// Unknown byte.
    pub unknown: i8,
}

impl RotmgPacket for ClaimBattlePassPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let unknown_string = reader.read_string()?;
        let unknown = reader.read_byte()? as i8;
        Ok(Self {
            unknown_string,
            unknown,
        })
    }

    fn description(&self) -> String {
        format!(
            "ClaimBattlePass: key={}, value={}",
            self.unknown_string, self.unknown
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&3u16.to_be_bytes());
        data.extend_from_slice(b"bp1");
        data.push(5);

        let mut reader = PacketReader::new(&data);
        let packet = ClaimBattlePassPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.unknown_string, "bp1");
        assert_eq!(packet.unknown, 5);
        assert!(reader.is_fully_parsed());
    }
}

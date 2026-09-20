//! CheckCreditsPacket implementation.
//!
//! Empty request packet.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// CheckCreditsPacket (ID 102) - Outgoing (empty)
#[derive(Debug, Clone)]
pub struct CheckCreditsPacket;

impl RotmgPacket for CheckCreditsPacket {
    fn deserialize(_reader: &mut PacketReader) -> io::Result<Self> {
        Ok(Self)
    }

    fn description(&self) -> String {
        "CheckCredits".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut reader = PacketReader::new(&[]);
        let packet = CheckCreditsPacket::deserialize(&mut reader).unwrap();
        assert_eq!(packet.description(), "CheckCredits");
        assert!(reader.is_fully_parsed());
    }
}

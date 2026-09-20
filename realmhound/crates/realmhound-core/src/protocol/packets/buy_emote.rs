//! BuyEmotePacket implementation.
//!
//! Sent to purchase an emote.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// BuyEmotePacket (ID 160) - Outgoing
#[derive(Debug, Clone)]
pub struct BuyEmotePacket {
    /// The emote type being purchased.
    pub emote_type: i32,
}

impl RotmgPacket for BuyEmotePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let emote_type = reader.read_i32()?;

        Ok(Self { emote_type })
    }

    fn description(&self) -> String {
        format!("BuyEmote: emoteType={}", self.emote_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = [0u8, 0, 0, 12];
        let mut reader = PacketReader::new(&data);
        let packet = BuyEmotePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.emote_type, 12);
        assert!(reader.is_fully_parsed());
    }
}

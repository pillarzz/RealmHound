//! TradeChanged packet implementation.
//!
//! Received when the active trade is changed.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Maximum number of offer flags accepted. A trade offer covers the 4 hotbar +
/// 8 inventory slots; this leaves headroom while rejecting corrupt length
/// prefixes before allocation.
const MAX_OFFER_FLAGS: i16 = 256;

/// TradeChanged packet (ID 28) - Incoming
#[derive(Debug, Clone)]
pub struct TradeChangedPacket {
    /// Which items in the trade partner's inventory are selected.
    pub offer: Vec<bool>,
}

impl RotmgPacket for TradeChangedPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let len = reader.read_i16()?;
        if len < 0 || len > MAX_OFFER_FLAGS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid offer length: {}", len),
            ));
        }
        let len = len as usize;
        let mut offer = Vec::with_capacity(len);
        for _ in 0..len {
            offer.push(reader.read_bool()?);
        }
        Ok(Self { offer })
    }

    fn description(&self) -> String {
        format!(
            "TradeChanged: offer={} selected of {}",
            self.offer.iter().filter(|&&b| b).count(),
            self.offer.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&4i16.to_be_bytes());
        data.extend_from_slice(&[1, 1, 0, 1]);

        let mut reader = PacketReader::new(&data);
        let packet = TradeChangedPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.offer, vec![true, true, false, true]);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_negative_length_rejected() {
        let mut data = Vec::new();
        data.extend_from_slice(&(-1i16).to_be_bytes());
        let mut reader = PacketReader::new(&data);
        assert!(TradeChangedPacket::deserialize(&mut reader).is_err());
    }
}

//! ChangeTradePacket implementation.
//!
//! Sent to change the client's offer in the current active trade.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ChangeTradePacket (ID 56) - Outgoing
///
/// Items 0-3 are the hotbar items, 4-12 are the 8 inventory slots. A `true`
/// value means the item is selected for the trade.
#[derive(Debug, Clone)]
pub struct ChangeTradePacket {
    /// Which items in the client's inventory are selected.
    pub offer: Vec<bool>,
}

impl RotmgPacket for ChangeTradePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let count = reader.read_i16()?;
        if count < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Negative offer length: {}", count),
            ));
        }
        let count = count as usize;
        if count > reader.remaining() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Offer length {} exceeds remaining bytes", count),
            ));
        }
        let mut offer = Vec::with_capacity(count);
        for _ in 0..count {
            offer.push(reader.read_bool()?);
        }
        Ok(Self { offer })
    }

    fn description(&self) -> String {
        format!(
            "ChangeTrade: offer={}",
            self.offer.iter().filter(|&&b| b).count()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&3i16.to_be_bytes());
        data.push(1);
        data.push(0);
        data.push(1);

        let mut reader = PacketReader::new(&data);
        let packet = ChangeTradePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.offer, vec![true, false, true]);
        assert!(reader.is_fully_parsed());
    }
}

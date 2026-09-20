//! TradeAccepted packet implementation.
//!
//! Received when the active trade is accepted.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Maximum number of offer flags accepted in either list. A trade offer covers
/// the 4 hotbar + 8 inventory slots; this leaves headroom while rejecting
/// corrupt length prefixes before allocation.
const MAX_OFFER_FLAGS: i16 = 256;

fn read_offer(reader: &mut PacketReader) -> io::Result<Vec<bool>> {
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
    Ok(offer)
}

/// TradeAccepted packet (ID 14) - Incoming
#[derive(Debug, Clone)]
pub struct TradeAcceptedPacket {
    /// Which items in the client's inventory are selected.
    pub client_offer: Vec<bool>,
    /// Which items in the trade partner's inventory are selected.
    pub partner_offer: Vec<bool>,
}

impl RotmgPacket for TradeAcceptedPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let client_offer = read_offer(reader)?;
        let partner_offer = read_offer(reader)?;
        Ok(Self {
            client_offer,
            partner_offer,
        })
    }

    fn description(&self) -> String {
        format!(
            "TradeAccepted: clientOffer={} partnerOffer={}",
            self.client_offer.iter().filter(|&&b| b).count(),
            self.partner_offer.iter().filter(|&&b| b).count()
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
        data.extend_from_slice(&[1, 0, 1]);
        data.extend_from_slice(&2i16.to_be_bytes());
        data.extend_from_slice(&[0, 1]);

        let mut reader = PacketReader::new(&data);
        let packet = TradeAcceptedPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.client_offer, vec![true, false, true]);
        assert_eq!(packet.partner_offer, vec![false, true]);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_negative_length_rejected() {
        let mut data = Vec::new();
        data.extend_from_slice(&(-5i16).to_be_bytes());
        let mut reader = PacketReader::new(&data);
        assert!(TradeAcceptedPacket::deserialize(&mut reader).is_err());
    }
}

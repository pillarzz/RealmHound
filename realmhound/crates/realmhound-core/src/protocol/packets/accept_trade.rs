//! AcceptTradePacket implementation.
//!
//! Sent to accept the current active trade.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Read an `i16`-length-prefixed array of booleans, bound-checking the length.
fn read_bool_array(reader: &mut PacketReader) -> io::Result<Vec<bool>> {
    let count = reader.read_i16()?;
    if count < 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Negative bool array length: {}", count),
        ));
    }
    let count = count as usize;
    // Each boolean is 1 byte; reject if larger than the remaining payload.
    if count > reader.remaining() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Bool array length {} exceeds remaining bytes", count),
        ));
    }
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(reader.read_bool()?);
    }
    Ok(out)
}

/// AcceptTradePacket (ID 36) - Outgoing
///
/// Items 0-3 are the hotbar items, 4-12 are the 8 inventory slots. A `true`
/// value means the item is selected for the trade.
#[derive(Debug, Clone)]
pub struct AcceptTradePacket {
    /// Which items in the client's inventory are selected.
    pub client_offer: Vec<bool>,
    /// Which items in the trade partner's inventory are selected.
    pub partner_offer: Vec<bool>,
}

impl RotmgPacket for AcceptTradePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let client_offer = read_bool_array(reader)?;
        let partner_offer = read_bool_array(reader)?;
        Ok(Self {
            client_offer,
            partner_offer,
        })
    }

    fn description(&self) -> String {
        format!(
            "AcceptTrade: clientOffer={} partnerOffer={}",
            self.client_offer.iter().filter(|&&b| b).count(),
            self.partner_offer.iter().filter(|&&b| b).count(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&2i16.to_be_bytes());
        data.push(1);
        data.push(0);
        data.extend_from_slice(&3i16.to_be_bytes());
        data.push(0);
        data.push(1);
        data.push(1);

        let mut reader = PacketReader::new(&data);
        let packet = AcceptTradePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.client_offer, vec![true, false]);
        assert_eq!(packet.partner_offer, vec![false, true, true]);
        assert!(reader.is_fully_parsed());
    }
}

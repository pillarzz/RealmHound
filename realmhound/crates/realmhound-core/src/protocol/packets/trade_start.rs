//! TradeStart packet implementation.
//!
//! Received when a new active trade has been initiated. Carries a `TradeItemData` payload.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Maximum number of trade item entries accepted in either inventory list.
/// A full RotMG inventory is 20 slots; this leaves generous headroom while
/// still rejecting corrupt length prefixes before allocation.
const MAX_TRADE_ITEMS: i16 = 256;

/// A single item entry in a trade inventory snapshot.
#[derive(Debug, Clone)]
pub struct TradeItemData {
    /// The item id.
    pub item: i32,
    /// The slot type the item is stored in.
    pub slot_type: i32,
    /// Whether or not the item is tradeable.
    pub tradeable: bool,
    /// Whether or not the item is included in an active trade.
    pub included: bool,
    /// Enchantment data as string.
    pub unique_data: String,
}

impl TradeItemData {
    /// Deserialize from packet data.
    pub fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let item = reader.read_i32()?;
        let slot_type = reader.read_i32()?;
        let tradeable = reader.read_bool()?;
        let included = reader.read_bool()?;
        let unique_data = reader.read_string()?;
        Ok(Self {
            item,
            slot_type,
            tradeable,
            included,
            unique_data,
        })
    }
}

fn read_trade_items(reader: &mut PacketReader) -> io::Result<Vec<TradeItemData>> {
    let len = reader.read_i16()?;
    if len < 0 || len > MAX_TRADE_ITEMS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Invalid trade item count: {}", len),
        ));
    }
    let len = len as usize;
    let mut items = Vec::with_capacity(len);
    for _ in 0..len {
        items.push(TradeItemData::deserialize(reader)?);
    }
    Ok(items)
}

/// TradeStart packet (ID 86) - Incoming
#[derive(Debug, Clone)]
pub struct TradeStartPacket {
    /// The player's inventory snapshot (slots 0-3 hotbar, 4-19 inventory/backpack).
    pub client_items: Vec<TradeItemData>,
    /// The trade partner's name.
    pub partner_name: String,
    /// The trade partner's inventory snapshot.
    pub partner_items: Vec<TradeItemData>,
    /// Object id.
    pub object_id: i32,
    /// Unknown.
    pub unknown: u8,
}

impl RotmgPacket for TradeStartPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let client_items = read_trade_items(reader)?;
        let partner_name = reader.read_string()?;
        let partner_items = read_trade_items(reader)?;
        let object_id = reader.read_i32()?;
        let unknown = reader.read_byte()?;

        Ok(Self {
            client_items,
            partner_name,
            partner_items,
            object_id,
            unknown,
        })
    }

    fn description(&self) -> String {
        format!(
            "TradeStart: partner={} clientItems={} partnerItems={} objectId={}",
            self.partner_name,
            self.client_items.len(),
            self.partner_items.len(),
            self.object_id
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_string(data: &mut Vec<u8>, s: &str) {
        data.extend_from_slice(&(s.len() as u16).to_be_bytes());
        data.extend_from_slice(s.as_bytes());
    }

    fn push_item(
        data: &mut Vec<u8>,
        item: i32,
        slot: i32,
        tradeable: bool,
        included: bool,
        uniq: &str,
    ) {
        data.extend_from_slice(&item.to_be_bytes());
        data.extend_from_slice(&slot.to_be_bytes());
        data.push(tradeable as u8);
        data.push(included as u8);
        push_string(data, uniq);
    }

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&1i16.to_be_bytes()); // clientItems len
        push_item(&mut data, 100, 0, true, false, "ench");
        push_string(&mut data, "Partner");
        data.extend_from_slice(&2i16.to_be_bytes()); // partnerItems len
        push_item(&mut data, 200, 1, false, true, "");
        push_item(&mut data, 300, 2, true, true, "x");
        data.extend_from_slice(&777i32.to_be_bytes()); // objectId
        data.push(9); // unknown

        let mut reader = PacketReader::new(&data);
        let packet = TradeStartPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.client_items.len(), 1);
        assert_eq!(packet.client_items[0].item, 100);
        assert!(packet.client_items[0].tradeable);
        assert!(!packet.client_items[0].included);
        assert_eq!(packet.client_items[0].unique_data, "ench");
        assert_eq!(packet.partner_name, "Partner");
        assert_eq!(packet.partner_items.len(), 2);
        assert_eq!(packet.partner_items[1].item, 300);
        assert_eq!(packet.object_id, 777);
        assert_eq!(packet.unknown, 9);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_negative_length_rejected() {
        let mut data = Vec::new();
        data.extend_from_slice(&(-1i16).to_be_bytes());
        let mut reader = PacketReader::new(&data);
        assert!(TradeStartPacket::deserialize(&mut reader).is_err());
    }
}

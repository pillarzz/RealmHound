//! BuyCustomisationSocketPacket implementation.
//!
//! Sent to buy customisation socket items.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// A single item-buy entry.
#[derive(Debug, Clone)]
pub struct ItemBuyData {
    /// The item category.
    pub category: i8,
    /// The object id of the item.
    pub object_id: i32,
    /// The currency used.
    pub currency: i8,
}

impl ItemBuyData {
    /// Deserialize from packet data.
    pub fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let category = reader.read_byte()? as i8;
        let object_id = reader.read_i32()?;
        let currency = reader.read_byte()? as i8;
        Ok(Self {
            category,
            object_id,
            currency,
        })
    }
}

/// BuyCustomisationSocketPacket (ID 140) - Outgoing
#[derive(Debug, Clone)]
pub struct BuyCustomisationSocketPacket {
    /// The items being purchased.
    pub items: Vec<ItemBuyData>,
}

impl RotmgPacket for BuyCustomisationSocketPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let count = reader.read_i16()?;
        if count < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Negative item count: {}", count),
            ));
        }
        let count = count as usize;
        // Each ItemBuyData is 6 bytes (i8 + i32 + i8); reject if larger than payload.
        if count.saturating_mul(6) > reader.remaining() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Item count {} exceeds remaining bytes", count),
            ));
        }
        let mut items = Vec::with_capacity(count);
        for _ in 0..count {
            items.push(ItemBuyData::deserialize(reader)?);
        }
        Ok(Self { items })
    }

    fn description(&self) -> String {
        format!("BuyCustomisationSocket: items={}", self.items.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&2i16.to_be_bytes()); // item count
        data.push(1); // item0.category
        data.extend_from_slice(&100i32.to_be_bytes()); // item0.objectId
        data.push(0); // item0.currency
        data.push(2); // item1.category
        data.extend_from_slice(&200i32.to_be_bytes()); // item1.objectId
        data.push(1); // item1.currency

        let mut reader = PacketReader::new(&data);
        let packet = BuyCustomisationSocketPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.items.len(), 2);
        assert_eq!(packet.items[0].category, 1);
        assert_eq!(packet.items[0].object_id, 100);
        assert_eq!(packet.items[1].object_id, 200);
        assert_eq!(packet.items[1].currency, 1);
        assert!(reader.is_fully_parsed());
    }
}

//! UpgradeEnchanter packet implementation.
//!
//! Sent to upgrade the enchanter.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// UpgradeEnchanter packet (ID 185) - Outgoing
#[derive(Debug, Clone)]
pub struct UpgradeEnchanterPacket {
    /// The dust type used.
    pub dust_type: i8,
    /// The currency type used.
    pub currency_type: i8,
}

impl RotmgPacket for UpgradeEnchanterPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let dust_type = reader.read_byte()? as i8;
        let currency_type = reader.read_byte()? as i8;
        Ok(Self {
            dust_type,
            currency_type,
        })
    }

    fn description(&self) -> String {
        format!(
            "UpgradeEnchanter: dust={}, currency={}",
            self.dust_type, self.currency_type
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = [1u8, 2u8];
        let mut reader = PacketReader::new(&data);
        let packet = UpgradeEnchanterPacket::deserialize(&mut reader).unwrap();
        assert_eq!(packet.dust_type, 1);
        assert_eq!(packet.currency_type, 2);
        assert!(reader.is_fully_parsed());
    }
}

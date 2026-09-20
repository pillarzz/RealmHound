//! PlayerCalloutPacket implementation.
//!
//! Sent when a player makes a callout/ping on an object.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// PlayerCalloutPacket (ID 134) - Outgoing
#[derive(Debug, Clone)]
pub struct PlayerCalloutPacket {
    /// The callout type.
    pub callout_type: i8,
    /// The object id targeted by the callout.
    pub object_id: i32,
}

impl RotmgPacket for PlayerCalloutPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let callout_type = reader.read_byte()? as i8;
        let object_id = reader.read_i32()?;

        Ok(Self {
            callout_type,
            object_id,
        })
    }

    fn description(&self) -> String {
        format!(
            "PlayerCallout: type={}, objectId={}",
            self.callout_type, self.object_id
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = [2u8, 0, 0, 0, 9];
        let mut reader = PacketReader::new(&data);
        let packet = PlayerCalloutPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.callout_type, 2);
        assert_eq!(packet.object_id, 9);
        assert!(reader.is_fully_parsed());
    }
}

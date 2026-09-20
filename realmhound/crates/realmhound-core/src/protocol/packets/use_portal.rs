//! UsePortalPacket implementation.
//!
//! Sent to prompt the server to send a `ReconnectPacket` for the used portal.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// UsePortalPacket (ID 47) - Outgoing
#[derive(Debug, Clone)]
pub struct UsePortalPacket {
    /// The object id of the portal to enter.
    pub object_id: i32,
}

impl RotmgPacket for UsePortalPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let object_id = reader.read_i32()?;

        Ok(Self { object_id })
    }

    fn description(&self) -> String {
        format!("UsePortal: objectId={}", self.object_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = 4321i32.to_be_bytes();
        let mut reader = PacketReader::new(&data);
        let packet = UsePortalPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.object_id, 4321);
        assert!(reader.is_fully_parsed());
    }
}

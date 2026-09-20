//! ActivateCrucible packet implementation.
//!
//! Sent to activate or deactivate a crucible.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ActivateCrucible packet (ID 180) - Outgoing
#[derive(Debug, Clone)]
pub struct ActivateCruciblePacket {
    /// The crucible id.
    pub crucible_id: String,
    /// Whether to activate (true) or deactivate (false).
    pub activate: bool,
}

impl RotmgPacket for ActivateCruciblePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let crucible_id = reader.read_string()?;
        let activate = reader.read_bool()?;
        Ok(Self {
            crucible_id,
            activate,
        })
    }

    fn description(&self) -> String {
        format!(
            "ActivateCrucible: id={}, activate={}",
            self.crucible_id, self.activate
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&2u16.to_be_bytes());
        data.extend_from_slice(b"c1");
        data.push(1);

        let mut reader = PacketReader::new(&data);
        let packet = ActivateCruciblePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.crucible_id, "c1");
        assert!(packet.activate);
        assert!(reader.is_fully_parsed());
    }
}

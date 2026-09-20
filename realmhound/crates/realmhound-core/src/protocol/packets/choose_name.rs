//! ChooseNamePacket implementation.
//!
//! Sent to change the client's account name.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ChooseNamePacket (ID 97) - Outgoing
#[derive(Debug, Clone)]
pub struct ChooseNamePacket {
    /// The name to change the account's name to.
    pub name: String,
}

impl RotmgPacket for ChooseNamePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let name = reader.read_string()?;

        Ok(Self { name })
    }

    fn description(&self) -> String {
        format!("ChooseName: name={}", self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = [0u8, 3, b'a', b'b', b'c'];
        let mut reader = PacketReader::new(&data);
        let packet = ChooseNamePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.name, "abc");
        assert!(reader.is_fully_parsed());
    }
}

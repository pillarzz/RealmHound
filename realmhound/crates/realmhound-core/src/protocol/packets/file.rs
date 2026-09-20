//! File packet implementation.
//!
//! A packet which contains a file.
//! The file body uses a 32-bit length prefix (`readStringUTF32`).

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// File packet (ID 106) - Incoming
#[derive(Debug, Clone)]
pub struct FilePacket {
    /// The name of the received file.
    pub file_name: String,
    /// The bytes of the file (encoded as a string in the game source).
    pub file: String,
}

impl RotmgPacket for FilePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let file_name = reader.read_string()?;
        let file = reader.read_string32()?;
        Ok(Self { file_name, file })
    }

    fn description(&self) -> String {
        format!("File: name={} ({} bytes)", self.file_name, self.file.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        let name = "map.wmap";
        data.extend_from_slice(&(name.len() as u16).to_be_bytes());
        data.extend_from_slice(name.as_bytes());
        let body = "content";
        data.extend_from_slice(&(body.len() as u32).to_be_bytes());
        data.extend_from_slice(body.as_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = FilePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.file_name, "map.wmap");
        assert_eq!(packet.file, "content");
        assert!(reader.is_fully_parsed());
    }
}

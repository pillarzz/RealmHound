//! NewCharacterInfo packet implementation.
//!
//! Sent by the server containing all character data as XML.
//! This packet is received when entering Pet Yard or Daily Quest Room.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// NewCharacterInfo packet (ID 108) - Incoming
///
/// Contains character data as XML, similar to the char/list API response.
/// The server sends this packet when entering certain areas (Pet Yard, Daily Quest Room).
/// This allows reading character stats without making HTTP requests.
#[derive(Debug, Clone)]
pub struct NewCharacterInfoPacket {
    /// Size field (purpose unclear, possibly compressed size)
    pub size: i16,
    /// Character data as XML string (same format as char/list API response)
    pub character_xml: String,
}

impl RotmgPacket for NewCharacterInfoPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let size = reader.read_i16()?;

        // Read remaining bytes as the XML string
        let remaining = reader.read_remaining();
        let character_xml = String::from_utf8(remaining)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        Ok(Self {
            size,
            character_xml,
        })
    }

    fn description(&self) -> String {
        let _xml_preview = if self.character_xml.len() > 100 {
            format!("{}...", &self.character_xml[..100])
        } else {
            self.character_xml.clone()
        };
        format!(
            "NewCharacterInfo: size={}, xml_len={}",
            self.size,
            self.character_xml.len()
        )
    }
}

impl NewCharacterInfoPacket {
    /// Check if this packet contains valid character XML data.
    pub fn has_character_data(&self) -> bool {
        !self.character_xml.is_empty() && self.character_xml.contains("<Char")
    }

    /// Get the XML wrapped in a root element for parsing.
    /// The XML in this packet may not have a root element, so we wrap it.
    pub fn get_parseable_xml(&self) -> String {
        if self.character_xml.starts_with("<?xml") || self.character_xml.starts_with("<Chars") {
            // Already has proper structure
            self.character_xml.clone()
        } else {
            // Wrap in a root element for parsing
            format!("<Chars>{}</Chars>", self.character_xml)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_character_info_deserialize() {
        // Size (2 bytes) + XML data
        let mut data = vec![0x00, 0x10]; // size = 16
        data.extend_from_slice(b"<Char id=\"123\"><Level>20</Level></Char>");

        let mut reader = PacketReader::new(&data);
        let packet = NewCharacterInfoPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.size, 16);
        assert!(packet.character_xml.contains("<Char"));
        assert!(packet.has_character_data());
    }

    #[test]
    fn test_new_character_info_description() {
        let packet = NewCharacterInfoPacket {
            size: 100,
            character_xml: "<Char id=\"1\"></Char>".to_string(),
        };

        let desc = packet.description();
        assert!(desc.contains("NewCharacterInfo"));
        assert!(desc.contains("size=100"));
    }

    #[test]
    fn test_get_parseable_xml() {
        let packet = NewCharacterInfoPacket {
            size: 0,
            character_xml: "<Char id=\"1\"><Level>20</Level></Char>".to_string(),
        };

        let xml = packet.get_parseable_xml();
        assert!(xml.starts_with("<Chars>"));
        assert!(xml.ends_with("</Chars>"));
    }
}

//! TEXT packet (ID 44) - Chat messages.
//!
//! Received when a chat message is sent by another player or NPC.

use super::super::PacketReader;
use super::traits::RotmgPacket;
use std::io;

/// Text/Chat packet received from the server.
///
/// Contains all information about a chat message including
/// sender details, recipient, and the message content.
#[derive(Debug, Clone)]
pub struct TextPacket {
    /// The sender of the message
    pub name: String,
    /// The object ID of the sender
    pub object_id: i32,
    /// The number of stars of the sender
    pub num_stars: i16,
    /// The length of time to display the chat bubble (in seconds?)
    pub bubble_time: u8,
    /// The recipient of the message (empty for public chat)
    pub recipient: String,
    /// The content of the message
    pub text: String,
    /// Clean version of the text (without special characters?)
    pub clean_text: String,
    /// Whether the sender is a supporter
    pub is_supporter: bool,
    /// The star background style of the player
    pub star_background: i32,
}

impl RotmgPacket for TextPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        Ok(Self {
            name: reader.read_string()?,
            object_id: reader.read_i32()?,
            num_stars: reader.read_i16()?,
            bubble_time: reader.read_byte()?,
            recipient: reader.read_string()?,
            text: reader.read_string()?,
            clean_text: reader.read_string()?,
            is_supporter: reader.read_bool()?,
            star_background: reader.read_i32()?,
        })
    }

    fn description(&self) -> String {
        if self.recipient.is_empty() {
            format!("{}: {}", self.name, self.text)
        } else {
            format!("{} -> {}: {}", self.name, self.recipient, self.text)
        }
    }
}

impl TextPacket {
    /// Check if this is a private message.
    pub fn is_private(&self) -> bool {
        !self.recipient.is_empty()
    }

    /// Check if this is from an NPC (object_id < 0 or special name patterns).
    pub fn is_npc(&self) -> bool {
        // NPCs typically have negative object IDs or specific name patterns
        self.object_id < 0 || self.name.starts_with('#')
    }

    /// Check if sender is a supporter (has special star).
    pub fn is_supporter(&self) -> bool {
        self.is_supporter
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_string_bytes(s: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        let len = s.len() as u16;
        bytes.extend_from_slice(&len.to_be_bytes());
        bytes.extend_from_slice(s.as_bytes());
        bytes
    }

    #[test]
    fn test_text_packet_deserialize() {
        let mut data = Vec::new();

        // name: "TestPlayer"
        data.extend_from_slice(&make_string_bytes("TestPlayer"));
        // object_id: 12345
        data.extend_from_slice(&12345i32.to_be_bytes());
        // num_stars: 70
        data.extend_from_slice(&70i16.to_be_bytes());
        // bubble_time: 5
        data.push(5);
        // recipient: "" (empty for public)
        data.extend_from_slice(&make_string_bytes(""));
        // text: "Hello World!"
        data.extend_from_slice(&make_string_bytes("Hello World!"));
        // clean_text: "Hello World!"
        data.extend_from_slice(&make_string_bytes("Hello World!"));
        // is_supporter: true
        data.push(1);
        // star_background: 2
        data.extend_from_slice(&2i32.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = TextPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.name, "TestPlayer");
        assert_eq!(packet.object_id, 12345);
        assert_eq!(packet.num_stars, 70);
        assert_eq!(packet.bubble_time, 5);
        assert_eq!(packet.recipient, "");
        assert_eq!(packet.text, "Hello World!");
        assert!(packet.is_supporter);
        assert!(!packet.is_private());
    }

    #[test]
    fn test_text_packet_private_message() {
        let mut data = Vec::new();

        data.extend_from_slice(&make_string_bytes("Sender"));
        data.extend_from_slice(&100i32.to_be_bytes());
        data.extend_from_slice(&50i16.to_be_bytes());
        data.push(3);
        data.extend_from_slice(&make_string_bytes("Receiver")); // Has recipient
        data.extend_from_slice(&make_string_bytes("Secret message"));
        data.extend_from_slice(&make_string_bytes("Secret message"));
        data.push(0);
        data.extend_from_slice(&0i32.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = TextPacket::deserialize(&mut reader).unwrap();

        assert!(packet.is_private());
        assert_eq!(packet.description(), "Sender -> Receiver: Secret message");
    }
}

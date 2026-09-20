//! GlobalNotification packet implementation.
//!
//! Received when a global notification is broadcast to all players.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// GlobalNotification packet (ID 66) - Incoming
#[derive(Debug, Clone)]
pub struct GlobalNotificationPacket {
    /// The type of notification received.
    pub notification_type: i32,
    /// The notification message.
    pub text: String,
}

impl RotmgPacket for GlobalNotificationPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let notification_type = reader.read_i32()?;
        let text = reader.read_string()?;

        Ok(Self {
            notification_type,
            text,
        })
    }

    fn description(&self) -> String {
        format!(
            "GlobalNotification[{}]: {}",
            self.notification_type, self.text
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_bytes(notification_type: i32, text: &str) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&notification_type.to_be_bytes());
        data.extend_from_slice(&(text.len() as u16).to_be_bytes());
        data.extend_from_slice(text.as_bytes());
        data
    }

    #[test]
    fn test_deserialize() {
        let data = build_bytes(3, "Server restarting");
        let mut reader = PacketReader::new(&data);
        let packet = GlobalNotificationPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.notification_type, 3);
        assert_eq!(packet.text, "Server restarting");
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_description() {
        let data = build_bytes(1, "Event started");
        let mut reader = PacketReader::new(&data);
        let packet = GlobalNotificationPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.description(), "GlobalNotification[1]: Event started");
    }
}

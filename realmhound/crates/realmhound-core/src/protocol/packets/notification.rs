//! Notification packet implementation.
//!
//! Received when the player gets an in-world notification (stat increase,
//! server message, object text, queue position, progress bar, emote, etc.).
//! The payload layout depends on the leading effect type.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Returns the notification effect-type name for a raw ordinal.
pub fn notification_effect_name(effect: u8) -> &'static str {
    match effect {
        0 => "StatIncrease",
        1 => "ServerMessage",
        2 => "ErrorMessage",
        3 => "StickyMessage",
        4 => "Global",
        5 => "Queue",
        6 => "ObjectText",
        7 => "PlayerDeath",
        8 => "PortalOpened",
        9 => "TeleportationError",
        10 => "PlayerCallout",
        11 => "ProgressBar",
        12 => "Behavior",
        13 => "Emote",
        14 => "Victory",
        15 => "MissionRefresh",
        16 => "MissionsProgressOnlyRefresh",
        20 => "BlueprintUnlock",
        21 => "WithIcon",
        22 => "FameBonus",
        23 => "ForgeFire",
        _ => "Unknown",
    }
}

/// Notification packet (ID 67) - Incoming
///
/// The set of populated fields depends on [`effect`](Self::effect); unused
/// fields keep their default (`0` / empty).
#[derive(Debug, Clone, Default)]
pub struct NotificationPacket {
    /// Notification effect type (raw ordinal).
    pub effect: u8,
    /// Effect-specific extra byte (also a bitmask for ProgressBar).
    pub extra: u8,
    /// The object id the notification is for (ObjectText / Emote).
    pub object_id: i32,
    /// The notification message text (most effects).
    pub message: String,
    /// UI extra value (Global).
    pub ui_extra: i32,
    /// Realm queue message type (Queue).
    pub realm_queue_message_type: i32,
    /// Position in the queue (Queue).
    pub queue_pos: i32,
    /// Text color (ObjectText / Behavior).
    pub color: i32,
    /// Picture type (PlayerDeath / PortalOpened / Behavior).
    pub picture_type: i32,
    /// Object id of the calling player (PlayerCallout).
    pub sender_object_id: i32,
    /// Number of stars of the calling player (PlayerCallout).
    pub number_of_stars: i32,
    /// Progress bar maximum (ProgressBar).
    pub progress_max: i32,
    /// Progress bar current value (ProgressBar).
    pub progress_value: i16,
    /// Emote type (Emote).
    pub emote_type: i32,
    /// Raw trailing payload captured for mission
    /// refresh effects (15 `MissionRefresh` / 16 `MissionsProgressOnlyRefresh`)
    /// whose wire layout is still being reverse-engineered. Empty for all other
    /// effects. Remove once the mission schema is decoded.
    pub raw: Vec<u8>,
}

impl NotificationPacket {
    /// Human-readable effect name.
    pub fn effect_name(&self) -> &'static str {
        notification_effect_name(self.effect)
    }
}

impl RotmgPacket for NotificationPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let effect = reader.read_byte()?;
        let extra = reader.read_byte()?;

        let mut p = Self {
            effect,
            extra,
            ..Default::default()
        };

        match effect {
            // StatIncrease / ServerMessage / ErrorMessage / StickyMessage / TeleportationError
            0 | 1 | 2 | 3 | 9 => {
                p.message = reader.read_string()?;
            }
            // Global
            4 => {
                p.message = reader.read_string()?;
                p.ui_extra = reader.read_i16()? as i32;
            }
            // Queue
            5 => {
                p.realm_queue_message_type = reader.read_i32()?;
                p.queue_pos = reader.read_i16()? as i32;
            }
            // ObjectText
            6 => {
                p.message = reader.read_string()?;
                p.object_id = reader.read_i32()?;
                p.color = reader.read_i32()?;
            }
            // PlayerDeath / PortalOpened
            7 | 8 => {
                p.message = reader.read_string()?;
                p.picture_type = reader.read_i32()?;
            }
            // PlayerCallout
            10 => {
                p.message = reader.read_string()?;
                p.sender_object_id = reader.read_i32()?;
                p.number_of_stars = reader.read_i16()? as i32;
            }
            // ProgressBar
            11 => {
                if extra != 0 {
                    if extra & 3 != 0 {
                        p.message = reader.read_string()?;
                    }
                    p.progress_max = reader.read_i32()?;
                    p.progress_value = reader.read_i16()?;
                }
            }
            // Behavior
            12 => {
                p.message = reader.read_string()?;
                p.picture_type = reader.read_i32()?;
                p.color = reader.read_i32()?;
            }
            // Emote
            13 => {
                p.object_id = reader.read_i32()?;
                p.emote_type = reader.read_i32()?;
            }
            // MissionRefresh / MissionsProgressOnlyRefresh:
            // the seasonal mission board is believed to arrive here as a JSON
            // payload, but the exact framing is unconfirmed. Capture the entire
            // remaining payload losslessly so a live sample reveals the layout
            // (see the [MISSION_DEBUG] router log). No decoding yet.
            15 | 16 => {
                p.raw = reader.read_remaining();
            }
            // Remaining effects carry no extra payload.
            _ => {}
        }

        Ok(p)
    }

    fn description(&self) -> String {
        if self.message.is_empty() {
            format!("Notification[{}]", self.effect_name())
        } else {
            format!("Notification[{}]: {}", self.effect_name(), self.message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_string(data: &mut Vec<u8>, s: &str) {
        data.extend_from_slice(&(s.len() as u16).to_be_bytes());
        data.extend_from_slice(s.as_bytes());
    }

    #[test]
    fn test_server_message() {
        let mut data = vec![1, 0]; // effect=ServerMessage, extra=0
        push_string(&mut data, "Hello world");

        let mut reader = PacketReader::new(&data);
        let packet = NotificationPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.effect, 1);
        assert_eq!(packet.effect_name(), "ServerMessage");
        assert_eq!(packet.message, "Hello world");
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_object_text() {
        let mut data = vec![6, 0]; // effect=ObjectText
        push_string(&mut data, "Boss spawned");
        data.extend_from_slice(&12345i32.to_be_bytes()); // object_id
        data.extend_from_slice(&0x00FF00i32.to_be_bytes()); // color

        let mut reader = PacketReader::new(&data);
        let packet = NotificationPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.effect_name(), "ObjectText");
        assert_eq!(packet.message, "Boss spawned");
        assert_eq!(packet.object_id, 12345);
        assert_eq!(packet.color, 0x00FF00);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_mission_refresh_captures_raw_payload() {
        // effect=15 (MissionRefresh), extra=0, then an opaque trailing payload
        // (framing unknown, so it is captured losslessly).
        let mut data = vec![15, 0];
        let payload = br#"{"missions":[]}"#;
        data.extend_from_slice(payload);

        let mut reader = PacketReader::new(&data);
        let packet = NotificationPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.effect, 15);
        assert_eq!(packet.effect_name(), "MissionRefresh");
        assert_eq!(packet.raw, payload);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_progress_bar_empty() {
        let data = vec![11, 0]; // effect=ProgressBar, extra=0 -> no payload
        let mut reader = PacketReader::new(&data);
        let packet = NotificationPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.effect_name(), "ProgressBar");
        assert!(packet.message.is_empty());
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_progress_bar_with_message() {
        let mut data = vec![11, 3]; // effect=ProgressBar, extra=3 -> message + max + value
        push_string(&mut data, "Quest");
        data.extend_from_slice(&100i32.to_be_bytes());
        data.extend_from_slice(&42i16.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = NotificationPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.message, "Quest");
        assert_eq!(packet.progress_max, 100);
        assert_eq!(packet.progress_value, 42);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_queue() {
        let mut data = vec![5, 0]; // effect=Queue
        data.extend_from_slice(&7i32.to_be_bytes()); // realm_queue_message_type
        data.extend_from_slice(&3i16.to_be_bytes()); // queue_pos

        let mut reader = PacketReader::new(&data);
        let packet = NotificationPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.effect_name(), "Queue");
        assert_eq!(packet.realm_queue_message_type, 7);
        assert_eq!(packet.queue_pos, 3);
        assert!(reader.is_fully_parsed());
    }
}

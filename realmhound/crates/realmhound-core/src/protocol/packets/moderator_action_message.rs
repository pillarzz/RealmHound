//! ModeratorActionMessagePacket implementation.
//!
//! Reserved for staff accounts to punish other players.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ModeratorActionMessagePacket (ID 124) - Outgoing
#[derive(Debug, Clone)]
pub struct ModeratorActionMessagePacket {
    /// The type of action requested (0=Mute, 1=Unmute, 2=Kick, 3=Block).
    pub action_code: i32,
    /// The message accompanying the action.
    pub action_message: String,
}

impl RotmgPacket for ModeratorActionMessagePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let action_code = reader.read_i32()?;
        let action_message = reader.read_string()?;

        Ok(Self {
            action_code,
            action_message,
        })
    }

    fn description(&self) -> String {
        format!(
            "ModeratorAction: code={}, message={}",
            self.action_code, self.action_message
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let data = [0u8, 0, 0, 2, 0, 2, b'h', b'i'];
        let mut reader = PacketReader::new(&data);
        let packet = ModeratorActionMessagePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.action_code, 2);
        assert_eq!(packet.action_message, "hi");
        assert!(reader.is_fully_parsed());
    }
}

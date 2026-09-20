//! GuildResult packet implementation.
//!
//! Received in response to a guild operation.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// GuildResult packet (ID 26) - Incoming
#[derive(Debug, Clone)]
pub struct GuildResultPacket {
    /// Whether the guild operation succeeded.
    pub success: bool,
    /// JSON line-builder payload describing the result.
    pub line_builder_json: String,
}

impl RotmgPacket for GuildResultPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let success = reader.read_bool()?;
        let line_builder_json = reader.read_string()?;
        Ok(Self {
            success,
            line_builder_json,
        })
    }

    fn description(&self) -> String {
        format!(
            "GuildResult: success={} json={}",
            self.success, self.line_builder_json
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.push(1); // success
        let json = "{}";
        data.extend_from_slice(&(json.len() as u16).to_be_bytes());
        data.extend_from_slice(json.as_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = GuildResultPacket::deserialize(&mut reader).unwrap();

        assert!(packet.success);
        assert_eq!(packet.line_builder_json, "{}");
        assert!(reader.is_fully_parsed());
    }
}

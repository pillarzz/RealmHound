//! NameResult packet implementation.
//!
//! Received in response to a `ChooseNamePacket`.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// NameResult packet (ID 21) - Incoming
#[derive(Debug, Clone)]
pub struct NameResultPacket {
    /// Whether or not the name change was successful.
    pub success: bool,
    /// The error which occurred, if the result was not successful.
    pub error_text: String,
}

impl RotmgPacket for NameResultPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let success = reader.read_bool()?;
        let error_text = reader.read_string()?;
        Ok(Self {
            success,
            error_text,
        })
    }

    fn description(&self) -> String {
        format!(
            "NameResult: success={} error={}",
            self.success, self.error_text
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_failure() {
        let mut data = Vec::new();
        data.push(0); // success = false
        let err = "taken";
        data.extend_from_slice(&(err.len() as u16).to_be_bytes());
        data.extend_from_slice(err.as_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = NameResultPacket::deserialize(&mut reader).unwrap();

        assert!(!packet.success);
        assert_eq!(packet.error_text, "taken");
        assert!(reader.is_fully_parsed());
    }
}

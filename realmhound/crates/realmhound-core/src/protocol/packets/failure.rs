//! Failure packet implementation.
//!
//! Received when the server reports an error (e.g. disconnect reason,
//! login failure, rate limiting).

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Known failure codes carried in a [`FailurePacket`].
///
/// The numeric values are the raw
/// `error_id` values sent by the server. Unknown codes are not represented
/// here; use [`FailurePacket::error_id`] for the raw value.
pub fn failure_code_name(error_id: i32) -> Option<&'static str> {
    match error_id {
        4 => Some("IncorrectVersion"),
        5 => Some("BadKey"),
        6 => Some("InvalidTeleportTarget"),
        7 => Some("EmailVerificationNeeded"),
        9 => Some("TeleportRealmBlock"),
        10 => Some("WrongServerEntered"),
        11 => Some("ServerFull"),
        15 => Some("ServerQueue"),
        _ => None,
    }
}

/// Failure packet (ID 0) - Incoming
///
/// Sent by the server when an error has occurred. `error_id` is the raw
/// failure code (kept as a signed integer),
/// and `error_description` is a human-readable message.
#[derive(Debug, Clone)]
pub struct FailurePacket {
    /// The error ID code of the failure.
    pub error_id: i32,
    /// A description of the error.
    pub error_description: String,
}

impl FailurePacket {
    /// Returns the human-readable name of the failure code, if known.
    pub fn code_name(&self) -> Option<&'static str> {
        failure_code_name(self.error_id)
    }
}

impl RotmgPacket for FailurePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let error_id = reader.read_i32()?;
        let error_description = reader.read_string()?;

        Ok(Self {
            error_id,
            error_description,
        })
    }

    fn description(&self) -> String {
        match self.code_name() {
            Some(name) => format!(
                "Failure[{}] {}: {}",
                self.error_id, name, self.error_description
            ),
            None => format!("Failure[{}]: {}", self.error_id, self.error_description),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_bytes(error_id: i32, desc: &str) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&error_id.to_be_bytes());
        data.extend_from_slice(&(desc.len() as u16).to_be_bytes());
        data.extend_from_slice(desc.as_bytes());
        data
    }

    #[test]
    fn test_failure_deserialize() {
        let data = build_bytes(7, "Account in use");
        let mut reader = PacketReader::new(&data);
        let packet = FailurePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.error_id, 7);
        assert_eq!(packet.error_description, "Account in use");
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_failure_empty_description() {
        let data = build_bytes(0, "");
        let mut reader = PacketReader::new(&data);
        let packet = FailurePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.error_id, 0);
        assert_eq!(packet.error_description, "");
    }

    #[test]
    fn test_failure_description() {
        let data = build_bytes(3, "Wrong version");
        let mut reader = PacketReader::new(&data);
        let packet = FailurePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.description(), "Failure[3]: Wrong version");
    }

    #[test]
    fn test_failure_known_code_name_and_description() {
        let data = build_bytes(11, "Server is full");
        let mut reader = PacketReader::new(&data);
        let packet = FailurePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.code_name(), Some("ServerFull"));
        assert_eq!(
            packet.description(),
            "Failure[11] ServerFull: Server is full"
        );
    }

    #[test]
    fn test_failure_unknown_code_name() {
        let data = build_bytes(999, "mystery");
        let mut reader = PacketReader::new(&data);
        let packet = FailurePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.code_name(), None);
        assert_eq!(packet.description(), "Failure[999]: mystery");
    }

    #[test]
    fn test_failure_code_name_mappings() {
        assert_eq!(failure_code_name(4), Some("IncorrectVersion"));
        assert_eq!(failure_code_name(5), Some("BadKey"));
        assert_eq!(failure_code_name(6), Some("InvalidTeleportTarget"));
        assert_eq!(failure_code_name(7), Some("EmailVerificationNeeded"));
        assert_eq!(failure_code_name(9), Some("TeleportRealmBlock"));
        assert_eq!(failure_code_name(10), Some("WrongServerEntered"));
        assert_eq!(failure_code_name(11), Some("ServerFull"));
        assert_eq!(failure_code_name(15), Some("ServerQueue"));
        assert_eq!(failure_code_name(8), None);
    }
}

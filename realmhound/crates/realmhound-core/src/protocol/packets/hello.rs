//! HELLO packet (ID 74) - Client connection request.
//!
//! Sent by the client to initiate a connection to the server.
//! Contains authentication information including the access token
//! which can be used to call RotMG's web API.

use super::super::PacketReader;
use super::traits::RotmgPacket;
use std::io;

/// Hello packet sent from client to server.
///
/// This packet initiates a connection and contains authentication
/// information. The `access_token` field is particularly important
/// as it can be used to call RotMG's web API (char/list, etc.).
#[derive(Debug, Clone)]
pub struct HelloPacket {
    /// The id of the map to connect to
    pub game_id: i32,
    /// The current build version of RotMG
    pub build_version: String,
    /// The access token from AppEngine used to login.
    /// This token can be used to call RotMG's web API.
    pub access_token: String,
    /// The key time of the key being used
    pub key_time: i32,
    /// The key of the map to connect to
    pub key: Vec<u8>,
    /// The platform the user is using (e.g., "Steam", "Unity")
    pub user_platform: String,
    /// The platform the game is played on
    pub play_platform: String,
    /// Steam token used to verify Steam user verification
    pub platform_token: String,
    /// The client token (hwid) of the Unity client
    pub client_token: String,
    /// Hardcoded token string
    pub user_token: String,
}

impl RotmgPacket for HelloPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        Ok(Self {
            game_id: reader.read_i32()?,
            build_version: reader.read_string()?,
            access_token: reader.read_string()?,
            key_time: reader.read_i32()?,
            key: reader.read_byte_array()?,
            user_platform: reader.read_string()?,
            play_platform: reader.read_string()?,
            platform_token: reader.read_string()?,
            client_token: reader.read_string()?,
            user_token: reader.read_string()?,
        })
    }

    fn description(&self) -> String {
        format!(
            "HELLO v{} gameId={} platform={}",
            self.build_version, self.game_id, self.user_platform
        )
    }
}

impl HelloPacket {
    /// Get the access token for API calls.
    ///
    /// This token can be used to authenticate with RotMG's web API
    /// endpoints like `char/list` and `account/listPowerUpStats`.
    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    /// Check if this is connecting to the Nexus (gameId = -2).
    pub fn is_nexus(&self) -> bool {
        self.game_id == -2
    }

    /// Check if this is connecting to a vault (gameId = -5).
    pub fn is_vault(&self) -> bool {
        self.game_id == -5
    }

    /// Check if this is connecting to a guild hall (gameId = -6).
    pub fn is_guild_hall(&self) -> bool {
        self.game_id == -6
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

    fn make_byte_array(data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let len = data.len() as u16;
        bytes.extend_from_slice(&len.to_be_bytes());
        bytes.extend_from_slice(data);
        bytes
    }

    #[test]
    fn test_hello_packet_deserialize() {
        let mut data = Vec::new();

        // game_id: -2 (Nexus)
        data.extend_from_slice(&(-2i32).to_be_bytes());
        // build_version: "1.3.2.1.0"
        data.extend_from_slice(&make_string_bytes("1.3.2.1.0"));
        // access_token: "test_token_12345"
        data.extend_from_slice(&make_string_bytes("test_token_12345"));
        // key_time: 0
        data.extend_from_slice(&0i32.to_be_bytes());
        // key: empty array
        data.extend_from_slice(&make_byte_array(&[]));
        // user_platform: "Steam"
        data.extend_from_slice(&make_string_bytes("Steam"));
        // play_platform: "Steam"
        data.extend_from_slice(&make_string_bytes("Steam"));
        // platform_token: ""
        data.extend_from_slice(&make_string_bytes(""));
        // client_token: "hwid_abc123"
        data.extend_from_slice(&make_string_bytes("hwid_abc123"));
        // user_token: ""
        data.extend_from_slice(&make_string_bytes(""));

        let mut reader = PacketReader::new(&data);
        let packet = HelloPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.game_id, -2);
        assert_eq!(packet.build_version, "1.3.2.1.0");
        assert_eq!(packet.access_token, "test_token_12345");
        assert_eq!(packet.key_time, 0);
        assert!(packet.key.is_empty());
        assert_eq!(packet.user_platform, "Steam");
        assert_eq!(packet.play_platform, "Steam");
        assert_eq!(packet.platform_token, "");
        assert_eq!(packet.client_token, "hwid_abc123");
        assert_eq!(packet.user_token, "");
    }

    #[test]
    fn test_hello_packet_is_nexus() {
        let mut data = Vec::new();
        // game_id: -2 (Nexus)
        data.extend_from_slice(&(-2i32).to_be_bytes());
        // build_version: empty string
        data.extend_from_slice(&0u16.to_be_bytes());
        // access_token: empty string
        data.extend_from_slice(&0u16.to_be_bytes());
        // key_time: 0
        data.extend_from_slice(&0i32.to_be_bytes());
        // key: empty byte array
        data.extend_from_slice(&0u16.to_be_bytes());
        // user_platform: empty
        data.extend_from_slice(&0u16.to_be_bytes());
        // play_platform: empty
        data.extend_from_slice(&0u16.to_be_bytes());
        // platform_token: empty
        data.extend_from_slice(&0u16.to_be_bytes());
        // client_token: empty
        data.extend_from_slice(&0u16.to_be_bytes());
        // user_token: empty
        data.extend_from_slice(&0u16.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = HelloPacket::deserialize(&mut reader).unwrap();
        assert!(packet.is_nexus());
        assert!(!packet.is_vault());
    }

    #[test]
    fn test_hello_packet_with_key() {
        let mut data = Vec::new();

        // game_id: 123 (dungeon)
        data.extend_from_slice(&123i32.to_be_bytes());
        // build_version
        data.extend_from_slice(&make_string_bytes("1.3.2.1.0"));
        // access_token
        data.extend_from_slice(&make_string_bytes("my_access_token"));
        // key_time: 12345678
        data.extend_from_slice(&12345678i32.to_be_bytes());
        // key: [0xDE, 0xAD, 0xBE, 0xEF]
        data.extend_from_slice(&make_byte_array(&[0xDE, 0xAD, 0xBE, 0xEF]));
        // remaining strings
        for _ in 0..5 {
            data.extend_from_slice(&0u16.to_be_bytes());
        }

        let mut reader = PacketReader::new(&data);
        let packet = HelloPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.game_id, 123);
        assert_eq!(packet.key_time, 12345678);
        assert_eq!(packet.key, vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn test_hello_packet_description() {
        let packet = HelloPacket {
            game_id: -2,
            build_version: "1.3.2.1.0".to_string(),
            access_token: "token".to_string(),
            key_time: 0,
            key: vec![],
            user_platform: "Steam".to_string(),
            play_platform: "Steam".to_string(),
            platform_token: String::new(),
            client_token: String::new(),
            user_token: String::new(),
        };

        assert_eq!(
            packet.description(),
            "HELLO v1.3.2.1.0 gameId=-2 platform=Steam"
        );
    }
}

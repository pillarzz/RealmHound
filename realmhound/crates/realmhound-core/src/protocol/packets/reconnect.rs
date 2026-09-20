//! Reconnect packet implementation.
//!
//! Received when the server instructs the client to connect to a new host,
//! e.g. when teleporting to a party member on a different server or using a portal.
//! Contains the target server's name, host address, and connection parameters.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Reconnect packet (ID 45) - Incoming
///
/// Sent by the server to redirect the client to a new host. The `name` field
/// contains the human-readable server name (e.g. "USWest"), which is more
/// reliable than IP-based server detection.
#[derive(Debug, Clone)]
pub struct ReconnectPacket {
    /// Human-readable server name (e.g. "USWest", "EUNorth")
    pub name: String,
    /// Host address of the target server
    pub host: String,
    /// Port of the target server
    pub port: u16,
    /// Game ID for the target map
    pub game_id: i32,
    /// Key time to send in the next HelloPacket
    pub key_time: i32,
    /// Key bytes to send in the next HelloPacket
    pub key: Vec<u8>,
}

impl RotmgPacket for ReconnectPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let name = reader.read_string()?;
        let host = reader.read_string()?;
        let port = reader.read_u16()?;
        let game_id = reader.read_i32()?;
        let key_time = reader.read_i32()?;
        let key = reader.read_byte_array()?;

        Ok(Self {
            name,
            host,
            port,
            game_id,
            key_time,
            key,
        })
    }

    fn description(&self) -> String {
        format!("Reconnect: {} ({}:{})", self.name, self.host, self.port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_reconnect_bytes(name: &str, host: &str, port: u16) -> Vec<u8> {
        let mut data = Vec::new();

        // name: string
        data.extend_from_slice(&(name.len() as u16).to_be_bytes());
        data.extend_from_slice(name.as_bytes());
        // host: string
        data.extend_from_slice(&(host.len() as u16).to_be_bytes());
        data.extend_from_slice(host.as_bytes());
        // port: u16
        data.extend_from_slice(&port.to_be_bytes());
        // game_id: i32
        data.extend_from_slice(&(-2i32).to_be_bytes());
        // key_time: i32
        data.extend_from_slice(&12345i32.to_be_bytes());
        // key: byte array (u16 length prefix + bytes)
        let key = vec![0xAB, 0xCD, 0xEF];
        data.extend_from_slice(&(key.len() as u16).to_be_bytes());
        data.extend_from_slice(&key);

        data
    }

    #[test]
    fn test_reconnect_deserialize() {
        let data = build_reconnect_bytes("USWest", "54.67.119.48", 2050);
        let mut reader = PacketReader::new(&data);
        let packet = ReconnectPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.name, "USWest");
        assert_eq!(packet.host, "54.67.119.48");
        assert_eq!(packet.port, 2050);
        assert_eq!(packet.game_id, -2);
        assert_eq!(packet.key_time, 12345);
        assert_eq!(packet.key, vec![0xAB, 0xCD, 0xEF]);
    }

    #[test]
    fn test_reconnect_empty_name() {
        let data = build_reconnect_bytes("", "127.0.0.1", 2050);
        let mut reader = PacketReader::new(&data);
        let packet = ReconnectPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.name, "");
        assert_eq!(packet.host, "127.0.0.1");
    }

    #[test]
    fn test_reconnect_description() {
        let data = build_reconnect_bytes("EUNorth", "52.59.198.155", 2050);
        let mut reader = PacketReader::new(&data);
        let packet = ReconnectPacket::deserialize(&mut reader).unwrap();

        assert_eq!(
            packet.description(),
            "Reconnect: EUNorth (52.59.198.155:2050)"
        );
    }
}

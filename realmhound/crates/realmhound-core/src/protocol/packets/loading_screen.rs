//! Loading screen packet implementation.
//!
//! Received to toggle the loading screen (e.g. when saving in the vault).

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Loading screen packet (ID 222) - Incoming
///
/// Tells the client whether to display the loading screen. Single boolean
/// field (a single byte).
#[derive(Debug, Clone)]
pub struct LoadingScreenPacket {
    /// Whether the loading screen is being displayed.
    pub is_loading: bool,
}

impl RotmgPacket for LoadingScreenPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let is_loading = reader.read_bool()?;

        Ok(Self { is_loading })
    }

    fn description(&self) -> String {
        format!("LoadingScreen: is_loading={}", self.is_loading)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loading_screen_true() {
        let data = [1u8];
        let mut reader = PacketReader::new(&data);
        let packet = LoadingScreenPacket::deserialize(&mut reader).unwrap();

        assert!(packet.is_loading);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_loading_screen_false() {
        let data = [0u8];
        let mut reader = PacketReader::new(&data);
        let packet = LoadingScreenPacket::deserialize(&mut reader).unwrap();

        assert!(!packet.is_loading);
        assert_eq!(packet.description(), "LoadingScreen: is_loading=false");
    }
}

//! CustomMapListPacket implementation.
//!
//! Sent to request the list of custom maps. Carries no payload.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// CustomMapListPacket (ID 131) - Outgoing
#[derive(Debug, Clone, Default)]
pub struct CustomMapListPacket;

impl RotmgPacket for CustomMapListPacket {
    fn deserialize(_reader: &mut PacketReader) -> io::Result<Self> {
        Ok(Self)
    }

    fn description(&self) -> String {
        "CustomMapList".to_string()
    }
}

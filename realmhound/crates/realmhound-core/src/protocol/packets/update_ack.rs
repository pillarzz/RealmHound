//! UpdateAckPacket implementation.
//!
//! Sent to acknowledge an `UpdatePacket`. Carries no payload.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// UpdateAckPacket (ID 81) - Outgoing
#[derive(Debug, Clone, Default)]
pub struct UpdateAckPacket;

impl RotmgPacket for UpdateAckPacket {
    fn deserialize(_reader: &mut PacketReader) -> io::Result<Self> {
        Ok(Self)
    }

    fn description(&self) -> String {
        "UpdateAck".to_string()
    }
}

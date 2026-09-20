//! CancelTradePacket implementation.
//!
//! Sent to cancel the current active trade. Carries no payload.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// CancelTradePacket (ID 91) - Outgoing
#[derive(Debug, Clone)]
pub struct CancelTradePacket;

impl RotmgPacket for CancelTradePacket {
    fn deserialize(_reader: &mut PacketReader) -> io::Result<Self> {
        Ok(Self)
    }

    fn description(&self) -> String {
        "CancelTrade".to_string()
    }
}

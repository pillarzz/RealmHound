//! RealmScoreUpdate packet implementation.
//!
//! Received when the realm score changes (quests completed, events cleared).

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// RealmScoreUpdate packet (ID 169) - Incoming
///
/// Sent by server when the realm score changes. The score increases as
/// realm events and quests are completed, approaching the max score
/// which triggers Oryx's Castle.
#[derive(Debug, Clone)]
pub struct RealmScoreUpdatePacket {
    /// Current realm score
    pub score: i32,
}

impl RotmgPacket for RealmScoreUpdatePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let score = reader.read_i32()?;

        Ok(Self { score })
    }

    fn description(&self) -> String {
        format!("RealmScoreUpdate: score={}", self.score)
    }
}

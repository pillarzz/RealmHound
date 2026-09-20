//! QuestObjectId packet implementation.
//!
//! Received to tell the player the object ID of their current quest target
//! and a list of all active quest targets (realm heroes/events on minimap).

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// QuestObjectId packet (ID 82) - Incoming
///
/// Sent by server when quest targets change. Contains:
/// - The primary quest target object ID (red marker on minimap)
/// - A list of all active quest targets (realm heroes/events)
///
/// In realms, the `list` contains object IDs of all currently alive
/// realm heroes and events that appear on the minimap.
#[derive(Debug, Clone)]
pub struct QuestObjectIdPacket {
    /// The object ID of the current/primary quest target
    pub object_id: i32,
    /// List of all quest target object IDs (realm heroes, events, etc.)
    pub list: Vec<i32>,
}

impl RotmgPacket for QuestObjectIdPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let object_id = reader.read_i32()?;

        let list_len = reader.read_array_length()?;
        let mut list = Vec::with_capacity(list_len);
        for _ in 0..list_len {
            list.push(reader.read_compressed_int()?);
        }

        Ok(Self { object_id, list })
    }

    fn description(&self) -> String {
        format!(
            "QuestObjectId: primary={}, targets=[{}]",
            self.object_id,
            self.list
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

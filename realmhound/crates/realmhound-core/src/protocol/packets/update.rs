//! UpdatePacket implementation.
//!
//! Received when map updates occur: new tiles visible, objects enter/leave.
//! Also contains stat updates for objects including player's SEASONAL flag.

use super::traits::RotmgPacket;
use super::PacketReader;
use crate::protocol::data::{GroundTileData, ObjectData, WorldPosData};
use std::io;

/// UpdatePacket (ID 42) - Incoming
///
/// Sent by server when map updates occur:
/// - New tiles become visible
/// - Objects enter the visible area
/// - Objects leave the visible area
///
/// This packet contains ObjectData with stats, which is how we detect
/// if the player is a seasonal character (StatType.SEASONAL = 24).
#[derive(Debug, Clone)]
pub struct UpdatePacket {
    /// Player position (if moved, otherwise 0,0)
    pub pos: WorldPosData,
    /// Level type byte
    pub level_type: u8,
    /// New tiles that are now visible
    pub tiles: Vec<GroundTileData>,
    /// Objects that have entered the visible area
    pub new_objects: Vec<ObjectData>,
    /// Object IDs that have left the visible area
    pub drops: Vec<i32>,
}

impl RotmgPacket for UpdatePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let pos = WorldPosData::deserialize(reader)?;
        let level_type = reader.read_byte()?;

        // Read tiles array
        let tiles_len = reader.read_array_length()?;
        let mut tiles = Vec::with_capacity(tiles_len);
        for _ in 0..tiles_len {
            tiles.push(GroundTileData::deserialize(reader)?);
        }

        // Read new objects array
        let objects_len = reader.read_array_length()?;
        let mut new_objects = Vec::with_capacity(objects_len);
        for _ in 0..objects_len {
            new_objects.push(ObjectData::deserialize(reader)?);
        }

        // Read drops array (object IDs that left)
        let drops_len = reader.read_array_length()?;
        let mut drops = Vec::with_capacity(drops_len);
        for _ in 0..drops_len {
            drops.push(reader.read_compressed_int()?);
        }

        Ok(Self {
            pos,
            level_type,
            tiles,
            new_objects,
            drops,
        })
    }

    fn description(&self) -> String {
        format!(
            "Update: {} tiles, {} objects, {} drops",
            self.tiles.len(),
            self.new_objects.len(),
            self.drops.len()
        )
    }
}

impl UpdatePacket {
    /// Find an object by its object ID in the new_objects list.
    pub fn find_object(&self, object_id: i32) -> Option<&ObjectData> {
        self.new_objects
            .iter()
            .find(|obj| obj.object_id() == object_id)
    }

    /// Check if a specific object (by ID) is a seasonal character.
    /// Returns Some(true) for seasonal, Some(false) for regular, None if not found or no seasonal stat.
    pub fn is_object_seasonal(&self, object_id: i32) -> Option<bool> {
        self.find_object(object_id)
            .and_then(|obj| obj.is_seasonal())
    }

    /// Check if a specific object (by ID) has an active crucible challenge.
    ///
    /// Tri-state: `Some(true)`/`Some(false)` only when the object is present
    /// *and* carries the CRUCIBLE stat (non-empty = active, empty = deactivated);
    /// `None` when the object is absent or the snapshot simply omits the stat.
    /// The player's CRUCIBLE stat is not guaranteed to be in every full snapshot
    /// (it often arrives as a NewTick delta), so absence must not be read as
    /// not-crucible -- otherwise a later map load would clear a live-detected
    /// flag.
    pub fn is_object_crucible(&self, object_id: i32) -> Option<bool> {
        self.find_object(object_id)
            .and_then(|obj| obj.status.crucible_stat_state())
    }

    /// Remaining loot-drop-boost seconds for a specific object (by ID).
    ///
    /// `Some(secs)` when the object is present and carries the `LootDropTimer`
    /// stat (`0` = the boost just ended); `None` when the object is absent or
    /// the snapshot omits the stat (no information).
    pub fn loot_drop_timer_for(&self, object_id: i32) -> Option<i32> {
        self.find_object(object_id)
            .and_then(|obj| obj.status.loot_drop_timer())
    }

    /// Find all objects that have a SEASONAL stat and return their (object_id, is_seasonal) pairs.
    pub fn seasonal_objects(&self) -> Vec<(i32, bool)> {
        self.new_objects
            .iter()
            .filter_map(|obj| {
                obj.is_seasonal()
                    .map(|is_seasonal| (obj.object_id(), is_seasonal))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_update_packet_empty() {
        let mut data = Vec::new();

        // WorldPosData: x=0.0, y=0.0
        data.extend_from_slice(&0.0f32.to_be_bytes());
        data.extend_from_slice(&0.0f32.to_be_bytes());

        // level_type: 1
        data.push(1);

        // tiles: empty array
        data.push(0);

        // new_objects: empty array
        data.push(0);

        // drops: empty array
        data.push(0);

        let mut reader = PacketReader::new(&data);
        let packet = UpdatePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.pos.x, 0.0);
        assert_eq!(packet.pos.y, 0.0);
        assert_eq!(packet.level_type, 1);
        assert!(packet.tiles.is_empty());
        assert!(packet.new_objects.is_empty());
        assert!(packet.drops.is_empty());
    }
}

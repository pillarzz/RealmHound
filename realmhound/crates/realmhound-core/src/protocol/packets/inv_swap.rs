//! InvSwapPacket implementation.
//!
//! Sent by the client when the player swaps items between inventory slots.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Slot data for inventory operations.
#[derive(Debug, Clone)]
pub struct SlotObjectData {
    /// The object ID of the entity which owns the slot (player, vault chest, loot bag, etc.)
    pub object_id: i32,
    /// The slot index within that container.
    /// For vault chests, this is the ABSOLUTE index (0-7 for page 0, 8-15 for page 1, etc.)
    pub slot_id: i32,
    /// The item type ID in the slot, or -1 if empty
    pub item_type: i32,
}

impl SlotObjectData {
    /// Deserialize from packet data.
    pub fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let object_id = reader.read_i32()?;
        let slot_id = reader.read_i32()?;
        let item_type = reader.read_i32()?;
        Ok(Self {
            object_id,
            slot_id,
            item_type,
        })
    }
}

/// InvSwapPacket (ID 55) - Outgoing
///
/// Sent by the client when the player drags an item from one slot to another.
/// The slot_id field in SlotObjectData uses ABSOLUTE indices for containers
/// like vault chests (e.g., slot 16 = page 2, slot 0).
#[derive(Debug, Clone)]
pub struct InvSwapPacket {
    /// Player time at the time of editing inventory
    pub time: i32,
    /// Player X position
    pub player_x: f32,
    /// Player Y position  
    pub player_y: f32,
    /// The slot the item is being swapped from
    pub slot_from: SlotObjectData,
    /// The slot the item is being swapped to
    pub slot_to: SlotObjectData,
}

impl RotmgPacket for InvSwapPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let time = reader.read_i32()?;
        let player_x = reader.read_f32()?;
        let player_y = reader.read_f32()?;
        let slot_from = SlotObjectData::deserialize(reader)?;
        let slot_to = SlotObjectData::deserialize(reader)?;

        Ok(Self {
            time,
            player_x,
            player_y,
            slot_from,
            slot_to,
        })
    }

    fn description(&self) -> String {
        format!(
            "InvSwap: from obj={} slot={} to obj={} slot={}",
            self.slot_from.object_id,
            self.slot_from.slot_id,
            self.slot_to.object_id,
            self.slot_to.slot_id,
        )
    }
}

impl InvSwapPacket {
    /// Check if either slot involves a specific object ID (e.g., vault chest).
    pub fn involves_object(&self, object_id: i32) -> bool {
        self.slot_from.object_id == object_id || self.slot_to.object_id == object_id
    }

    /// Get the slot data for a specific object ID, if involved.
    pub fn get_slot_for_object(&self, object_id: i32) -> Option<&SlotObjectData> {
        if self.slot_from.object_id == object_id {
            Some(&self.slot_from)
        } else if self.slot_to.object_id == object_id {
            Some(&self.slot_to)
        } else {
            None
        }
    }

    /// Get the vault page for a slot ID (slot_id / 8).
    pub fn page_for_slot(slot_id: i32) -> usize {
        (slot_id / 8) as usize
    }
}

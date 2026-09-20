//! Movement record data type.
//!

use super::WorldPosData;
use crate::protocol::PacketReader;
use std::io;

/// Movement data of an entity moving to a point with a delta time.
#[derive(Debug, Clone, Copy, Default)]
pub struct MoveRecord {
    /// The client time of this move record.
    pub time: i32,
    /// The position the entity is moving to.
    pub pos: WorldPosData,
}

impl MoveRecord {
    /// Deserialize from packet reader.
    pub fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        Ok(Self {
            time: reader.read_i32()?,
            pos: WorldPosData::deserialize(reader)?,
        })
    }
}

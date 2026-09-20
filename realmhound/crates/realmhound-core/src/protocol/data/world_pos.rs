//! World position data type.

use crate::protocol::PacketReader;
use std::io;

/// World position coordinates.
#[derive(Debug, Clone, Copy, Default)]
pub struct WorldPosData {
    /// X coordinate
    pub x: f32,
    /// Y coordinate
    pub y: f32,
}

impl WorldPosData {
    /// Deserialize from packet reader.
    pub fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        Ok(Self {
            x: reader.read_f32()?,
            y: reader.read_f32()?,
        })
    }
}

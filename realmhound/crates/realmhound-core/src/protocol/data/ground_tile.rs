//! Ground tile data type.

use crate::protocol::PacketReader;
use std::io;

/// Ground tile information including position and type.
#[derive(Debug, Clone, Copy)]
pub struct GroundTileData {
    /// X coordinate of the tile
    pub x: i16,
    /// Y coordinate of the tile
    pub y: i16,
    /// Tile type ID
    pub tile_type: u16,
}

impl GroundTileData {
    /// Deserialize from packet reader.
    pub fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        Ok(Self {
            x: reader.read_i16()?,
            y: reader.read_i16()?,
            tile_type: reader.read_u16()?,
        })
    }
}

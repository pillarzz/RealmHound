//! Object reader for Unity serialized objects.

use super::reader::DataReader;
use super::serialized_type::SerializedType;
use std::io;

/// Unity class ID types we care about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ClassIDType {
    Unknown = -1,
    Texture2D = 28,
    TextAsset = 49,
    SpriteAtlas = 687078895,
}

impl From<i32> for ClassIDType {
    fn from(value: i32) -> Self {
        match value {
            28 => ClassIDType::Texture2D,
            49 => ClassIDType::TextAsset,
            687078895 => ClassIDType::SpriteAtlas,
            _ => ClassIDType::Unknown,
        }
    }
}

/// Object reader containing metadata about a serialized object.
#[derive(Debug)]
pub struct ObjectInfo {
    pub path_id: i64,
    pub byte_start: u64,
    pub byte_size: u64,
    pub type_id: i32,
    pub class_id: i32,
    pub class_type: ClassIDType,
}

impl ObjectInfo {
    /// Parse object info from the reader.
    pub fn parse(
        reader: &mut DataReader,
        version: u64,
        data_offset: u64,
        big_id_enabled: bool,
        types: &[SerializedType],
    ) -> io::Result<Self> {
        let path_id = if big_id_enabled {
            reader.read_i64()?
        } else if version < 14 {
            reader.read_i32()? as i64
        } else {
            reader.align_stream()?;
            reader.read_i64()?
        };

        let byte_start = if version >= 22 {
            reader.read_i64()? as u64
        } else {
            reader.read_i32()? as u64
        };
        let byte_start = byte_start + data_offset;

        let byte_size = reader.read_u32()? as u64;

        let type_id = reader.read_i32()?;

        let class_id = if version < 16 {
            reader.read_i16()? as i32
        } else {
            types
                .get(type_id as usize)
                .map(|t| t.class_id)
                .unwrap_or(-1)
        };

        let class_type = ClassIDType::from(class_id);

        if version < 11 {
            reader.read_u16()?; // is_destroyed
        } else if version < 17 {
            reader.read_i16()?; // script_type_index
        }

        if version == 15 || version == 16 {
            reader.read_byte()?; // stripped
        }

        Ok(Self {
            path_id,
            byte_start,
            byte_size,
            type_id,
            class_id,
            class_type,
        })
    }
}

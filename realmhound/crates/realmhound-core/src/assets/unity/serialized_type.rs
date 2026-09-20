//! Serialized type metadata.

use super::reader::DataReader;
use std::io;

/// Serialized type information for Unity objects.
#[derive(Debug, Clone)]
pub struct SerializedType {
    pub class_id: i32,
    pub is_stripped_type: bool,
    pub script_type_index: i16,
    pub script_id: [u8; 16],
    pub old_type_hash: [u8; 16],
}

impl SerializedType {
    /// Parse serialized types from the reader.
    pub fn parse_types(
        reader: &mut DataReader,
        version: u64,
        enable_type_tree: bool,
        type_count: usize,
    ) -> io::Result<Vec<Self>> {
        let mut types = Vec::with_capacity(type_count);

        for _ in 0..type_count {
            let class_id = reader.read_i32()?;

            let is_stripped_type = if version >= 16 {
                reader.read_bool()?
            } else {
                false
            };

            let script_type_index = if version >= 17 {
                reader.read_i16()?
            } else {
                -1
            };

            let mut script_id = [0u8; 16];
            let mut old_type_hash = [0u8; 16];

            if version >= 13 {
                if (version < 16 && class_id < 0) || (version >= 16 && class_id == 114) {
                    let bytes = reader.read_bytes(16)?;
                    script_id.copy_from_slice(&bytes);
                }
                let bytes = reader.read_bytes(16)?;
                old_type_hash.copy_from_slice(&bytes);
            }

            if enable_type_tree {
                // Skip type tree parsing - not needed for basic extraction
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "Type trees not supported",
                ));
            }

            types.push(SerializedType {
                class_id,
                is_stripped_type,
                script_type_index,
                script_id,
                old_type_hash,
            });
        }

        Ok(types)
    }
}

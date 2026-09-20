//! Object data type.

use super::ObjectStatusData;
use crate::protocol::PacketReader;
use std::io;

/// A game object with its type and status.
#[derive(Debug, Clone)]
pub struct ObjectData {
    /// The type ID of this object
    pub object_type: u16,
    /// Status information for this object
    pub status: ObjectStatusData,
}

impl ObjectData {
    /// Deserialize from packet reader.
    pub fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let object_type = reader.read_u16()?;
        let status = ObjectStatusData::deserialize(reader)?;

        Ok(Self {
            object_type,
            status,
        })
    }

    /// Get the object ID from the status.
    pub fn object_id(&self) -> i32 {
        self.status.object_id
    }

    /// Check if this object is a seasonal character.
    pub fn is_seasonal(&self) -> Option<bool> {
        self.status.is_seasonal()
    }

    /// True when this object has an active crucible challenge (non-empty
    /// CRUCIBLE stat).
    pub fn is_crucible_active(&self) -> bool {
        self.status.is_crucible_active()
    }
}

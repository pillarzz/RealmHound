//! Stat data type.

use super::stat_type::StatType;
use crate::protocol::PacketReader;
use std::io;

/// A single stat value for an object.
#[derive(Debug, Clone)]
pub struct StatData {
    /// The stat type ID
    pub stat_type_id: u8,
    /// The stat type (parsed from ID)
    pub stat_type: StatType,
    /// Integer stat value (if not a string stat)
    pub stat_value: i32,
    /// String stat value (if this is a string stat)
    pub string_stat_value: Option<String>,
    /// Secondary stat value
    pub stat_value_two: i32,
}

impl StatData {
    /// Deserialize from packet reader.
    pub fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let stat_type_id = reader.read_byte()?;
        let stat_type = StatType::from_id(stat_type_id);

        let (stat_value, string_stat_value) = if StatType::is_string_stat(stat_type_id) {
            (0, Some(reader.read_string()?))
        } else {
            (reader.read_compressed_int()?, None)
        };

        let stat_value_two = reader.read_compressed_int()?;

        Ok(Self {
            stat_type_id,
            stat_type,
            stat_value,
            string_stat_value,
            stat_value_two,
        })
    }

    /// Check if this is the SEASONAL stat.
    pub fn is_seasonal_stat(&self) -> bool {
        self.stat_type == StatType::Seasonal
    }

    /// Get the seasonal value (1 = seasonal, 0 = regular).
    /// Returns None if this is not the SEASONAL stat.
    pub fn seasonal_value(&self) -> Option<bool> {
        if self.is_seasonal_stat() {
            Some(self.stat_value == 1)
        } else {
            None
        }
    }

    /// True when this is the CRUCIBLE stat (128) carrying a non-empty payload,
    /// i.e. the character currently has an active crucible challenge. The stat is
    /// a string holding the active crucible id/JSON; empty means no active
    /// crucible.
    pub fn is_crucible_active(&self) -> bool {
        self.stat_type == StatType::Crucible
            && self
                .string_stat_value
                .as_deref()
                .is_some_and(|s| !s.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat(stat_type: StatType, string_value: Option<&str>) -> StatData {
        StatData {
            stat_type_id: stat_type as u8,
            stat_type,
            stat_value: 0,
            string_stat_value: string_value.map(|s| s.to_string()),
            stat_value_two: 0,
        }
    }

    #[test]
    fn crucible_active_only_for_non_empty_crucible_stat() {
        assert!(stat(StatType::Crucible, Some("c1")).is_crucible_active());
        assert!(!stat(StatType::Crucible, Some("")).is_crucible_active());
        assert!(!stat(StatType::Crucible, None).is_crucible_active());
        assert!(!stat(StatType::Dust, Some("c1")).is_crucible_active());
    }
}

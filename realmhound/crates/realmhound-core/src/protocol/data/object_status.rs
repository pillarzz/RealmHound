//! Object status data type.

use super::{StatData, WorldPosData};
use crate::protocol::PacketReader;
use std::io;

/// Status information for a game object, including position and stats.
#[derive(Debug, Clone)]
pub struct ObjectStatusData {
    /// The object ID this status is for
    pub object_id: i32,
    /// Position of the object
    pub pos: WorldPosData,
    /// Stats for this object
    pub stats: Vec<StatData>,
}

impl ObjectStatusData {
    /// Deserialize from packet reader.
    pub fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let object_id = reader.read_compressed_int()?;
        let pos = WorldPosData::deserialize(reader)?;

        let stats_len = reader.read_array_length()?;
        let mut stats = Vec::with_capacity(stats_len);
        for _ in 0..stats_len {
            stats.push(StatData::deserialize(reader)?);
        }

        Ok(Self {
            object_id,
            pos,
            stats,
        })
    }

    /// Find the SEASONAL stat if present.
    /// Returns Some(true) for seasonal character, Some(false) for regular, None if not present.
    pub fn is_seasonal(&self) -> Option<bool> {
        self.stats.iter().find_map(|stat| stat.seasonal_value())
    }

    /// True when this object carries a non-empty CRUCIBLE (128) stat, i.e. an
    /// active crucible challenge. A full object snapshot without the stat means
    /// no active crucible, so callers should treat a present object with `false`
    /// here as authoritatively not-crucible.
    pub fn is_crucible_active(&self) -> bool {
        self.stats.iter().any(|stat| stat.is_crucible_active())
    }

    /// Tri-state crucible reading for a status *delta* (e.g. NewTick), where the
    /// CRUCIBLE stat is only present when it changed:
    /// - `Some(true)`  -- CRUCIBLE stat present and non-empty (active),
    /// - `Some(false)` -- CRUCIBLE stat present but empty (deactivated),
    /// - `None`        -- CRUCIBLE stat absent from this delta (no information).
    ///
    /// Unlike [`Self::is_crucible_active`], absence is *not* interpreted as
    /// not-crucible, so a NewTick that simply omits an unchanged stat never
    /// clears a live-detected flag.
    pub fn crucible_stat_state(&self) -> Option<bool> {
        use crate::protocol::data::StatType;
        self.stats
            .iter()
            .find(|stat| stat.stat_type == StatType::Crucible)
            .map(|stat| {
                stat.string_stat_value
                    .as_deref()
                    .is_some_and(|s| !s.is_empty())
            })
    }

    /// Tri-state loot-drop-boost reading. Returns the `LootDropTimer` (67)
    /// stat's remaining seconds when the stat is present in this
    /// snapshot/delta (`0` means the boost just ended), or `None` when the stat
    /// is absent -- so a delta that simply omits an unchanged timer carries no
    /// information and must not clear a live-detected boost.
    pub fn loot_drop_timer(&self) -> Option<i32> {
        use crate::protocol::data::StatType;
        self.stats
            .iter()
            .find(|stat| stat.stat_type == StatType::LootDropTimer)
            .map(|stat| stat.stat_value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::data::StatType;

    fn crucible_stat(value: Option<&str>) -> StatData {
        StatData {
            stat_type_id: StatType::Crucible as u8,
            stat_type: StatType::Crucible,
            stat_value: 0,
            string_stat_value: value.map(|s| s.to_string()),
            stat_value_two: 0,
        }
    }

    fn status(stats: Vec<StatData>) -> ObjectStatusData {
        ObjectStatusData {
            object_id: 1,
            pos: WorldPosData { x: 0.0, y: 0.0 },
            stats,
        }
    }

    #[test]
    fn crucible_stat_state_is_tristate() {
        // Stat absent -> no information.
        assert_eq!(status(vec![]).crucible_stat_state(), None);
        // Present and non-empty -> active.
        assert_eq!(
            status(vec![crucible_stat(Some("c1"))]).crucible_stat_state(),
            Some(true)
        );
        // Present but empty -> deactivated.
        assert_eq!(
            status(vec![crucible_stat(Some(""))]).crucible_stat_state(),
            Some(false)
        );
        assert_eq!(
            status(vec![crucible_stat(None)]).crucible_stat_state(),
            Some(false)
        );
    }
}

//! Entity tracking for loot attribution.

use crate::protocol::data::{ObjectStatusData, StatData, StatType, WorldPosData};
use std::collections::HashMap;

/// A tracked entity in the game world (mob, bag, or player).
#[derive(Debug, Clone)]
pub struct LootEntity {
    /// Unique object ID in this map instance
    pub object_id: i32,
    /// Object type (determines what kind of entity this is)
    pub object_type: i32,
    /// Current position in the world
    pub pos: WorldPosData,
    /// Stats indexed by stat type ID
    pub stats: HashMap<u8, StatData>,
    /// Time this entity was created/spawned (milliseconds since epoch)
    pub creation_time: u64,
    /// Loot drop timer end time (0 if not active)
    pub loot_drop_time: u64,
    /// Loot tier timer end time (0 if not active)
    pub loot_tier_time: u64,
    /// Override mob ID string for attribution (e.g., "20493HM" for hard mode)
    pub loot_mob_id_override: Option<String>,
    /// True if this is a fabricated entity for attribution (no actual world entity)
    pub fabricated_attribution: bool,
}

impl LootEntity {
    /// Create a new entity with the given ID.
    pub fn new(object_id: i32, object_type: i32, time: u64) -> Self {
        Self {
            object_id,
            object_type,
            pos: WorldPosData { x: 0.0, y: 0.0 },
            stats: HashMap::new(),
            creation_time: time,
            loot_drop_time: 0,
            loot_tier_time: 0,
            loot_mob_id_override: None,
            fabricated_attribution: false,
        }
    }

    /// Create a fabricated entity for attribution purposes.
    pub fn fabricated(object_type: i32, time: u64) -> Self {
        let mut entity = Self::new(-1, object_type, time);
        entity.fabricated_attribution = true;
        entity
    }

    /// Update entity from ObjectStatusData.
    pub fn update_stats(&mut self, status: &ObjectStatusData, time_ms: u64) {
        self.pos = status.pos.clone();

        for stat in &status.stats {
            self.stats.insert(stat.stat_type_id, stat.clone());

            // Update loot timers
            match stat.stat_type {
                StatType::LootDropTimer => {
                    self.loot_drop_time = 0;
                    if stat.stat_value > 0 {
                        self.loot_drop_time = stat.stat_value as u64 * 1000 + time_ms;
                    }
                }
                StatType::LootTierTimer => {
                    self.loot_tier_time = 0;
                    if stat.stat_value > 0 {
                        self.loot_tier_time = stat.stat_value as u64 * 1000 + time_ms;
                    }
                }
                _ => {}
            }
        }
    }

    /// Get a stat value by type.
    pub fn get_stat(&self, stat_type: StatType) -> Option<&StatData> {
        self.stats.get(&(stat_type as u8))
    }

    /// Get inventory item IDs (slots 0-7).
    /// Returns -1 for empty slots.
    pub fn get_inventory(&self) -> [i32; 8] {
        let mut inventory = [-1i32; 8];
        for i in 0..8 {
            let stat_id = StatType::Inventory0 as u8 + i as u8;
            if let Some(stat) = self.stats.get(&stat_id) {
                inventory[i] = stat.stat_value;
            }
        }
        inventory
    }

    /// Get the unique data string (enchantment data).
    pub fn get_unique_data_string(&self) -> Option<&str> {
        self.stats
            .get(&(StatType::UniqueDataString as u8))
            .and_then(|s| s.string_stat_value.as_deref())
    }

    /// Calculate squared distance to a position.
    pub fn dist_sqrd(&self, other_pos: &WorldPosData) -> f32 {
        let dx = self.pos.x - other_pos.x;
        let dy = self.pos.y - other_pos.y;
        dx * dx + dy * dy
    }

    /// Get remaining loot drop time in milliseconds.
    /// Returns 0 if timer is not active or has expired.
    pub fn loot_drop_time_remaining(&self, current_time: u64) -> u64 {
        if self.loot_drop_time > current_time {
            self.loot_drop_time - current_time
        } else {
            0
        }
    }

    /// Get remaining loot tier time in milliseconds.
    /// Returns 0 if timer is not active or has expired.
    pub fn loot_tier_time_remaining(&self, current_time: u64) -> u64 {
        if self.loot_tier_time > current_time {
            self.loot_tier_time - current_time
        } else {
            0
        }
    }
}

/// Tracked player state for loot tracking.
///
/// This captures player info at the moment of a loot drop for historical records.
#[derive(Debug, Clone)]
pub struct TrackedPlayer {
    /// Character ID from CreateSuccessPacket
    pub char_id: i32,
    /// Player object ID in current map
    pub object_id: i32,
    /// Object type (class ID)
    pub object_type: i32,
    /// Player name
    pub name: String,
    /// Skin ID (or 0 for default skin)
    pub skin_id: i32,
    /// Current fame
    pub fame: i32,
    /// Whether this is a seasonal character
    pub is_seasonal: bool,
    /// Clothing dye/cloth texture (StatType::Texture1)
    pub tex1: u32,
    /// Accessory dye/cloth texture (StatType::Texture2)
    pub tex2: u32,
    /// Loot drop timer end time
    pub loot_drop_time: u64,
    /// Loot tier timer end time
    pub loot_tier_time: u64,
}

impl TrackedPlayer {
    /// Create a new tracked player.
    pub fn new(char_id: i32, object_id: i32, object_type: i32) -> Self {
        Self {
            char_id,
            object_id,
            object_type,
            name: String::new(),
            skin_id: 0,
            fame: 0,
            is_seasonal: false,
            tex1: 0,
            tex2: 0,
            loot_drop_time: 0,
            loot_tier_time: 0,
        }
    }

    /// Update player stats from StatData.
    pub fn update_from_stats(&mut self, stats: &[StatData], time_ms: u64) {
        for stat in stats {
            match stat.stat_type {
                StatType::Name => {
                    if let Some(ref name) = stat.string_stat_value {
                        self.name = name.clone();
                    }
                }
                StatType::SkinId => {
                    self.skin_id = stat.stat_value;
                }
                StatType::CurrFame => {
                    self.fame = stat.stat_value;
                }
                StatType::Seasonal => {
                    self.is_seasonal = stat.stat_value == 1;
                }
                StatType::Texture1 => {
                    self.tex1 = stat.stat_value as u32;
                }
                StatType::Texture2 => {
                    self.tex2 = stat.stat_value as u32;
                }
                StatType::LootDropTimer => {
                    self.loot_drop_time = 0;
                    if stat.stat_value > 0 {
                        self.loot_drop_time = stat.stat_value as u64 * 1000 + time_ms;
                    }
                }
                StatType::LootTierTimer => {
                    self.loot_tier_time = 0;
                    if stat.stat_value > 0 {
                        self.loot_tier_time = stat.stat_value as u64 * 1000 + time_ms;
                    }
                }
                _ => {}
            }
        }
    }

    /// Get the display icon ID (skin if set, otherwise class).
    pub fn icon_id(&self) -> i32 {
        if self.skin_id > 0 {
            self.skin_id
        } else {
            self.object_type
        }
    }

    /// Get remaining loot drop time in milliseconds.
    pub fn loot_drop_time_remaining(&self, current_time: u64) -> u64 {
        if self.loot_drop_time > current_time {
            self.loot_drop_time - current_time
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entity_creation() {
        let entity = LootEntity::new(123, 1292, 1000);
        assert_eq!(entity.object_id, 123);
        assert_eq!(entity.object_type, 1292);
        assert_eq!(entity.creation_time, 1000);
        assert!(!entity.fabricated_attribution);
    }

    #[test]
    fn test_fabricated_entity() {
        let entity = LootEntity::fabricated(20493, 2000);
        assert_eq!(entity.object_id, -1);
        assert_eq!(entity.object_type, 20493);
        assert!(entity.fabricated_attribution);
    }

    #[test]
    fn test_dist_sqrd() {
        let mut entity = LootEntity::new(1, 1, 0);
        entity.pos = WorldPosData { x: 10.0, y: 10.0 };

        let other = WorldPosData { x: 13.0, y: 14.0 };
        let dist = entity.dist_sqrd(&other);

        // 3^2 + 4^2 = 9 + 16 = 25
        assert!((dist - 25.0).abs() < 0.001);
    }

    #[test]
    fn test_tracked_player() {
        let mut player = TrackedPlayer::new(1234, 5678, 782);
        assert_eq!(player.char_id, 1234);
        assert_eq!(player.object_id, 5678);
        assert_eq!(player.icon_id(), 782); // No skin, returns class

        player.skin_id = 9000;
        assert_eq!(player.icon_id(), 9000); // Has skin, returns skin
    }
}

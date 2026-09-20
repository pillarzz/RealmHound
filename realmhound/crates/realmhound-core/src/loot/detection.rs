//! Loot bag detection and tracking.
//!
//! Implements a double-buffer pattern for batching bags per tick.

use super::bag_types::{is_any_loot_bag, LootBagType};
use super::entity::LootEntity;
use crate::api::character::parse_enchant_ids;
use crate::assets::item_category::{ItemCategorizer, ItemSubCategory};
use crate::assets::{get_asset_manager, EnchantmentTier};
use crate::protocol::data::{ObjectStatusData, StatType, WorldPosData};
use crate::settings::LootTrackingSettings;
use std::collections::{HashMap, HashSet};

/// Pending loot bag waiting for attribution.
#[derive(Debug, Clone)]
pub struct PendingBag {
    /// The loot entity (bag)
    pub entity: LootEntity,
    /// Items in the bag (slot -> item_id)
    pub items: [i32; 8],
    /// Enchant data per slot (parsed from UNIQUE_DATA_STRING)
    pub enchants: [Option<String>; 8],
}

impl PendingBag {
    /// Create a new pending bag from an entity.
    pub fn new(entity: LootEntity) -> Self {
        Self {
            entity,
            items: [-1; 8],
            enchants: Default::default(),
        }
    }

    /// Parse inventory and enchants from the entity's stats.
    pub fn parse_inventory(&mut self) {
        // Parse items from INVENTORY_0 through INVENTORY_7
        for i in 0..8 {
            let stat_id = StatType::Inventory0 as u8 + i as u8;
            if let Some(stat) = self.entity.stats.get(&stat_id) {
                if stat.stat_value > 0 {
                    self.items[i] = stat.stat_value;
                }
            }
        }

        // Parse enchants from UNIQUE_DATA_STRING
        // Format: "enchant1,enchant2,enchant3,..." (base64 encoded per slot)
        if let Some(udata) = self.entity.get_unique_data_string() {
            let parts: Vec<&str> = udata.split(',').collect();
            for (i, part) in parts.iter().enumerate().take(8) {
                if !part.is_empty() && *part != "AAIE_f_9__3__f8=" {
                    // AAIE_f_9__3__f8= is the "empty" marker
                    self.enchants[i] = Some(part.to_string());
                }
            }
        }
    }

    /// Get the bag type.
    pub fn bag_type(&self) -> Option<LootBagType> {
        LootBagType::from_id(self.entity.object_type)
    }

    /// Check if this bag has any items.
    pub fn has_items(&self) -> bool {
        self.items.iter().any(|&id| id > 0)
    }

    /// Get non-empty item IDs.
    pub fn item_ids(&self) -> Vec<i32> {
        self.items.iter().filter(|&&id| id > 0).copied().collect()
    }

    /// Check if any item in this bag has valuable enchantments.
    /// Valuable = Unique, Awakened, or High Loot Bonus (III/IV).
    pub fn has_valuable_enchant(&self) -> bool {
        let manager = get_asset_manager();
        for enchant_str in self.enchants.iter().flatten() {
            let enchant_ids = parse_enchant_ids(enchant_str);
            if manager.has_any_valuable_enchant(&enchant_ids) {
                return true;
            }
        }
        false
    }

    /// Check if any item in this bag has an enchant matching the enabled settings.
    pub fn has_enchant_matching_settings(&self, settings: &LootTrackingSettings) -> bool {
        let manager = get_asset_manager();
        for enchant_str in self.enchants.iter().flatten() {
            let enchant_ids = parse_enchant_ids(enchant_str);
            for &eid in &enchant_ids {
                if let Some(tier) = manager.enchant_tier(eid) {
                    match tier {
                        EnchantmentTier::HighLootBonus if settings.track_loot_enchants => {
                            return true
                        }
                        EnchantmentTier::Unique if settings.track_unique_enchants => return true,
                        EnchantmentTier::Awakened if settings.track_awakened_enchants => {
                            return true
                        }
                        _ => {}
                    }
                }
            }
        }
        false
    }

    /// Check if any item in this bag is a shiny item.
    pub fn has_shiny_item(&self) -> bool {
        let manager = get_asset_manager();
        for &item_id in &self.items {
            if item_id > 0 && manager.is_shiny(item_id) {
                return true;
            }
        }
        false
    }

    /// Check if any item in this bag is Legendary+ rarity.
    /// Legendary = 3 enchants, Divine = 4 enchants on a single item.
    pub fn has_legendary_plus_item(&self) -> bool {
        for enchant_str in self.enchants.iter().flatten() {
            let enchant_ids = parse_enchant_ids(enchant_str);
            if enchant_ids.len() >= 3 {
                return true;
            }
        }
        false
    }

    /// Check if any item in this bag is a UT (Untiered) item.
    pub fn has_ut_item(&self) -> bool {
        let manager = get_asset_manager();
        for &item_id in &self.items {
            if item_id > 0 && manager.is_ut(item_id) {
                return true;
            }
        }
        false
    }

    /// Check if any item in this bag is a potion (stat potions, soulbound potions, greater potions).
    pub fn has_potion(&self) -> bool {
        let manager = get_asset_manager();
        for &item_id in &self.items {
            if item_id > 0 {
                if let Some(asset) = manager.get_object(item_id) {
                    let (_, sub) = ItemCategorizer::categorize(&asset);
                    if matches!(
                        sub,
                        ItemSubCategory::Potions
                            | ItemSubCategory::PotionsSB
                            | ItemSubCategory::GreaterPotions
                    ) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Check if any item in this bag is a mark.
    pub fn has_mark(&self) -> bool {
        let manager = get_asset_manager();
        for &item_id in &self.items {
            if item_id > 0 {
                if let Some(asset) = manager.get_object(item_id) {
                    let (_, sub) = ItemCategorizer::categorize(&asset);
                    if sub == ItemSubCategory::Marks {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Check if any item in this bag is a tiered item at or above the given tier.
    pub fn has_tiered_at_or_above(&self, min_tier: i32) -> bool {
        let manager = get_asset_manager();
        for &item_id in &self.items {
            if item_id > 0 {
                if let Some(asset) = manager.get_object(item_id) {
                    if let Some(tier) = asset.get_tier() {
                        if tier >= min_tier {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    /// Check if this bag's type is enabled in tracking settings.
    fn bag_type_enabled(bag_type: LootBagType, settings: &LootTrackingSettings) -> bool {
        match bag_type {
            LootBagType::White | LootBagType::BoostedWhite => settings.track_white,
            LootBagType::Orange | LootBagType::BoostedOrange => settings.track_orange,
            LootBagType::Red | LootBagType::BoostedRed => settings.track_red,
            LootBagType::Gold | LootBagType::BoostedGold => settings.track_gold,
            LootBagType::Blue | LootBagType::BoostedBlue => settings.track_blue,
            LootBagType::Egg | LootBagType::BoostedEgg => settings.track_egg,
            LootBagType::Teal | LootBagType::BoostedTeal => settings.track_teal,
            LootBagType::Purple | LootBagType::BoostedPurple => settings.track_purple,
            LootBagType::Pink | LootBagType::BoostedPink => settings.track_pink,
            LootBagType::Soulbound => settings.track_soulbound,
            LootBagType::Brown | LootBagType::BoostedBrown => settings.track_brown,
        }
    }

    /// Check if this bag should be tracked based on settings.
    ///
    /// A bag is tracked if its bag type is enabled (Section 1) OR if it
    /// contains at least one item matching an enabled content filter (Section 2).
    pub fn should_be_tracked_with_settings(&self, settings: &LootTrackingSettings) -> bool {
        let bag_type = match self.bag_type() {
            Some(bt) => bt,
            None => return false,
        };

        // Section 1: bag type enabled
        if Self::bag_type_enabled(bag_type, settings) {
            return true;
        }

        // Section 2: content filters
        if settings.track_shiny && self.has_shiny_item() {
            return true;
        }

        if self.has_enchant_matching_settings(settings) {
            return true;
        }

        if settings.track_legendary_plus && self.has_legendary_plus_item() {
            return true;
        }

        if settings.track_ut_items && self.has_ut_item() {
            return true;
        }

        if settings.track_potions && self.has_potion() {
            return true;
        }

        if settings.track_marks && self.has_mark() {
            return true;
        }

        if settings.track_tiered_items && self.has_tiered_at_or_above(settings.min_tiered_tier) {
            return true;
        }

        false
    }

    /// Check if this bag should be tracked using default settings.
    /// Used for backward compatibility where no custom settings are available.
    pub fn should_be_tracked(&self) -> bool {
        self.should_be_tracked_with_settings(&LootTrackingSettings::default())
    }
}

/// Killed entity info for attribution.
#[derive(Debug, Clone)]
pub struct KilledEntity {
    /// The entity that was killed
    pub entity: LootEntity,
    /// Time when killed (milliseconds since epoch)
    pub kill_time: u64,
}

/// Loot detection state.
///
/// Uses a double-buffer pattern (alternating per-tick containers)
/// to batch bags per tick and process them together.
pub struct LootDetector {
    /// Set of bag object IDs already processed (prevents duplicates)
    seen_bag_ids: HashSet<i32>,

    /// Double buffer for batching bags per tick
    tick_buffers: [Vec<PendingBag>; 2],

    /// Current buffer index (0 or 1)
    tick_toggle: usize,

    /// Entities killed this tick (for attribution)
    killed_entities: Vec<KilledEntity>,

    /// All tracked entities (for position lookup)
    entity_list: HashMap<i32, LootEntity>,

    /// Entities that the player has hit (for loot attribution)
    /// Only entities in this set can be considered as droppers.
    entity_hit_list: HashSet<i32>,

    /// Current map seed (for attribution)
    map_seed: i32,

    /// Current map name
    map_name: String,
}

impl Default for LootDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl LootDetector {
    /// Create a new loot detector.
    pub fn new() -> Self {
        Self {
            seen_bag_ids: HashSet::new(),
            tick_buffers: [Vec::new(), Vec::new()],
            tick_toggle: 0,
            killed_entities: Vec::new(),
            entity_list: HashMap::new(),
            entity_hit_list: HashSet::new(),
            map_seed: -1,
            map_name: String::new(),
        }
    }

    /// Called when entering a new map.
    pub fn on_new_map(&mut self, map_name: &str, map_seed: i32) {
        self.clear();
        self.map_name = map_name.to_string();
        self.map_seed = map_seed;
    }

    /// Clear all state (on map change or disconnect).
    pub fn clear(&mut self) {
        self.seen_bag_ids.clear();
        self.tick_buffers[0].clear();
        self.tick_buffers[1].clear();
        self.killed_entities.clear();
        self.entity_list.clear();
        self.entity_hit_list.clear();
        self.map_seed = -1;
        self.map_name.clear();
    }

    /// Get current map name.
    pub fn map_name(&self) -> &str {
        &self.map_name
    }

    /// Get current map seed.
    pub fn map_seed(&self) -> i32 {
        self.map_seed
    }

    /// Check if an object type is a loot bag (any type).
    pub fn is_loot_bag(object_type: i32) -> bool {
        is_any_loot_bag(object_type)
    }

    /// Process a new object spawn from Update packet.
    /// Returns true if this was a new loot bag.
    ///
    pub fn on_object_spawn(
        &mut self,
        object_id: i32,
        object_type: i32,
        status: &ObjectStatusData,
        time_ms: u64,
    ) -> bool {
        // Update entity list
        let entity = self
            .entity_list
            .entry(object_id)
            .or_insert_with(|| LootEntity::new(object_id, object_type, time_ms));
        entity.update_stats(status, time_ms);

        // Check if this is a new loot bag
        if Self::is_loot_bag(object_type) && !self.seen_bag_ids.contains(&object_id) {
            self.seen_bag_ids.insert(object_id);

            // Create pending bag and parse inventory
            let mut pending = PendingBag::new(entity.clone());
            pending.parse_inventory();

            let bag_type = pending.bag_type();
            let item_count = pending.items.iter().filter(|&&id| id > 0).count();
            tracing::trace!(
                "[LOOT_DETECT] New bag spawn: id={}, type={:?}, items={}, should_track={}",
                object_id,
                bag_type,
                item_count,
                pending.should_be_tracked()
            );

            // Add to current tick buffer
            self.tick_buffers[self.tick_toggle].push(pending);
            return true;
        }

        false
    }

    /// Process an object status update.
    pub fn on_object_update(&mut self, object_id: i32, status: &ObjectStatusData, time_ms: u64) {
        if let Some(entity) = self.entity_list.get_mut(&object_id) {
            entity.update_stats(status, time_ms);
        }
    }

    /// Record that the player has hit an entity.
    /// Only entities in the hit list can be considered as loot droppers.
    pub fn on_entity_hit(&mut self, target_id: i32) {
        self.entity_hit_list.insert(target_id);
    }

    /// Check if an entity has been hit by the player.
    pub fn was_entity_hit(&self, object_id: i32) -> bool {
        self.entity_hit_list.contains(&object_id)
    }

    /// Record an entity being killed (for attribution).
    /// Only adds to killed list if the entity was in the hit list (player damaged it).
    pub fn on_entity_dropped(&mut self, object_id: i32, time_ms: u64) {
        // Normal droppers are entities the player has hit. A small whitelist of
        // "forced droppers" (Ice Tomb souls) drop loot without ever being hit,
        // so they count as droppers too.
        if let Some(entity) = self.entity_list.get(&object_id).cloned() {
            let forced = super::is_forced_loot_dropper(entity.object_type);
            if !(self.entity_hit_list.contains(&object_id) || forced) {
                return;
            }
            // Never attribute a bag to an environmental structure (wall/gate/
            // pillar/room-check). Such objects can register hits via
            // EnemyOccupySquare and get removed when a gate unlocks, stealing
            // attribution from the real nearby enemy. Only block objects the
            // catalog positively knows are non-valid sources (fail-open on
            // unknown types); forced droppers bypass the check.
            if !forced {
                let mgr = get_asset_manager();
                let known_invalid = mgr.object_class(entity.object_type).is_some()
                    && !mgr.is_valid_drop_source(entity.object_type);
                if known_invalid {
                    return;
                }
            }
            self.killed_entities.push(KilledEntity {
                entity,
                kill_time: time_ms,
            });
        }
    }

    /// Remove an entity (on death or despawn).
    /// Also removes from hit list.
    pub fn on_entity_removed(&mut self, object_id: i32) {
        self.entity_list.remove(&object_id);
        self.entity_hit_list.remove(&object_id);
    }

    /// Get an entity by ID.
    pub fn get_entity(&self, object_id: i32) -> Option<&LootEntity> {
        self.entity_list.get(&object_id)
    }

    /// Begin a new loot tick - swap buffers.
    /// Returns the bags from the previous tick for processing.
    pub fn begin_tick(&mut self) -> Vec<PendingBag> {
        // Swap to other buffer
        self.tick_toggle ^= 1;

        // Take bags from the now-current buffer (which was previous tick's buffer)
        std::mem::take(&mut self.tick_buffers[self.tick_toggle])
    }

    /// Get killed entities for attribution.
    /// These are cleared at end of tick.
    pub fn killed_entities(&self) -> &[KilledEntity] {
        &self.killed_entities
    }

    /// End the loot tick - clear killed entities.
    pub fn end_tick(&mut self) {
        self.killed_entities.clear();
    }

    /// Find the closest killed entity to a bag position.
    /// Used for basic attribution (more complex logic in attribution module).
    pub fn find_closest_killed(&self, bag_pos: &WorldPosData) -> Option<&KilledEntity> {
        self.killed_entities.iter().min_by(|a, b| {
            let dist_a = a.entity.dist_sqrd(bag_pos);
            let dist_b = b.entity.dist_sqrd(bag_pos);
            dist_a
                .partial_cmp(&dist_b)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    /// Get statistics about the detector state for debugging.
    /// Returns (seen_bags, entities, hit_entities, pending_tick_0, pending_tick_1)
    pub fn debug_stats(&self) -> (usize, usize, usize, usize, usize) {
        (
            self.seen_bag_ids.len(),
            self.entity_list.len(),
            self.entity_hit_list.len(),
            self.tick_buffers[0].len(),
            self.tick_buffers[1].len(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::data::StatData;

    fn make_status(x: f32, y: f32) -> ObjectStatusData {
        ObjectStatusData {
            object_id: 0,
            pos: WorldPosData { x, y },
            stats: vec![],
        }
    }

    fn make_status_with_inventory(x: f32, y: f32, items: &[i32]) -> ObjectStatusData {
        let mut stats = Vec::new();
        for (i, &item_id) in items.iter().enumerate().take(8) {
            if item_id > 0 {
                let stat_type_id = StatType::Inventory0 as u8 + i as u8;
                stats.push(StatData {
                    stat_type_id,
                    stat_type: StatType::from_id(stat_type_id),
                    stat_value: item_id,
                    string_stat_value: None,
                    stat_value_two: 0,
                });
            }
        }
        ObjectStatusData {
            object_id: 0,
            pos: WorldPosData { x, y },
            stats,
        }
    }

    #[test]
    fn test_new_detector() {
        let detector = LootDetector::new();
        assert!(detector.seen_bag_ids.is_empty());
        assert!(detector.killed_entities.is_empty());
    }

    #[test]
    fn test_on_new_map() {
        let mut detector = LootDetector::new();
        detector.on_new_map("Oryx's Castle", 12345);
        assert_eq!(detector.map_name(), "Oryx's Castle");
        assert_eq!(detector.map_seed(), 12345);
    }

    #[test]
    fn test_is_loot_bag() {
        // White bag = 1292
        assert!(LootDetector::is_loot_bag(1292));
        // Orange bag = 1291
        assert!(LootDetector::is_loot_bag(1291));
        // Brown bag = 1281 (not a "notable" loot drop but still a loot bag)
        assert!(!LootDetector::is_loot_bag(1281)); // Brown bags are filtered out
                                                   // Random object
        assert!(!LootDetector::is_loot_bag(12345));
    }

    #[test]
    fn test_object_spawn_loot_bag() {
        let mut detector = LootDetector::new();
        let status = make_status_with_inventory(10.0, 20.0, &[1234, 5678]);

        // First spawn should register
        let is_new = detector.on_object_spawn(100, 1292, &status, 1000);
        assert!(is_new);

        // Same ID should not register again
        let is_new2 = detector.on_object_spawn(100, 1292, &status, 1001);
        assert!(!is_new2);

        // Different ID should register
        let is_new3 = detector.on_object_spawn(101, 1292, &status, 1002);
        assert!(is_new3);
    }

    #[test]
    fn test_tick_double_buffer() {
        let mut detector = LootDetector::new();
        let status = make_status_with_inventory(10.0, 20.0, &[1234]);

        // Spawn a bag (goes to buffer[toggle=0])
        detector.on_object_spawn(100, 1292, &status, 1000);

        // Begin tick 1: toggles to 1, returns buffer[1] which is empty
        // (bag 100 is in buffer[0], not processed yet)
        let bags = detector.begin_tick();
        assert_eq!(bags.len(), 0, "First tick should be empty");

        // Spawn another bag (goes to buffer[toggle=1])
        detector.on_object_spawn(101, 1295, &status, 2000); // Orange bag (1295)

        // Begin tick 2: toggles to 0, returns buffer[0] which has bag 100
        let bags2 = detector.begin_tick();
        assert_eq!(bags2.len(), 1);
        assert_eq!(bags2[0].entity.object_id, 100);

        // Begin tick 3: toggles to 1, returns buffer[1] which has bag 101
        let bags3 = detector.begin_tick();
        assert_eq!(bags3.len(), 1);
        assert_eq!(bags3[0].entity.object_id, 101);
    }

    #[test]
    fn test_killed_entities() {
        let mut detector = LootDetector::new();
        let status = make_status(50.0, 50.0);

        // Add an entity
        detector.on_object_spawn(200, 9999, &status, 1000);

        // Mark it as hit first (required for attribution)
        detector.on_entity_hit(200);

        // Drop it (only works if it was hit)
        detector.on_entity_dropped(200, 1500);
        assert_eq!(detector.killed_entities().len(), 1);

        // End tick clears killed entities
        detector.end_tick();
        assert!(detector.killed_entities().is_empty());
    }

    #[test]
    fn test_killed_entities_requires_hit() {
        let mut detector = LootDetector::new();
        let status = make_status(50.0, 50.0);

        // Add an entity
        detector.on_object_spawn(200, 9999, &status, 1000);

        // Drop it WITHOUT hitting it first
        detector.on_entity_dropped(200, 1500);

        // Should NOT be in killed list (wasn't hit)
        assert!(detector.killed_entities().is_empty());
    }

    #[test]
    fn test_forced_dropper_counts_without_hit() {
        let mut detector = LootDetector::new();
        let status = make_status(50.0, 50.0);

        // An Ice Tomb soul (object type 45638) drops loot without ever being
        // hit by the player, so it must count as a dropper anyway.
        detector.on_object_spawn(200, 45638, &status, 1000);
        detector.on_entity_dropped(200, 1500);

        assert_eq!(detector.killed_entities().len(), 1);
        assert_eq!(detector.killed_entities()[0].entity.object_type, 45638);
    }

    #[test]
    fn test_find_closest_killed() {
        let mut detector = LootDetector::new();

        // Add entities at different positions
        let status1 = make_status(10.0, 10.0);
        let status2 = make_status(50.0, 50.0);
        detector.on_object_spawn(201, 9999, &status1, 1000);
        detector.on_object_spawn(202, 9998, &status2, 1000);

        // Hit both entities
        detector.on_entity_hit(201);
        detector.on_entity_hit(202);

        // Drop both
        detector.on_entity_dropped(201, 1500);
        detector.on_entity_dropped(202, 1500);

        // Find closest to (12, 12) - should be entity 201 at (10,10)
        let bag_pos = WorldPosData { x: 12.0, y: 12.0 };
        let closest = detector.find_closest_killed(&bag_pos);
        assert!(closest.is_some());
        assert_eq!(closest.unwrap().entity.object_id, 201);
    }

    #[test]
    fn test_pending_bag_inventory() {
        let mut entity = LootEntity::new(100, 1292, 1000);
        entity.stats.insert(
            StatType::Inventory0 as u8,
            StatData {
                stat_type_id: StatType::Inventory0 as u8,
                stat_type: StatType::Inventory0,
                stat_value: 1234,
                string_stat_value: None,
                stat_value_two: 0,
            },
        );
        entity.stats.insert(
            StatType::Inventory2 as u8,
            StatData {
                stat_type_id: StatType::Inventory2 as u8,
                stat_type: StatType::Inventory2,
                stat_value: 5678,
                string_stat_value: None,
                stat_value_two: 0,
            },
        );

        let mut pending = PendingBag::new(entity);
        pending.parse_inventory();

        assert_eq!(pending.items[0], 1234);
        assert_eq!(pending.items[1], -1);
        assert_eq!(pending.items[2], 5678);
        assert!(pending.has_items());
        assert_eq!(pending.item_ids(), vec![1234, 5678]);
    }
}

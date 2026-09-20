//! Loot drop attribution system.
//!
//! Attributes loot bag drops to the mobs that dropped them.
//! Handles delayed drops via text packet triggers (boss death messages).

use super::entity::LootEntity;
use crate::protocol::data::WorldPosData;

/// Known special mob IDs for attribution windows.
pub mod boss_ids {
    /// Kitsune Umi (Moonlight Village)
    pub const UMI_KITSUNE: i32 = 20493;
    /// Dancer Miko (Moonlight Village)
    pub const MIKO_DANCER: i32 = 20451;
    /// Void Entity (The Void)
    pub const VOID_ENTITY: i32 = 45076;
    /// Bridge Sentinel (The Shatters)
    pub const BRIDGE_SENTINEL: i32 = 29003;
    /// Twilight Archmage (The Shatters)
    pub const TWILIGHT_ARCHMAGE: i32 = 29021;
    /// Accursed King (The Shatters)
    pub const ACCURSED_KING: i32 = 29039;
}

/// Variant suffixes for hard mode / true variant bosses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariantSuffix {
    /// Hard Mode variant
    HardMode,
    /// True Revelation variant (Umi)
    TrueRevelation,
}

impl VariantSuffix {
    /// Get the suffix string for mob ID override.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::HardMode => "HM",
            Self::TrueRevelation => "TR",
        }
    }
}

/// Result of a text packet trigger check.
#[derive(Debug, Clone)]
pub struct AttributionTrigger {
    /// The mob ID to attribute drops to
    pub mob_id: i32,
    /// Number of ticks to keep the attribution window open
    pub ticks: u32,
    /// Optional variant suffix (HM/TR)
    pub variant: Option<VariantSuffix>,
}

/// Loot attribution manager.
///
/// Handles attributing loot drops to mobs, including delayed drops
/// from bosses that have death animations.
#[derive(Debug, Default)]
pub struct LootAttributionManager {
    /// Mob ID to attribute drops to during the window (-1 = no window)
    next_tick_mob_id: i32,

    /// Map seed at time of trigger (prevents cross-instance carryover)
    next_tick_seed: i32,

    /// Forced variant suffix (HM/TR) during attribution window
    forced_variant: Option<VariantSuffix>,

    /// Number of loot ticks remaining in attribution window
    remaining_ticks: u32,

    /// Current tick's map seed
    current_tick_seed: i32,

    /// Fabricated entities created this tick for attribution
    fabricated_this_tick: Vec<LootEntity>,
}

impl LootAttributionManager {
    /// Create a new attribution manager.
    pub fn new() -> Self {
        Self {
            next_tick_mob_id: -1,
            next_tick_seed: -1,
            forced_variant: None,
            remaining_ticks: 0,
            current_tick_seed: -1,
            fabricated_this_tick: Vec::new(),
        }
    }

    /// Clear all state (on map change).
    pub fn clear(&mut self) {
        self.next_tick_mob_id = -1;
        self.next_tick_seed = -1;
        self.forced_variant = None;
        self.remaining_ticks = 0;
        self.current_tick_seed = -1;
        self.fabricated_this_tick.clear();
    }

    /// Handle a text packet to check for attribution triggers.
    /// Returns true if a trigger was matched.
    ///
    pub fn handle_text_packet(&mut self, name: &str, text: &str, map_seed: i32) -> bool {
        if let Some(trigger) = Self::check_text_trigger(name, text) {
            self.open_window(trigger.mob_id, map_seed, trigger.ticks, trigger.variant);
            true
        } else {
            false
        }
    }

    /// Check if a text packet matches any attribution trigger.
    pub fn check_text_trigger(name: &str, text: &str) -> Option<AttributionTrigger> {
        // Kitsune Umi (Moonlight Village)
        if name == "#Kitsune Umi" && text == "This fully concludes the Moonlight Festival!" {
            return Some(AttributionTrigger {
                mob_id: boss_ids::UMI_KITSUNE,
                ticks: 2,
                variant: None,
            });
        }

        // Dancer Miko (Moonlight Village)
        if name == "#Dancer Miko" && text == "Thank you all for coming tonight." {
            return Some(AttributionTrigger {
                mob_id: boss_ids::MIKO_DANCER,
                ticks: 2,
                variant: None,
            });
        }

        // Umi, Goddess of Revelry (Moonlight Village - True Umi variant)
        if name == "#Umi, Goddess of Revelry"
            && text == "This fully concludes the Moonlight Festival."
        {
            return Some(AttributionTrigger {
                mob_id: boss_ids::UMI_KITSUNE,
                ticks: 2,
                variant: Some(VariantSuffix::TrueRevelation),
            });
        }

        // Void Entity (The Void)
        if name == "#Void Entity" 
            && text == "You fools... You can never truly defeat me! I am in all of you! I AM all of you!" 
        {
            return Some(AttributionTrigger {
                mob_id: boss_ids::VOID_ENTITY,
                ticks: 2,
                variant: None,
            });
        }

        // Bridge Sentinel (The Shatters)
        if name == "#The Bridge Sentinel" && text == "I tried to protect you... I have failed." {
            return Some(AttributionTrigger {
                mob_id: boss_ids::BRIDGE_SENTINEL,
                ticks: 2,
                variant: None,
            });
        }

        // Bridge Sentinel HM (Valen the Unbreakable)
        if name == "#Valen the Unbreakable"
            && text == "I see now... my strength could not have held against this growing power."
        {
            return Some(AttributionTrigger {
                mob_id: boss_ids::BRIDGE_SENTINEL,
                ticks: 2,
                variant: Some(VariantSuffix::HardMode),
            });
        }

        // Twilight Archmage (The Shatters)
        if name == "#Twilight Archmage"
            && text == "Wait, there's still time! I JUST NEED MORE POWER! WAIT!"
        {
            return Some(AttributionTrigger {
                mob_id: boss_ids::TWILIGHT_ARCHMAGE,
                ticks: 2,
                variant: None,
            });
        }

        // Twilight Archmage HM (Nox the Wild Shadow)
        if name == "#Nox the Wild Shadow" 
            && text == "Unworthy as you are to know what hides beyond, I've had... an epiphany. So in case you've failed to realize..." 
        {
            return Some(AttributionTrigger {
                mob_id: boss_ids::TWILIGHT_ARCHMAGE,
                ticks: 2,
                variant: Some(VariantSuffix::HardMode),
            });
        }

        // King Azamoth (The Shatters)
        if name == "#The Accursed King"
            && text == "...do you truly think your end will be any different?"
        {
            return Some(AttributionTrigger {
                mob_id: boss_ids::ACCURSED_KING,
                ticks: 2,
                variant: None,
            });
        }

        // King Azamoth HM
        if name == "#King Azamoth" && text == "This fate is mine to bear... not hers." {
            return Some(AttributionTrigger {
                mob_id: boss_ids::ACCURSED_KING,
                ticks: 2,
                variant: Some(VariantSuffix::HardMode),
            });
        }

        None
    }

    /// Open an attribution window.
    fn open_window(&mut self, mob_id: i32, seed: i32, ticks: u32, variant: Option<VariantSuffix>) {
        self.next_tick_mob_id = mob_id;
        self.next_tick_seed = seed;
        self.forced_variant = variant;
        self.remaining_ticks = ticks;
    }

    /// Begin a loot tick (reset per-tick structures).
    pub fn begin_loot_tick(&mut self, current_seed: i32) {
        self.current_tick_seed = current_seed;
        self.fabricated_this_tick.clear();
    }

    /// Find attribution for a single bag.
    /// Returns the mob entity that dropped this bag (or None if unknown).
    ///
    pub fn find_attribution_for_bag(
        &mut self,
        bag_pos: &WorldPosData,
        killed_entities: &[super::detection::KilledEntity],
        map_seed: i32,
        time_ms: u64,
    ) -> Option<LootEntity> {
        // Prefer killed entity closest to the bag
        let mut best_dist = f32::MAX;
        let mut best_mob: Option<LootEntity> = None;

        for killed in killed_entities {
            let dist = killed.entity.dist_sqrd(bag_pos);
            if dist < best_dist {
                best_dist = dist;
                best_mob = Some(killed.entity.clone());
            }
        }

        // Fallback to attribution window if no killed entity found
        if best_mob.is_none()
            && self.remaining_ticks > 0
            && map_seed == self.next_tick_seed
            && self.next_tick_mob_id > 0
        {
            // Create fabricated entity for attribution
            let fabricated = LootEntity::fabricated(self.next_tick_mob_id, time_ms);
            self.fabricated_this_tick.push(fabricated.clone());
            best_mob = Some(fabricated);
        }

        best_mob
    }

    /// Apply HM/TR overrides after all fabricated attributions are known this tick.
    pub fn apply_per_tick_overrides(&mut self) {
        if self.fabricated_this_tick.is_empty() {
            return;
        }

        // If forced variant suffix, apply to all fabricated
        if let Some(variant) = &self.forced_variant {
            let suffix = variant.as_str();
            for fab in &mut self.fabricated_this_tick {
                fab.loot_mob_id_override = Some(format!("{}{}", fab.object_type, suffix));
            }
        } else if self.fabricated_this_tick.len() > 1 {
            // Multiple fabricated without forced variant = HM for Umi/Miko
            for fab in &mut self.fabricated_this_tick {
                if fab.object_type == boss_ids::UMI_KITSUNE {
                    fab.loot_mob_id_override = Some("20493HM".to_string());
                } else if fab.object_type == boss_ids::MIKO_DANCER {
                    fab.loot_mob_id_override = Some("20451HM".to_string());
                }
            }
        }
    }

    /// Get fabricated attributions from this tick.
    pub fn fabricated_this_tick(&self) -> &[LootEntity] {
        &self.fabricated_this_tick
    }

    /// End-of-tick housekeeping (decrement and possibly reset attribution window).
    pub fn end_loot_tick(&mut self) {
        if self.remaining_ticks > 0 {
            self.remaining_ticks -= 1;
        }

        if self.remaining_ticks == 0 {
            self.next_tick_mob_id = -1;
            self.forced_variant = None;
            self.next_tick_seed = -1;
        }
    }

    /// Check if attribution window is open.
    pub fn has_attribution_window(&self) -> bool {
        self.remaining_ticks > 0 && self.next_tick_mob_id > 0
    }

    /// Get the current attribution mob ID (if window is open).
    pub fn attribution_mob_id(&self) -> Option<i32> {
        if self.has_attribution_window() {
            Some(self.next_tick_mob_id)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_manager() {
        let mgr = LootAttributionManager::new();
        assert!(!mgr.has_attribution_window());
        assert_eq!(mgr.attribution_mob_id(), None);
    }

    #[test]
    fn test_kitsune_umi_trigger() {
        let trigger = LootAttributionManager::check_text_trigger(
            "#Kitsune Umi",
            "This fully concludes the Moonlight Festival!",
        );
        assert!(trigger.is_some());
        let t = trigger.unwrap();
        assert_eq!(t.mob_id, boss_ids::UMI_KITSUNE);
        assert_eq!(t.ticks, 2);
        assert!(t.variant.is_none());
    }

    #[test]
    fn test_umi_goddess_trigger() {
        let trigger = LootAttributionManager::check_text_trigger(
            "#Umi, Goddess of Revelry",
            "This fully concludes the Moonlight Festival.",
        );
        assert!(trigger.is_some());
        let t = trigger.unwrap();
        assert_eq!(t.mob_id, boss_ids::UMI_KITSUNE);
        assert_eq!(t.variant, Some(VariantSuffix::TrueRevelation));
    }

    #[test]
    fn test_bridge_sentinel_hm_trigger() {
        let trigger = LootAttributionManager::check_text_trigger(
            "#Valen the Unbreakable",
            "I see now... my strength could not have held against this growing power.",
        );
        assert!(trigger.is_some());
        let t = trigger.unwrap();
        assert_eq!(t.mob_id, boss_ids::BRIDGE_SENTINEL);
        assert_eq!(t.variant, Some(VariantSuffix::HardMode));
    }

    #[test]
    fn test_void_entity_trigger() {
        let trigger = LootAttributionManager::check_text_trigger(
            "#Void Entity",
            "You fools... You can never truly defeat me! I am in all of you! I AM all of you!",
        );
        assert!(trigger.is_some());
        let t = trigger.unwrap();
        assert_eq!(t.mob_id, boss_ids::VOID_ENTITY);
    }

    #[test]
    fn test_no_trigger() {
        let trigger = LootAttributionManager::check_text_trigger("SomePlayer", "Hello world!");
        assert!(trigger.is_none());
    }

    #[test]
    fn test_handle_text_packet() {
        let mut mgr = LootAttributionManager::new();

        let triggered = mgr.handle_text_packet(
            "#Kitsune Umi",
            "This fully concludes the Moonlight Festival!",
            12345,
        );

        assert!(triggered);
        assert!(mgr.has_attribution_window());
        assert_eq!(mgr.attribution_mob_id(), Some(boss_ids::UMI_KITSUNE));
    }

    #[test]
    fn test_attribution_window_lifecycle() {
        let mut mgr = LootAttributionManager::new();

        // Open window
        mgr.handle_text_packet(
            "#Kitsune Umi",
            "This fully concludes the Moonlight Festival!",
            12345,
        );
        assert_eq!(mgr.remaining_ticks, 2);

        // First tick
        mgr.begin_loot_tick(12345);
        mgr.end_loot_tick();
        assert_eq!(mgr.remaining_ticks, 1);
        assert!(mgr.has_attribution_window());

        // Second tick
        mgr.begin_loot_tick(12345);
        mgr.end_loot_tick();
        assert_eq!(mgr.remaining_ticks, 0);
        assert!(!mgr.has_attribution_window());
    }

    #[test]
    fn test_find_attribution_killed_entity() {
        let mut mgr = LootAttributionManager::new();
        mgr.begin_loot_tick(12345);

        // Create a killed entity at (10, 10)
        let mut killed_entity = LootEntity::new(100, 9999, 1000);
        killed_entity.pos = crate::protocol::data::WorldPosData { x: 10.0, y: 10.0 };

        let killed = vec![super::super::detection::KilledEntity {
            entity: killed_entity,
            kill_time: 1000,
        }];

        // Bag at (12, 12) should be attributed to killed entity
        let bag_pos = WorldPosData { x: 12.0, y: 12.0 };
        let attribution = mgr.find_attribution_for_bag(&bag_pos, &killed, 12345, 1500);

        assert!(attribution.is_some());
        assert_eq!(attribution.unwrap().object_id, 100);
    }

    #[test]
    fn test_find_attribution_fabricated() {
        let mut mgr = LootAttributionManager::new();

        // Open window for Kitsune Umi
        mgr.handle_text_packet(
            "#Kitsune Umi",
            "This fully concludes the Moonlight Festival!",
            12345,
        );

        mgr.begin_loot_tick(12345);

        // No killed entities, should create fabricated
        let bag_pos = WorldPosData { x: 50.0, y: 50.0 };
        let attribution = mgr.find_attribution_for_bag(&bag_pos, &[], 12345, 1500);

        assert!(attribution.is_some());
        let attr = attribution.unwrap();
        assert_eq!(attr.object_type, boss_ids::UMI_KITSUNE);
        assert!(attr.fabricated_attribution);
    }

    #[test]
    fn test_apply_hm_override() {
        let mut mgr = LootAttributionManager::new();

        // Open HM window
        mgr.handle_text_packet(
            "#Valen the Unbreakable",
            "I see now... my strength could not have held against this growing power.",
            12345,
        );

        mgr.begin_loot_tick(12345);

        // Create fabricated attribution
        let bag_pos = WorldPosData { x: 50.0, y: 50.0 };
        mgr.find_attribution_for_bag(&bag_pos, &[], 12345, 1500);

        // Apply overrides
        mgr.apply_per_tick_overrides();

        // Check override was applied
        assert_eq!(mgr.fabricated_this_tick.len(), 1);
        assert_eq!(
            mgr.fabricated_this_tick[0].loot_mob_id_override,
            Some("29003HM".to_string())
        );
    }

    #[test]
    fn test_clear() {
        let mut mgr = LootAttributionManager::new();
        mgr.handle_text_packet(
            "#Kitsune Umi",
            "This fully concludes the Moonlight Festival!",
            12345,
        );
        assert!(mgr.has_attribution_window());

        mgr.clear();
        assert!(!mgr.has_attribution_window());
    }
}

//! Player state tracking for loot drops.
//!
//! Tracks the current player's state (name, fame, class, skin, loot timers)
//! and captures snapshots at the moment of loot drops for historical records.

use super::entity::TrackedPlayer;
use crate::protocol::data::StatData;
use std::collections::HashMap;

/// Exaltation tier thresholds and bonuses.
pub mod exalt_tiers {
    /// Minimum completions for any bonus
    pub const MIN_FOR_BONUS: i32 = 5;
    /// Completions needed for 5% bonus
    pub const TIER_5_PCT: i32 = 5;
    /// Completions needed for 10% bonus
    pub const TIER_10_PCT: i32 = 15;
    /// Completions needed for 15% bonus
    pub const TIER_15_PCT: i32 = 30;
    /// Completions needed for 20% bonus
    pub const TIER_20_PCT: i32 = 50;
    /// Completions needed for 25% bonus (max per class)
    pub const TIER_25_PCT: i32 = 75;
    /// Fully exalted bonus (all classes maxed)
    pub const FULLY_EXALTED_BONUS: i32 = 35;
}

/// Character class IDs and their weapon groups for exaltation.
/// Weapon groups determine which classes share exaltation bonuses:
/// - Dagger classes: Rogue(768), Assassin(800), Trickster(804)
/// - Bow classes: Archer(775), Huntress(802), Bard(796)
/// - Staff classes: Wizard(782), Necromancer(801), Mystic(803)
/// - Wand classes: Priest(784), Sorcerer(805), Summoner(817)
/// - Sword classes: Warrior(797), Knight(798), Paladin(799)
/// - Katana classes: Ninja(806), Samurai(785), Kensei(818)
pub mod class_ids {
    // Dagger classes
    pub const ROGUE: i32 = 768;
    pub const ASSASSIN: i32 = 800;
    pub const TRICKSTER: i32 = 804;

    // Bow classes
    pub const ARCHER: i32 = 775;
    pub const HUNTRESS: i32 = 802;
    pub const BARD: i32 = 796;

    // Staff classes
    pub const WIZARD: i32 = 782;
    pub const NECROMANCER: i32 = 801;
    pub const MYSTIC: i32 = 803;

    // Wand classes
    pub const PRIEST: i32 = 784;
    pub const SORCERER: i32 = 805;
    pub const SUMMONER: i32 = 817;

    // Sword classes
    pub const WARRIOR: i32 = 797;
    pub const KNIGHT: i32 = 798;
    pub const PALADIN: i32 = 799;

    // Katana classes
    pub const NINJA: i32 = 806;
    pub const SAMURAI: i32 = 785;
    pub const KENSEI: i32 = 818;

    // Sceptre class (uses Wand weapon type, Leather armor)
    pub const DRUID: i32 = 819;

    /// All 19 playable character class IDs.
    pub const ALL: &'static [i32] = &[
        ROGUE,
        ASSASSIN,
        TRICKSTER,
        ARCHER,
        HUNTRESS,
        BARD,
        WIZARD,
        NECROMANCER,
        MYSTIC,
        PRIEST,
        SORCERER,
        SUMMONER,
        WARRIOR,
        KNIGHT,
        PALADIN,
        NINJA,
        SAMURAI,
        KENSEI,
        DRUID,
    ];

    /// Whether `object_type` is one of the playable character classes.
    pub fn is_player_class(object_type: i32) -> bool {
        ALL.contains(&object_type)
    }

    /// Get weapon group classes for exaltation calculation.
    /// Each class shares exalt bonuses with classes using the same weapon type.
    /// Each class has an associated weapon group.
    pub fn weapon_classes(class_id: i32) -> Vec<i32> {
        match class_id {
            // Dagger classes: Rogue, Assassin, Trickster
            768 | 800 | 804 => vec![768, 800, 804],
            // Bow classes: Archer, Huntress, Bard
            775 | 802 | 796 => vec![775, 802, 796],
            // Staff classes: Wizard, Necromancer, Mystic
            782 | 801 | 803 => vec![782, 801, 803],
            // Wand classes: Priest, Sorcerer, Summoner, Druid
            784 | 805 | 817 | 819 => vec![784, 805, 817, 819],
            // Sword classes: Warrior, Knight, Paladin
            797 | 798 | 799 => vec![797, 798, 799],
            // Katana classes: Ninja, Samurai, Kensei
            806 | 785 | 818 => vec![806, 785, 818],
            _ => vec![class_id],
        }
    }

    /// Get armor group classes for exaltation calculation. Classes sharing an
    /// armor type share Armor Proficiency progress.
    /// - Heavy: Warrior, Knight, Paladin, Samurai, Kensei
    /// - Leather: Rogue, Archer, Ninja, Huntress, Assassin, Trickster, Druid
    /// - Robe: Wizard, Priest, Necromancer, Mystic, Sorcerer, Summoner, Bard
    pub fn armor_classes(class_id: i32) -> Vec<i32> {
        const HEAVY: &[i32] = &[797, 798, 799, 785, 818];
        const LEATHER: &[i32] = &[768, 775, 806, 802, 800, 804, 819];
        const ROBE: &[i32] = &[782, 784, 801, 803, 805, 817, 796];
        if HEAVY.contains(&class_id) {
            HEAVY.to_vec()
        } else if LEATHER.contains(&class_id) {
            LEATHER.to_vec()
        } else if ROBE.contains(&class_id) {
            ROBE.to_vec()
        } else {
            vec![class_id]
        }
    }

    /// Display label for a class's armor type (e.g. "Heavy").
    pub fn armor_label(class_id: i32) -> &'static str {
        match class_id {
            797 | 798 | 799 | 785 | 818 => "Heavy",
            768 | 775 | 806 | 802 | 800 | 804 | 819 => "Leather",
            782 | 784 | 801 | 803 | 805 | 817 | 796 => "Robe",
            _ => "Armor",
        }
    }

    /// Display label for a class's weapon type (e.g. "Swords").
    pub fn weapon_label(class_id: i32) -> &'static str {
        match class_id {
            768 | 800 | 804 => "Daggers",
            775 | 802 | 796 => "Bows",
            782 | 801 | 803 => "Staves",
            784 | 805 | 817 | 819 => "Wands",
            797 | 798 | 799 => "Swords",
            806 | 785 | 818 => "Katanas",
            _ => "Weapons",
        }
    }
}

/// Exaltation data for a character class.
#[derive(Debug, Clone, Default)]
pub struct ClassExaltData {
    /// Exaltation completions per stat [HP, MP, ATT, DEF, SPD, VIT, WIS, DEX]
    pub completions: [i32; 8],
}

impl ClassExaltData {
    /// Get the minimum completion count across all stats.
    pub fn min_completions(&self) -> i32 {
        self.completions.iter().copied().min().unwrap_or(0)
    }

    /// Check if this class is fully exalted (all stats at 75).
    pub fn is_fully_exalted(&self) -> bool {
        self.completions
            .iter()
            .all(|&c| c >= exalt_tiers::TIER_25_PCT)
    }
}

/// Player state tracker.
///
/// Manages the current player's state and exaltation data for loot tracking.
#[derive(Debug, Default)]
pub struct PlayerStateTracker {
    /// Current player state (None if not in game)
    current_player: Option<TrackedPlayer>,

    /// Exaltation data per class ID
    exalt_data: HashMap<i32, ClassExaltData>,

    /// Current map seed (for validation)
    current_map_seed: i32,

    /// Current dungeon name
    current_dungeon: String,
}

impl PlayerStateTracker {
    /// Create a new player state tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Called when CreateSuccessPacket is received.
    pub fn on_create_success(&mut self, char_id: i32, object_id: i32) {
        self.current_player = Some(TrackedPlayer::new(char_id, object_id, 0));
    }

    /// Called when entering a new map.
    pub fn on_new_map(&mut self, dungeon_name: &str, map_seed: i32) {
        self.current_dungeon = dungeon_name.to_string();
        self.current_map_seed = map_seed;
    }

    /// Called when leaving a map / disconnecting.
    pub fn on_disconnect(&mut self) {
        self.current_player = None;
        self.current_dungeon.clear();
        self.current_map_seed = -1;
    }

    /// Update player stats from an Update packet.
    pub fn update_player_stats(&mut self, object_id: i32, stats: &[StatData], time_ms: u64) {
        if let Some(ref mut player) = self.current_player {
            if player.object_id == object_id {
                player.update_from_stats(stats, time_ms);
                // Object type (class ID) comes from NewTick/Update object data,
                // not from stats - use set_player_class() for that
            }
        }
    }

    /// Set the player's object type (class ID).
    pub fn set_player_class(&mut self, object_type: i32) {
        if let Some(ref mut player) = self.current_player {
            player.object_type = object_type;
        }
    }

    /// Get current player state.
    pub fn current_player(&self) -> Option<&TrackedPlayer> {
        self.current_player.as_ref()
    }

    /// Get current player state (mutable).
    pub fn current_player_mut(&mut self) -> Option<&mut TrackedPlayer> {
        self.current_player.as_mut()
    }

    /// Get current dungeon name.
    pub fn current_dungeon(&self) -> &str {
        &self.current_dungeon
    }

    /// Get current map seed.
    pub fn current_map_seed(&self) -> i32 {
        self.current_map_seed
    }

    /// Set exaltation data for a class.
    pub fn set_exalt_data(&mut self, class_id: i32, data: ClassExaltData) {
        self.exalt_data.insert(class_id, data);
    }

    /// Load exaltation data from account stats (from char list XML).
    pub fn load_exalt_data(&mut self, exalts: HashMap<i32, ClassExaltData>) {
        self.exalt_data = exalts;
    }

    /// Calculate exaltation loot bonus for a class.
    pub fn exalt_loot_bonus(&self, class_id: i32) -> i32 {
        // Check if fully exalted (all 18 classes maxed)
        if self.is_fully_exalted() {
            return exalt_tiers::FULLY_EXALTED_BONUS;
        }

        let mut bonus = 25;
        let weapon_classes = class_ids::weapon_classes(class_id);

        for &wc in &weapon_classes {
            if let Some(exalt) = self.exalt_data.get(&wc) {
                for &completions in &exalt.completions {
                    if completions < exalt_tiers::MIN_FOR_BONUS {
                        return 0;
                    } else if completions < exalt_tiers::TIER_10_PCT && bonus > 5 {
                        bonus = 5;
                    } else if completions < exalt_tiers::TIER_15_PCT && bonus > 10 {
                        bonus = 10;
                    } else if completions < exalt_tiers::TIER_20_PCT && bonus > 15 {
                        bonus = 15;
                    } else if completions < exalt_tiers::TIER_25_PCT && bonus > 20 {
                        bonus = 20;
                    }
                }
            } else {
                // No exalt data for this weapon class = no bonus
                return 0;
            }
        }

        bonus
    }

    /// Check if account is fully exalted (every canonical class at 75
    /// completions on all stats).
    pub fn is_fully_exalted(&self) -> bool {
        class_ids::ALL.iter().all(|id| {
            self.exalt_data
                .get(id)
                .is_some_and(ClassExaltData::is_fully_exalted)
        })
    }

    /// Capture a snapshot of the current player state for a loot drop.
    /// Returns None if no player is tracked.
    pub fn capture_snapshot(&self) -> Option<PlayerSnapshot> {
        let player = self.current_player.as_ref()?;

        Some(PlayerSnapshot {
            char_id: player.char_id,
            object_id: player.object_id,
            class_id: player.object_type,
            name: player.name.clone(),
            skin_id: player.skin_id,
            tex1: player.tex1,
            tex2: player.tex2,
            fame: player.fame,
            is_seasonal: player.is_seasonal,
            exalt_bonus: self.exalt_loot_bonus(player.object_type),
            loot_drop_active: player.loot_drop_time > 0,
            loot_tier_active: player.loot_tier_time > 0,
            dungeon: self.current_dungeon.clone(),
            map_seed: self.current_map_seed,
        })
    }
}

/// A snapshot of player state at the moment of a loot drop.
#[derive(Debug, Clone)]
pub struct PlayerSnapshot {
    /// Character ID
    pub char_id: i32,
    /// Object ID in current map
    pub object_id: i32,
    /// Class ID (object type)
    pub class_id: i32,
    /// Player name
    pub name: String,
    /// Skin ID (0 for default)
    pub skin_id: i32,
    /// Clothing dye/cloth texture (tex1)
    pub tex1: u32,
    /// Accessory dye/cloth texture (tex2)
    pub tex2: u32,
    /// Fame at moment of drop
    pub fame: i32,
    /// Whether this is a seasonal character
    pub is_seasonal: bool,
    /// Exaltation loot bonus percentage
    pub exalt_bonus: i32,
    /// Whether loot drop timer was active
    pub loot_drop_active: bool,
    /// Whether loot tier timer was active
    pub loot_tier_active: bool,
    /// Dungeon name where drop occurred
    pub dungeon: String,
    /// Map seed (for validation)
    pub map_seed: i32,
}

impl PlayerSnapshot {
    /// Get the display icon ID (skin if set, otherwise class).
    pub fn icon_id(&self) -> i32 {
        if self.skin_id > 0 {
            self.skin_id
        } else {
            self.class_id
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_tracker() {
        let tracker = PlayerStateTracker::new();
        assert!(tracker.current_player().is_none());
        assert!(tracker.current_dungeon().is_empty());
    }

    #[test]
    fn test_on_create_success() {
        let mut tracker = PlayerStateTracker::new();
        tracker.on_create_success(1234, 5678);

        let player = tracker.current_player().unwrap();
        assert_eq!(player.char_id, 1234);
        assert_eq!(player.object_id, 5678);
    }

    #[test]
    fn test_on_new_map() {
        let mut tracker = PlayerStateTracker::new();
        tracker.on_new_map("The Void", 12345);

        assert_eq!(tracker.current_dungeon(), "The Void");
        assert_eq!(tracker.current_map_seed(), 12345);
    }

    #[test]
    fn test_exalt_bonus_no_data() {
        let tracker = PlayerStateTracker::new();
        // No exalt data = 0 bonus
        assert_eq!(tracker.exalt_loot_bonus(768), 0);
    }

    #[test]
    fn test_exalt_bonus_partial() {
        let mut tracker = PlayerStateTracker::new();

        // Add partial exalt data for sword classes (Warrior, Knight, Paladin)
        let mut exalt = ClassExaltData::default();
        exalt.completions = [20, 20, 20, 20, 20, 20, 20, 20]; // All at 20 = 10% bonus tier

        // Set for all sword classes: Warrior(797), Knight(798), Paladin(799)
        for &class_id in &[797, 798, 799] {
            tracker.set_exalt_data(class_id, exalt.clone());
        }

        assert_eq!(tracker.exalt_loot_bonus(797), 10);
    }

    #[test]
    fn test_capture_snapshot() {
        let mut tracker = PlayerStateTracker::new();
        tracker.on_create_success(1234, 5678);
        tracker.on_new_map("Oryx's Castle", 99999);
        tracker.set_player_class(768); // Rogue

        if let Some(player) = tracker.current_player_mut() {
            player.name = "TestPlayer".to_string();
            player.fame = 50000;
            player.skin_id = 9001;
        }

        let snapshot = tracker.capture_snapshot().unwrap();
        assert_eq!(snapshot.char_id, 1234);
        assert_eq!(snapshot.name, "TestPlayer");
        assert_eq!(snapshot.fame, 50000);
        assert_eq!(snapshot.dungeon, "Oryx's Castle");
        assert_eq!(snapshot.icon_id(), 9001); // Has skin
    }

    #[test]
    fn test_class_exalt_data() {
        let mut exalt = ClassExaltData::default();
        assert_eq!(exalt.min_completions(), 0);
        assert!(!exalt.is_fully_exalted());

        exalt.completions = [75, 75, 75, 75, 75, 75, 75, 75];
        assert_eq!(exalt.min_completions(), 75);
        assert!(exalt.is_fully_exalted());
    }

    #[test]
    fn test_on_disconnect() {
        let mut tracker = PlayerStateTracker::new();
        tracker.on_create_success(1234, 5678);
        tracker.on_new_map("Test", 123);

        assert!(tracker.current_player().is_some());

        tracker.on_disconnect();

        assert!(tracker.current_player().is_none());
        assert!(tracker.current_dungeon().is_empty());
    }
}

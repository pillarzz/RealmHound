//! Dungeon data: difficulty ratings and item-to-dungeon mappings.
//!
//! - Difficulty: hardcoded from RealmEye wiki data
//! - Item mappings: `ORG_XXX` labels on items → which dungeon each item originates from

use std::collections::HashMap;

use crate::stats::dungeon_registry::get_dungeon_registry;

use super::object_list::ObjectList;

/// Case-insensitive difficulty lookup from the hardcoded table.
pub fn dungeon_difficulty(name: &str) -> Option<f32> {
    let lower = name.to_lowercase();
    DUNGEON_DIFFICULTIES
        .iter()
        .find(|(n, _)| n.to_lowercase() == lower)
        .map(|(_, d)| *d)
}

/// Dungeons that never count toward mission difficulty objectives (not
/// obtainable in-game / excluded by Deca), so they are hidden from
/// difficulty-range tooltips.
const MISSION_EXCLUDED_DUNGEONS: &[&str] = &["Chess", "Oryxmania"];

/// Dungeons whose grave difficulty falls within `[lo, hi]` (inclusive), sorted
/// by descending difficulty then name (harder dungeons first, A-Z within each
/// difficulty). Used by mission tooltips to list which dungeons satisfy an
/// "X-Y grave difficulty" objective.
pub fn dungeons_in_difficulty_range(lo: f32, hi: f32) -> Vec<(&'static str, f32)> {
    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    let mut out: Vec<(&'static str, f32)> = DUNGEON_DIFFICULTIES
        .iter()
        .filter(|(n, d)| *d >= lo && *d <= hi && !MISSION_EXCLUDED_DUNGEONS.contains(n))
        .map(|(n, d)| (*n, *d))
        .collect();
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then(a.0.cmp(b.0)));
    out
}

/// Difficulty tier for the public key-pop notification filter. Buckets a
/// dungeon's RealmEye grave-difficulty rating into the four player-facing
/// categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPopTier {
    /// Grave difficulty <= 2.0.
    Rookie,
    /// Grave difficulty > 2.0 and <= 4.5.
    Adept,
    /// Grave difficulty > 4.5 and <= 6.5.
    Expert,
    /// Grave difficulty > 6.5.
    Exaltation,
}

/// Bucket a raw grave-difficulty rating into a [`KeyPopTier`].
pub fn difficulty_tier(difficulty: f32) -> KeyPopTier {
    if difficulty <= 2.0 {
        KeyPopTier::Rookie
    } else if difficulty <= 4.5 {
        KeyPopTier::Adept
    } else if difficulty <= 6.5 {
        KeyPopTier::Expert
    } else {
        KeyPopTier::Exaltation
    }
}

/// Classify a portal/dungeon name into a [`KeyPopTier`]. Accepts raw portal
/// object names (which end in " Portal") as well as bare dungeon names, and
/// resolves aliases via the dungeon registry. Returns `None` when the dungeon
/// has no known difficulty rating.
pub fn key_pop_tier(name: &str) -> Option<KeyPopTier> {
    let base = name.strip_suffix(" Portal").unwrap_or(name);
    dungeon_difficulty(base)
        .or_else(|| dungeon_difficulty(&get_dungeon_registry().normalize(base)))
        .map(difficulty_tier)
}

/// Dungeon data built from game assets.
#[derive(Debug)]
pub struct DungeonData {
    /// Dungeon name → item IDs that originate from it
    dungeon_to_items: HashMap<String, Vec<i32>>,
    /// Item ID → dungeon names it originates from
    item_to_dungeons: HashMap<i32, Vec<&'static str>>,
}

impl DungeonData {
    /// Build dungeon data from loaded object list.
    pub fn build(objects: Option<&ObjectList>) -> Self {
        let (dungeon_to_items, item_to_dungeons) = Self::build_item_mappings(objects);

        Self {
            dungeon_to_items,
            item_to_dungeons,
        }
    }

    /// Get item IDs that originate from a dungeon.
    pub fn items_for_dungeon(&self, name: &str) -> Vec<i32> {
        let canonical = get_dungeon_registry().normalize(name);
        self.dungeon_to_items
            .get(&canonical)
            .cloned()
            .unwrap_or_default()
    }

    /// Get dungeon names that an item originates from.
    pub fn dungeons_for_item(&self, item_id: i32) -> Vec<&'static str> {
        self.item_to_dungeons
            .get(&item_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Build item↔dungeon mappings from ORG_ labels on loaded objects.
    fn build_item_mappings(
        objects: Option<&ObjectList>,
    ) -> (HashMap<String, Vec<i32>>, HashMap<i32, Vec<&'static str>>) {
        let mut dungeon_to_items: HashMap<String, Vec<i32>> = HashMap::new();
        let mut item_to_dungeons: HashMap<i32, Vec<&'static str>> = HashMap::new();

        let objects = match objects {
            Some(o) => o,
            None => return (dungeon_to_items, item_to_dungeons),
        };

        let registry = get_dungeon_registry();

        for (id, asset) in objects.iter_with_ids() {
            if asset.labels.is_empty() {
                continue;
            }
            let dungeons = registry.dungeons_from_labels(&asset.labels);
            if dungeons.is_empty() {
                continue;
            }

            for dungeon in &dungeons {
                dungeon_to_items
                    .entry(dungeon.to_string())
                    .or_default()
                    .push(*id);
            }
            item_to_dungeons.insert(*id, dungeons);
        }

        (dungeon_to_items, item_to_dungeons)
    }
}

/// Hardcoded dungeon difficulty ratings from RealmEye wiki.
/// Source: https://www.realmeye.com/wiki/dungeons
static DUNGEON_DIFFICULTIES: &[(&str, f32)] = &[
    ("Abyss of Demons", 4.0),
    ("Advanced Kogbold Steamworks", 9.0),
    ("Ancient Ruins", 3.0),
    ("Battle for the Nexus", 5.0),
    ("Beachzone", 0.0),
    ("Belladonna's Garden", 5.5),
    ("Candyland Hunting Grounds", 3.5),
    ("Cave of a Thousand Treasures", 2.5),
    ("Chess", 9.0),
    ("Cnidarian Reef", 5.5),
    ("Court of Oryx", 4.5),
    ("Crystal Cavern", 7.5),
    ("Cultist Hideout", 7.0),
    ("Cursed Library", 4.0),
    ("Davy Jones' Locker", 5.0),
    ("Deadwater Docks", 5.5),
    ("Forax", 5.0),
    ("Forbidden Jungle", 1.5),
    ("Forest Maze", 1.0),
    ("Fungal Cavern", 7.5),
    ("Haunted Cemetery", 4.5),
    ("Heroic Undead Lair", 6.0),
    ("Hidden Interregnum", 7.0),
    ("High Tech Terror", 6.5),
    ("Ice Citadel", 7.0),
    ("Ice Tomb", 6.0),
    ("Infernal Abyss of Demons", 6.0),
    ("Katalund", 5.0),
    ("Kogbold Steamworks", 8.0),
    ("Lair of Draconis", 6.0),
    ("Lair of Shaitan", 6.0),
    ("Legacy Heroic Abyss of Demons", 5.0),
    ("Legacy Heroic Undead Lair", 5.0),
    ("Lost Halls", 8.0),
    ("Mad God Mayhem", 6.0),
    ("Mad Lab", 4.0),
    ("Magic Woods", 3.0),
    ("Malogia", 5.0),
    ("Manor of the Immortals", 4.0),
    ("Moonlight Village", 9.0),
    ("Mountain Temple", 6.0),
    ("Neo Forax", 8.0),
    ("Neo Katalund", 8.0),
    ("Neo Malogia", 8.0),
    ("Neo Untaris", 8.0),
    ("Ocean Trench", 5.0),
    ("Oryx's Castle", 4.5),
    ("Oryx's Chamber", 4.5),
    ("Oryx's Sanctuary", 9.5),
    ("Oryxmania", 8.0),
    ("Parasite Chambers", 5.5),
    ("Pirate Cave", 1.0),
    ("Plagued Nest", 8.5),
    ("Puppet Master's Encore", 5.5),
    ("Puppet Master's Theatre", 4.0),
    ("Queen Bunny Chamber", 5.5),
    ("Rainbow Road", 0.0),
    ("Santa's Workshop", 0.0),
    ("Secluded Thicket", 6.5),
    ("Snake Pit", 2.5),
    ("Spectral Penitentiary", 8.0),
    ("Spider Den", 1.5),
    ("Sprite World", 2.5),
    ("Stromwell's Rift I", 5.0),
    ("Stromwell's Rift II", 7.5),
    ("Stromwell's Rift III", 10.0),
    ("Sulfurous Wetlands", 6.0),
    ("The Crawling Depths", 5.5),
    ("The Hive", 2.0),
    ("The Inner Workings", 0.0),
    ("The Machine", 4.5),
    ("The Nest", 7.0),
    ("The Shatters", 10.0),
    ("The Tavern", 5.5),
    ("The Third Dimension", 6.0),
    ("The Trials of Cronus", 6.5),
    ("The Void", 8.5),
    ("Remnant of the Void", 8.5),
    ("Tomb of the Ancients", 6.0),
    ("Toxic Sewers", 4.0),
    ("Undead Lair", 3.5),
    ("Untaris", 5.0),
    ("White Snake Invasion I", 6.5),
    ("White Snake Invasion II", 7.5),
    ("White Snake Invasion III", 8.5),
    ("Wine Cellar", 6.0),
    ("Woodland Labyrinth", 5.5),
];

#[cfg(test)]
mod key_pop_tier_tests {
    use super::*;

    #[test]
    fn difficulty_buckets_match_thresholds() {
        // Boundaries: Rookie <= 2, Adept (2, 4.5], Expert (4.5, 6.5], Exaltation > 6.5.
        assert_eq!(difficulty_tier(0.0), KeyPopTier::Rookie);
        assert_eq!(difficulty_tier(2.0), KeyPopTier::Rookie);
        assert_eq!(difficulty_tier(2.5), KeyPopTier::Adept);
        assert_eq!(difficulty_tier(4.5), KeyPopTier::Adept);
        assert_eq!(difficulty_tier(4.6), KeyPopTier::Expert);
        assert_eq!(difficulty_tier(6.5), KeyPopTier::Expert);
        assert_eq!(difficulty_tier(6.6), KeyPopTier::Exaltation);
        assert_eq!(difficulty_tier(10.0), KeyPopTier::Exaltation);
    }

    #[test]
    fn classifies_known_dungeons() {
        assert_eq!(key_pop_tier("The Hive"), Some(KeyPopTier::Rookie)); // 2.0
        assert_eq!(key_pop_tier("Snake Pit"), Some(KeyPopTier::Adept)); // 2.5
        assert_eq!(key_pop_tier("Abyss of Demons"), Some(KeyPopTier::Adept)); // 4.0
        assert_eq!(key_pop_tier("Lair of Draconis"), Some(KeyPopTier::Expert)); // 6.0
        assert_eq!(key_pop_tier("Secluded Thicket"), Some(KeyPopTier::Expert)); // 6.5
        assert_eq!(key_pop_tier("The Nest"), Some(KeyPopTier::Exaltation)); // 7.0
    }

    #[test]
    fn strips_portal_suffix() {
        assert_eq!(key_pop_tier("Snake Pit Portal"), Some(KeyPopTier::Adept));
        assert_eq!(
            key_pop_tier("Lost Halls Portal"),
            Some(KeyPopTier::Exaltation)
        ); // 8.0
    }

    #[test]
    fn unknown_dungeon_is_none() {
        assert!(key_pop_tier("Totally Not A Dungeon").is_none());
    }
}

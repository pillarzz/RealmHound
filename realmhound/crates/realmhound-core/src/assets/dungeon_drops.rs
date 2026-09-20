//! Dungeon-portal drop categorizer: which enemies drop each dungeon's portal,
//! grouped by biome and classified by category.
//!
//! Data is scraped from the RealmEye wiki biome pages (the "Enemies" section,
//! split into Regular Enemies / Heroes of Oryx / Encounters / Beacon Guardian)
//! and stored in `assets/data/dungeon_drops.json`. Each enemy row also records
//! which dungeon portals it drops, so the loader builds an inverse index keyed
//! by portal (dungeon) name.
//!
//! This module intentionally only exposes the raw, indexed data. The tooltip
//! display ordering (tier/category priority, biome sectioning) is a
//! presentation concern handled by the UI, not baked in here.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

static DUNGEON_DROP_DATA: OnceLock<DungeonDropData> = OnceLock::new();

pub fn get_dungeon_drops() -> &'static DungeonDropData {
    DUNGEON_DROP_DATA.get_or_init(|| {
        let json = include_str!("../../../../assets/data/dungeon_drops.json");
        DungeonDropData::from_json(json)
    })
}

/// Realm difficulty tier of a biome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BiomeTier {
    Rookie,
    Adept,
    Veteran,
    Seasonal,
}

impl BiomeTier {
    fn parse(s: &str) -> Self {
        match s {
            "adept" => BiomeTier::Adept,
            "veteran" => BiomeTier::Veteran,
            "seasonal" => BiomeTier::Seasonal,
            _ => BiomeTier::Rookie,
        }
    }
}

/// Which enemy category a drop source belongs to on its biome page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropCategory {
    Regular,
    Hero,
    Encounter,
    Beacon,
}

/// One enemy that drops a given dungeon's portal.
#[derive(Debug, Clone)]
pub struct DungeonDropSource {
    pub enemy_name: String,
    pub biome: String,
    pub tier: BiomeTier,
    pub category: DropCategory,
}

/// One biome's drop list. Each biome appears exactly once; an enemy that spans
/// several biomes is repeated under each of them. Enemies within a biome are
/// ordered Encounters -> Heroes of Oryx -> regulars.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BiomeSection {
    pub biome: String,
    /// Realm tier of this biome, so the UI can sink seasonal biomes (which are
    /// only on the map a couple months a year) to the bottom of a tooltip.
    pub tier: BiomeTier,
    pub enemies: Vec<String>,
    /// Drop category for each entry in `enemies` (same order/length), so the UI
    /// can order sources by category (regular -> hero -> encounter) without
    /// re-deriving it.
    pub categories: Vec<DropCategory>,
}

#[derive(Debug)]
pub struct DungeonDropData {
    /// Portal (dungeon) name -> enemies that drop it.
    portal_to_sources: HashMap<String, Vec<DungeonDropSource>>,
    /// Biome name -> its Beacon Guardian enemy name (used as the biome's icon).
    biome_beacon: HashMap<String, String>,
    /// Every biome's encounter bosses, tagged with the biome's tier. Used to
    /// build the tier-wide encounter tooltips (independent of any one portal).
    biome_encounters: Vec<(String, BiomeTier, Vec<String>)>,
}

/// Priority of a biome tier in tooltip ordering (Veteran first).
fn tier_rank(t: BiomeTier) -> u8 {
    match t {
        BiomeTier::Veteran => 0,
        BiomeTier::Adept => 1,
        BiomeTier::Seasonal => 2,
        BiomeTier::Rookie => 3,
    }
}

/// Priority of a drop category within a biome group (Encounters first).
fn cat_rank(c: DropCategory) -> u8 {
    match c {
        DropCategory::Encounter => 0,
        DropCategory::Hero => 1,
        DropCategory::Regular => 2,
        DropCategory::Beacon => 3,
    }
}

/// Build per-biome sections from `(biome, tier, category, enemy_name)` rows.
/// Each biome becomes one section (an enemy that appears in several biomes is
/// repeated under each). Biomes are ordered by tier then name; enemies within a
/// biome by category then name, de-duplicated (keeping the most prominent
/// category).
fn build_sections(rows: Vec<(String, BiomeTier, DropCategory, String)>) -> Vec<BiomeSection> {
    // biome -> (tier, enemy_name -> best category)
    let mut map: HashMap<String, (BiomeTier, HashMap<String, DropCategory>)> = HashMap::new();
    for (biome, tier, category, name) in rows {
        let entry = map.entry(biome).or_insert_with(|| (tier, HashMap::new()));
        entry
            .1
            .entry(name)
            .and_modify(|c| {
                if cat_rank(category) < cat_rank(*c) {
                    *c = category;
                }
            })
            .or_insert(category);
    }

    let mut sections: Vec<(String, BiomeTier, Vec<(DropCategory, String)>)> = map
        .into_iter()
        .map(|(biome, (tier, enemies))| {
            let list = enemies.into_iter().map(|(n, c)| (c, n)).collect();
            (biome, tier, list)
        })
        .collect();
    sections.sort_by(|a, b| {
        tier_rank(a.1)
            .cmp(&tier_rank(b.1))
            .then_with(|| a.0.cmp(&b.0))
    });

    sections
        .into_iter()
        .map(|(biome, tier, mut enemies)| {
            enemies.sort_by(|a, b| {
                cat_rank(a.0)
                    .cmp(&cat_rank(b.0))
                    .then_with(|| a.1.cmp(&b.1))
            });
            BiomeSection {
                biome,
                tier,
                categories: enemies.iter().map(|(c, _)| *c).collect(),
                enemies: enemies.into_iter().map(|(_, n)| n).collect(),
            }
        })
        .collect()
}

#[derive(Deserialize)]
struct RawRoot {
    #[serde(default)]
    biomes: Vec<RawBiome>,
}

#[derive(Deserialize)]
struct RawBiome {
    biome: String,
    tier: String,
    #[serde(default)]
    regular: Vec<RawEnemy>,
    #[serde(default)]
    hero: Vec<RawEnemy>,
    #[serde(default)]
    encounter: Vec<RawEnemy>,
    #[serde(default)]
    beacon: Vec<RawEnemy>,
}

/// `[enemy_name, [portal_name, ...]]`.
#[derive(Deserialize)]
struct RawEnemy(String, Vec<String>);

impl DungeonDropData {
    fn normalize_name(name: &str) -> String {
        name.replace('\u{2019}', "'")
            .replace('\u{00A0}', " ")
            .trim()
            .to_string()
    }

    fn from_json(json: &str) -> Self {
        let raw: RawRoot = serde_json::from_str(json).unwrap_or_else(|e| {
            eprintln!("Failed to parse dungeon_drops.json: {e}");
            RawRoot { biomes: Vec::new() }
        });

        let mut portal_to_sources: HashMap<String, Vec<DungeonDropSource>> = HashMap::new();
        let mut biome_beacon: HashMap<String, String> = HashMap::new();
        let mut biome_encounters: Vec<(String, BiomeTier, Vec<String>)> = Vec::new();

        for biome in &raw.biomes {
            let tier = BiomeTier::parse(&biome.tier);
            let biome_name = Self::normalize_name(&biome.biome);
            if let Some(b) = biome.beacon.first() {
                biome_beacon.insert(biome_name.clone(), Self::normalize_name(&b.0));
            }
            let encounters: Vec<String> = biome
                .encounter
                .iter()
                .map(|e| Self::normalize_name(&e.0))
                .collect();
            if !encounters.is_empty() {
                biome_encounters.push((biome_name.clone(), tier, encounters));
            }
            let sections = [
                (DropCategory::Regular, &biome.regular),
                (DropCategory::Hero, &biome.hero),
                (DropCategory::Encounter, &biome.encounter),
                (DropCategory::Beacon, &biome.beacon),
            ];
            for (category, enemies) in sections {
                for enemy in enemies {
                    let enemy_name = Self::normalize_name(&enemy.0);
                    for portal in &enemy.1 {
                        let portal_name = Self::normalize_name(portal);
                        portal_to_sources
                            .entry(portal_name)
                            .or_default()
                            .push(DungeonDropSource {
                                enemy_name: enemy_name.clone(),
                                biome: biome_name.clone(),
                                tier,
                                category,
                            });
                    }
                }
            }
        }

        Self {
            portal_to_sources,
            biome_beacon,
            biome_encounters,
        }
    }

    /// Enemies that drop the given dungeon's portal, in load order (unsorted).
    pub fn sources_for_dungeon(&self, portal_name: &str) -> &[DungeonDropSource] {
        self.portal_to_sources
            .get(&Self::normalize_name(portal_name))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// A biome's Beacon Guardian enemy name, used as the biome's section icon.
    pub fn beacon_for_biome(&self, biome: &str) -> Option<&str> {
        self.biome_beacon.get(biome).map(|s| s.as_str())
    }

    /// Build the ordered, per-biome tooltip model for a dungeon portal.
    ///
    /// Each biome that has a drop source is shown once; an enemy that drops the
    /// portal in several biomes (e.g. a shared encounter) is repeated under each
    /// biome. Biomes are ordered by tier (Veteran, then Adept, then Seasonal,
    /// then Rookie) and then alphabetically. Within a biome the order is
    /// Encounters -> Heroes of Oryx -> regular enemies. Beacon Guardians are
    /// excluded (they have their own mission tooltip).
    pub fn tooltip_model(&self, portal_name: &str) -> Vec<BiomeSection> {
        let rows: Vec<(String, BiomeTier, DropCategory, String)> = self
            .sources_for_dungeon(portal_name)
            .iter()
            .filter(|s| s.category != DropCategory::Beacon)
            .map(|s| (s.biome.clone(), s.tier, s.category, s.enemy_name.clone()))
            .collect();
        build_sections(rows)
    }

    /// The biomes (any tier) whose page lists `name` as an encounter boss, so
    /// mission encounter tooltips can be grouped by spawn location while still
    /// deriving *membership* from the authoritative in-game encounter label.
    /// Sorted and de-duplicated; empty when the encounter has no known biome.
    pub fn encounter_biomes(&self, name: &str) -> Vec<String> {
        let key = Self::normalize_name(name);
        let mut biomes: Vec<String> = self
            .biome_encounters
            .iter()
            .filter(|(_, _, encs)| encs.iter().any(|e| e == &key))
            .map(|(b, _, _)| b.clone())
            .collect();
        biomes.sort();
        biomes.dedup();
        biomes
    }

    /// The realm tier of a biome (as recorded in the scraped data), or `None`
    /// for an unknown biome name.
    pub fn biome_tier(&self, biome: &str) -> Option<BiomeTier> {
        let key = Self::normalize_name(biome);
        self.biome_encounters
            .iter()
            .find(|(b, _, _)| b == &key)
            .map(|(_, t, _)| *t)
    }

    /// Whether any enemy is known to drop the given dungeon's portal.
    pub fn has_dungeon(&self, portal_name: &str) -> bool {
        self.portal_to_sources
            .contains_key(&Self::normalize_name(portal_name))
    }

    /// Whether `name` is listed as an Encounter boss on any biome page (as
    /// opposed to a regular enemy or Hero of Oryx). Used to decide which drop
    /// bullets carry a spawn-cap ("respawnable"/"limited") label.
    pub fn is_encounter(&self, name: &str) -> bool {
        let key = Self::normalize_name(name);
        self.biome_encounters
            .iter()
            .any(|(_, _, encs)| encs.iter().any(|e| e == &key))
    }

    /// All dungeon portal names that have at least one known drop source.
    pub fn all_dungeon_names(&self) -> impl Iterator<Item = &str> {
        self.portal_to_sources.keys().map(|s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(sources: &[DungeonDropSource]) -> Vec<&str> {
        sources.iter().map(|s| s.enemy_name.as_str()).collect()
    }

    #[test]
    fn sprite_world_matches_issue_example() {
        let data = get_dungeon_drops();
        let s = data.sources_for_dungeon("Sprite World");
        // All from the Sprite Forest (Adept) biome.
        assert!(s
            .iter()
            .all(|x| x.biome == "Sprite Forest" && x.tier == BiomeTier::Adept));
        let n = names(s);
        assert!(n.contains(&"Sprite Beast"), "{n:?}");
        assert!(n.contains(&"Sprite Child"), "{n:?}");
        // Heroes of Oryx from the same biome also drop it.
        assert!(n.contains(&"Celestial Sprite"), "{n:?}");
    }

    #[test]
    fn ocean_trench_groups_across_biomes() {
        let data = get_dungeon_drops();
        let s = data.sources_for_dungeon("Ocean Trench");
        let find = |name: &str| s.iter().find(|x| x.enemy_name == name);

        let sea_dragon = find("Sea Dragon").expect("Sea Dragon");
        assert_eq!(sea_dragon.biome, "Deep Sea Abyss");
        assert_eq!(sea_dragon.tier, BiomeTier::Veteran);
        assert_eq!(sea_dragon.category, DropCategory::Hero);

        let squid = find("Abyssal Squid").expect("Abyssal Squid");
        assert_eq!(squid.biome, "Deep Sea Abyss");
        assert_eq!(squid.category, DropCategory::Regular);

        let ice_giant = find("Ice Giant").expect("Ice Giant");
        assert_eq!(ice_giant.biome, "Runic Tundra");
        assert_eq!(ice_giant.tier, BiomeTier::Veteran);
    }

    #[test]
    fn data_loads_and_is_nonempty() {
        let data = get_dungeon_drops();
        assert!(data.all_dungeon_names().count() > 20);
    }

    #[test]
    fn ocean_trench_tooltip_order_matches_issue() {
        let data = get_dungeon_drops();
        let groups = data.tooltip_model("Ocean Trench");
        // Veteran biomes first: Deep Sea Abyss (Sea Dragon hero, then Abyssal
        // Squid regular), then Runic Tundra (Ice Giant).
        assert_eq!(groups[0].biome, "Deep Sea Abyss");
        assert_eq!(groups[0].enemies, vec!["Sea Dragon", "Abyssal Squid"]);
        assert_eq!(groups[1].biome, "Runic Tundra");
        assert_eq!(groups[1].enemies, vec!["Ice Giant"]);
        // Adept encounters follow, each under its own biome.
        let group_with = |enemy: &str| {
            groups
                .iter()
                .find(|g| g.enemies.iter().any(|e| e == enemy))
                .unwrap_or_else(|| panic!("no group for {enemy}"))
        };
        let coral = group_with("Hermit God");
        assert_eq!(coral.biome, "Coral Reefs");
        let cove = group_with("Eye of the Storm");
        assert_eq!(cove.biome, "Shipwreck Cove");
        let idx = |enemy: &str| {
            groups
                .iter()
                .position(|g| g.enemies.iter().any(|e| e == enemy))
                .unwrap()
        };
        assert!(idx("Sea Dragon") < idx("Hermit God"));
        assert!(idx("Ice Giant") < idx("Eye of the Storm"));
    }

    #[test]
    fn third_dimension_shared_encounter_duplicated_per_biome() {
        let data = get_dungeon_drops();
        let groups = data.tooltip_model("The Third Dimension");
        // Cube God drops The Third Dimension in both Abandoned City and Sprite
        // Forest, so it is listed under each biome section (duplicated).
        let cube: Vec<&BiomeSection> = groups
            .iter()
            .filter(|g| g.enemies.iter().any(|e| e == "Cube God"))
            .collect();
        let biomes: Vec<&str> = cube.iter().map(|g| g.biome.as_str()).collect();
        assert!(biomes.contains(&"Abandoned City"), "{biomes:?}");
        assert!(biomes.contains(&"Sprite Forest"), "{biomes:?}");
        // Astral Rift (Sprite Forest only) shares the Sprite Forest section.
        let sprite = groups
            .iter()
            .find(|g| g.biome == "Sprite Forest")
            .expect("Sprite Forest section");
        assert!(sprite.enemies.iter().any(|e| e == "Astral Rift"));
        assert!(sprite.enemies.iter().any(|e| e == "Cube God"));
    }

    #[test]
    fn sprite_world_tooltip_is_single_biome() {
        let data = get_dungeon_drops();
        let groups = data.tooltip_model("Sprite World");
        assert_eq!(groups.len(), 1);
        let g = &groups[0];
        assert_eq!(g.biome, "Sprite Forest");
        assert!(g.enemies.contains(&"Celestial Sprite".to_string()));
        assert!(g.enemies.contains(&"Sprite Beast".to_string()));
        assert!(g.enemies.contains(&"Sprite Child".to_string()));
    }

    #[test]
    fn gardener_drops_belladonnas_garden_from_both_biomes() {
        let data = get_dungeon_drops();
        let groups = data.tooltip_model("Belladonna's Garden");
        let biomes: Vec<&str> = groups
            .iter()
            .filter(|g| g.enemies.iter().any(|e| e == "The Gardener"))
            .map(|g| g.biome.as_str())
            .collect();
        assert!(biomes.contains(&"Sprite Forest"), "{biomes:?}");
        assert!(biomes.contains(&"Coral Reefs"), "{biomes:?}");
        // Listed as an Encounter so its drop bullet is spawn-cap labelled.
        assert!(data.is_encounter("The Gardener"));
    }

    #[test]
    fn encounter_biomes_lookup_and_tier() {
        let data = get_dungeon_drops();
        // Hermit God is an Adept (Coral Reefs) encounter on RealmEye.
        let biomes = data.encounter_biomes("Hermit God");
        assert!(biomes.contains(&"Coral Reefs".to_string()));
        assert_eq!(data.biome_tier("Coral Reefs"), Some(BiomeTier::Adept));
        assert_eq!(data.biome_tier("Deep Sea Abyss"), Some(BiomeTier::Veteran));
        // Unknown encounter -> no biomes.
        assert!(data.encounter_biomes("Definitely Not A Boss").is_empty());
    }

    #[test]
    fn new_encounters_attribute_to_their_biome() {
        let data = get_dungeon_drops();
        for (name, biome) in [
            ("Artificial Slop", "Haunted Hallows"),
            ("Cold Soul", "Risen Hell"),
            ("Stygian Mirror", "Risen Hell"),
            ("Towering Perfection", "Sprite Forest"),
            ("Man-eating Barnacle", "Coral Reefs"),
            ("Cube Deity", "Deep Sea Abyss"),
            // Previously un-attributed Abandoned City encounters.
            ("Mammoth Rat", "Abandoned City"),
            ("Jade and Garnet Statues", "Abandoned City"),
        ] {
            assert!(
                data.encounter_biomes(name).contains(&biome.to_string()),
                "{name} should attribute to {biome}, got {:?}",
                data.encounter_biomes(name)
            );
        }
    }

    #[test]
    fn ocean_trench_keeps_seasonal_biome_source() {
        // Ocean Trench is dropped by an Oryxmas Eternal Frost enemy; the group
        // must survive in the model (render-time sprite resolution hides the
        // iconless Hat God reskin, not the core data layer).
        let data = get_dungeon_drops();
        let groups = data.tooltip_model("Ocean Trench");
        assert!(
            groups.iter().any(|g| g.biome == "Eternal Frost"),
            "Eternal Frost group expected for Ocean Trench"
        );
    }
}

//! RealmEye drop data: precise item -> mob -> dungeon/biome mapping.
//!
//! Loaded from a compact JSON file scraped from RealmEye wiki dungeon and biome
//! "Drops of Interest" tables. Provides precise dungeon resolution (e.g.,
//! distinguishes Infernal Abyss from regular Abyss).

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

static REALMEYE_DATA: OnceLock<RealmEyeDropData> = OnceLock::new();

pub fn get_realmeye_drops() -> &'static RealmEyeDropData {
    REALMEYE_DATA.get_or_init(|| {
        let json = include_str!("../../../../assets/data/realmeye_drops.json");
        RealmEyeDropData::from_json(json)
    })
}

#[derive(Debug, Clone)]
pub struct DropSource {
    pub source_name: String,
    pub location_name: String,
    pub location_type: DropLocationType,
    /// True when the source's location is a seasonal-only event/biome (e.g. an
    /// Easter reskin). Seasonal sources are excluded when deciding whether a
    /// rare item points to a single unambiguous drop source.
    pub seasonal: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DropLocationType {
    Dungeon,
    Biome,
}

#[derive(Debug)]
pub struct RealmEyeDropData {
    item_to_dungeons: HashMap<i32, Vec<String>>,
    item_to_sources: HashMap<i32, Vec<DropSource>>,
    dungeon_to_items: HashMap<String, Vec<i32>>,
    dungeon_portals: HashMap<String, i32>,
}

#[derive(Deserialize)]
struct RawCompact {
    sources: Vec<String>,
    dungeons: Vec<(String, Option<i32>, Vec<(usize, Vec<i32>)>)>,
    biomes: Vec<(String, String, Vec<(usize, Vec<i32>)>)>,
}

/// Extra drop sources appended to items' tooltips WITHOUT adding those items to
/// the source dungeon's collection. Used for:
/// - the "Neo" alien dungeons (Neo Malogia/Katalund/Untaris/Forax): the
///   original alien UTs now also drop from the harder Neo-version bosses, so
///   their "Drops from" tooltip must list the Neo bosses -- but the base UTs
///   are deliberately kept OUT of the Neo collections (only the Neo-exclusive
///   forge upgrades belong there);
/// - The Forgotten Ring, which RealmEye lists as dropping from two Shatters
///   objectives (the Royal Epic Quest Chest is a quest reward, not a drop).
///
/// These are injected into `item_to_sources` only -- never into
/// `dungeon_to_items` -- so collection membership is untouched.
///
/// Each Neo boss drops exactly the same UT id set as its original counterpart
/// (verified against RealmEye item pages). Tuple: (source name, location, item
/// ids).
const ADDITIONAL_SOURCES: &[(&str, &str, &[i32])] = &[
    // Neo Katalund
    ("Neo Golden Cannon", "Neo Katalund", &[7494, 25699]),
    (
        "Neo Golden Sentreenel",
        "Neo Katalund",
        &[7489, 7494, 25691, 25699],
    ),
    (
        "Golden Sphinx Mk II",
        "Neo Katalund",
        &[7489, 7494, 25691, 25695, 25699, 25701, 52066],
    ),
    (
        "Neo Katalund Satellite Core",
        "Neo Katalund",
        &[7489, 7494, 25691, 25695, 25699, 25701, 52066],
    ),
    // Neo Malogia
    ("Neo Roonogian", "Neo Malogia", &[7492, 25696]),
    ("Neo Syogian", "Neo Malogia", &[7492, 25693, 25696, 52070]),
    (
        "King Suesogian I",
        "Neo Malogia",
        &[7488, 7492, 25686, 25690, 25693, 25696, 52070],
    ),
    (
        "Neo Malogia Satellite Core",
        "Neo Malogia",
        &[7488, 7492, 25686, 25690, 25693, 25696, 52070],
    ),
    // Neo Untaris
    ("Neo Clingstar", "Neo Untaris", &[7491, 25694]),
    ("Neo Clax", "Neo Untaris", &[7491, 25694, 25698, 52068]),
    (
        "Tarul, Eyes of the Cosmos",
        "Neo Untaris",
        &[7491, 7495, 25694, 25698, 25700, 25703, 52068],
    ),
    (
        "Neo Untaris Satellite Core",
        "Neo Untaris",
        &[7491, 7495, 25694, 25698, 25700, 25703, 52068],
    ),
    // Neo Forax
    ("Neo Heartbat", "Neo Forax", &[25688, 52064]),
    ("Neo Waste", "Neo Forax", &[7490, 25688, 25692, 52064]),
    (
        "Acidus, Natural Disaster",
        "Neo Forax",
        &[7490, 7493, 25688, 25692, 25697, 25702, 52064],
    ),
    (
        "Neo Forax Satellite Core",
        "Neo Forax",
        &[7490, 7493, 25688, 25692, 25697, 25702, 52064],
    ),
    // The Forgotten Ring + Shiny (Shatters objectives; not in RealmEye's scrape).
    ("Derelict Monument", "The Shatters", &[9669, 7512]),
    ("Tablet of the Monarchy", "The Shatters", &[9669, 7512]),
    // The Gardener's Valentine whites (not in RealmEye's scrape). Base ids only;
    // the shiny gems inherit these sources via the shiny -> base display-name
    // fallback in the item tooltip. Soulful Affection is combine-only but is
    // listed here as a Gardener source per product request.
    ("The Gardener", "Realm", &[1904, 4053, 4054, 4055]),
    // Prismimic white drops from the Prismimic Defender.
    ("Prismimic Defender", "Realm", &[19254]),
    // Forgotten Legacy event white bags (encounter bosses; not in the scrape).
    ("Artificial Slop", "Realm", &[47999]),
    ("Cold Soul", "Realm", &[47955]),
    ("Towering Perfection", "Realm", &[48003]),
    ("Man-eating Barnacle", "Realm", &[48143]),
    ("Stygian Mirror", "Realm", &[47998]),
    ("Cube Deity", "Realm", &[47947]),
    ("Daughter of Limon", "Realm", &[48139]),
    ("Astral Rift", "Realm", &[47977]),
    ("Possessed Pumpkin", "Realm", &[48142]),
    ("Legion General", "Realm", &[48081]),
    ("Ancient Kaiju", "Realm", &[48129]),
    ("Skeletal Centipede", "Realm", &[48144]),
    ("Ravenous Rot", "Realm", &[47907]),
];

impl RealmEyeDropData {
    /// Normalize Unicode characters to their ASCII equivalents.
    fn normalize_name(name: &str) -> String {
        name.replace('\u{2019}', "'").replace('\u{00A0}', " ")
    }

    /// Resolve a raw drop-source name to its display name. Normalizes Unicode and
    /// applies per-source renames: RealmEye lists three Moonlight Village whites
    /// as dropping from "Village Girl Umi", but that source is really the MV
    /// Fishing Loot crate. Renaming here (rather than a whole-item override)
    /// keeps the whites' genuine boss-dancer sources and lists the crate last.
    fn resolve_source_name(raw: &str) -> String {
        let name = Self::normalize_name(raw);
        match name.as_str() {
            "Village Girl Umi" => "MV Fishing Loot".to_string(),
            _ => name,
        }
    }

    fn from_json(json: &str) -> Self {
        let raw: RawCompact = serde_json::from_str(json).unwrap_or_else(|e| {
            eprintln!("Failed to parse realmeye_drops.json: {e}");
            RawCompact {
                sources: Vec::new(),
                dungeons: Vec::new(),
                biomes: Vec::new(),
            }
        });

        let mut item_to_dungeons: HashMap<i32, Vec<String>> = HashMap::new();
        let mut item_to_sources: HashMap<i32, Vec<DropSource>> = HashMap::new();
        let mut dungeon_to_items: HashMap<String, Vec<i32>> = HashMap::new();
        let mut dungeon_portals: HashMap<String, i32> = HashMap::new();

        for (raw_dungeon_name, portal_id, drops) in &raw.dungeons {
            let dungeon_name = Self::normalize_name(raw_dungeon_name);
            if let Some(pid) = portal_id {
                dungeon_portals.insert(dungeon_name.clone(), *pid);
            }

            let mut all_items: Vec<i32> = Vec::new();

            for (src_idx, item_ids) in drops {
                let source_name = Self::resolve_source_name(
                    raw.sources
                        .get(*src_idx)
                        .map(|s| s.as_str())
                        .unwrap_or_default(),
                );

                for &item_id in item_ids {
                    item_to_dungeons
                        .entry(item_id)
                        .or_default()
                        .push(dungeon_name.clone());

                    item_to_sources
                        .entry(item_id)
                        .or_default()
                        .push(DropSource {
                            source_name: source_name.clone(),
                            location_name: dungeon_name.clone(),
                            location_type: DropLocationType::Dungeon,
                            seasonal: false,
                        });

                    if !all_items.contains(&item_id) {
                        all_items.push(item_id);
                    }
                }
            }

            dungeon_to_items.insert(dungeon_name.clone(), all_items);
        }

        for (biome_name, tier, drops) in &raw.biomes {
            let biome_seasonal = tier.eq_ignore_ascii_case("seasonal");
            for (src_idx, item_ids) in drops {
                let source_name = Self::resolve_source_name(
                    raw.sources
                        .get(*src_idx)
                        .map(|s| s.as_str())
                        .unwrap_or_default(),
                );

                for &item_id in item_ids {
                    item_to_dungeons
                        .entry(item_id)
                        .or_default()
                        .push("Realm".to_string());

                    item_to_sources
                        .entry(item_id)
                        .or_default()
                        .push(DropSource {
                            source_name: source_name.clone(),
                            location_name: biome_name.clone(),
                            location_type: DropLocationType::Biome,
                            seasonal: biome_seasonal,
                        });

                    dungeon_to_items
                        .entry("Realm".to_string())
                        .or_default()
                        .push(item_id);
                }
            }
        }

        for dungeons in item_to_dungeons.values_mut() {
            dungeons.sort();
            dungeons.dedup();
        }

        for items in dungeon_to_items.values_mut() {
            items.sort();
            items.dedup();
        }

        // Append Neo-dungeon boss sources to the base alien UTs' tooltips only.
        // Deliberately NOT added to item_to_dungeons / dungeon_to_items so the
        // base UTs stay out of the Neo collections.
        for (source_name, location_name, item_ids) in ADDITIONAL_SOURCES {
            for &item_id in *item_ids {
                item_to_sources
                    .entry(item_id)
                    .or_default()
                    .push(DropSource {
                        source_name: source_name.to_string(),
                        location_name: location_name.to_string(),
                        location_type: DropLocationType::Dungeon,
                        seasonal: false,
                    });
            }
        }

        Self {
            item_to_dungeons,
            item_to_sources,
            dungeon_to_items,
            dungeon_portals,
        }
    }

    pub fn dungeons_for_item(&self, item_id: i32) -> &[String] {
        self.item_to_dungeons
            .get(&item_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn sources_for_item(&self, item_id: i32) -> &[DropSource] {
        self.item_to_sources
            .get(&item_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn items_for_dungeon(&self, dungeon_name: &str) -> &[i32] {
        self.dungeon_to_items
            .get(dungeon_name)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn portal_for_dungeon(&self, dungeon_name: &str) -> Option<i32> {
        self.dungeon_portals.get(dungeon_name).copied()
    }

    pub fn has_item(&self, item_id: i32) -> bool {
        self.item_to_sources.contains_key(&item_id)
    }

    /// All dungeon names that have at least one item drop.
    pub fn all_dungeon_names(&self) -> impl Iterator<Item = &str> {
        self.dungeon_to_items.keys().map(|s| s.as_str())
    }

    /// Check if a source name is a category (starts with "All ").
    pub fn is_category_source(source_name: &str) -> bool {
        source_name.starts_with("All ")
    }
}

/// Item IDs for weapons and ST rings from the Neo Forax/Katalund/Malogia/
/// Untaris dungeons. RealmEye's wiki lists these as dropping from the
/// "Satellite Core" boss, but they're actually forge-crafted from materials
/// (the dungeon's "Essence" item) rather than dropped by any enemy.
const FORGE_CRAFTED_ITEMS: &[i32] = &[
    // Neo Malogia weapons + ST ring
    56435, 56455, 56452, 56467, // Neo Katalund weapons + ST ring
    56459, 56453, 56436, 56468, // Neo Forax weapons + ST ring
    56451, 56454, 56457, 56469, // Neo Untaris weapons + ST ring
    56456, 56458, 56460, 56470,
    // Mayhem Medallion, Ring of Wrath, Autarch Amulet: RealmEye lists these
    // as dropping from Inspector Stromwell in Stromwell's Rift, but they're
    // forge-crafted only.
    32416, 32417, 32418,
    // Kogbold Steamworks forge upgrades (base + shiny): not dropped directly,
    // crafted from the dungeon's Kogbold Enhancement Core.
    49468, 3980, // Clockwork Repeater + Shiny
    49469, 3981, // Mechanical Kogbold Limb + Shiny
    49470, 3982, // Atavistic Soul Sabre + Shiny
    49471, 3983, // Socket Blade + Shiny
    // Spectral Penitentiary forge upgrades (base + shiny).
    23627, 5334, // Deliverance + Shiny
    23623, 5335, // Pandemic Poison + Shiny
    23625, 5336, // Boundless Vessel + Shiny
    23624, 5337, // Amulet of Restoration + Shiny
    // Moonlight Village forge upgrades (no shiny).
    20745, // Fortitude
    20766, // Mischief
    20746, // Vigor
];

/// True if this item is a forge-crafted output that should show "Forge
/// craft" as its drop source instead of the scraped RealmEye mob/dungeon.
pub fn is_forge_crafted(item_id: i32) -> bool {
    FORGE_CRAFTED_ITEMS.contains(&item_id)
}

/// Per-item overrides for the scraped drop source name. RealmEye lists
/// "Neo Essence" as dropping from a "Forge Craft" pseudo-category, but it
/// actually drops from the mob Commander Calbrik.
///
/// Moonlight Village fishing rods are scraped as dropping from "Kitsune Umi",
/// but they actually come from the MV Fishing Loot crates she spawns (7 tiers,
/// collapsed to one generic "MV Fishing Loot" row). The Legendary Fishing Rod
/// is excluded here -- it is Tinkerer-only and gets its own custom note.
///
/// (Sage's Wakibiki, Ethereal Happi and Flowering Kimono keep their genuine
/// boss-dancer sources; their bogus "Village Girl Umi" source is renamed to
/// "MV Fishing Loot" in [`RealmEyeDropData::resolve_source_name`] instead.)
///
/// Titan's Heart is a new Ancient Ruins UT (Sandstone Titan) added before
/// RealmEye's scrape lists it, so its source is injected here by name.
const SOURCE_NAME_OVERRIDE: &[(i32, &str)] = &[
    (56190, "Commander Calbrik"),          // Neo Essence
    (20703, "MV Fishing Loot"),            // Basic Fishing Rod
    (20722, "MV Fishing Loot"),            // Intermediate Fishing Rod
    (20810, "MV Fishing Loot"),            // Expert Fishing Rod
    (20723, "MV Fishing Loot"),            // Master Fishing Rod
    (3920, "MV Fishing Loot"),             // Master Fishing Rod (Shiny)
    (17565, "Adolescent Green Beehemoth"), // Green Beehemoth Armor (Plagued Nest)
    (57471, "Sandstone Titan"),            // Titan's Heart (Ancient Ruins)
];

/// The Legendary Fishing Rod is not fished up like the other rods; it is a
/// once-per-account Tinkerer reward traded for a Fishing Award. Its tooltip
/// shows a custom "obtained by trading" note instead of a drop source.
pub const LEGENDARY_FISHING_ROD_ITEM_ID: i32 = 20726;
/// The Fishing Award consumable traded for the Legendary Fishing Rod.
pub const FISHING_AWARD_ITEM_ID: i32 = 20832;
/// A representative MV Fishing Loot crate object (tier 1), used to render the
/// generic "MV Fishing Loot" source sprite.
pub const MV_FISHING_LOOT_OBJECT_ID: i32 = 20789;

/// True if this item is the Tinkerer-traded Legendary Fishing Rod.
pub fn is_legendary_fishing_rod(item_id: i32) -> bool {
    item_id == LEGENDARY_FISHING_ROD_ITEM_ID
}

/// The Green Beehemoth Quiver and its shiny drop from the Killer Bee Queen in
/// the Plagued Nest. RealmEye has no drop data for them, so their tooltip shows
/// a custom note pairing the Killer Bee Queen with the Plagued Nest portal to
/// mark the dungeon-specific version.
pub const GREEN_BEEHEMOTH_QUIVER_ITEM_ID: i32 = 17564;
/// Shiny variant of the Green Beehemoth Quiver.
pub const GREEN_BEEHEMOTH_QUIVER_SHINY_ITEM_ID: i32 = 5346;
/// The Killer Bee Queen enemy object (source of the Green Beehemoth Quiver).
pub const KILLER_BEE_QUEEN_OBJECT_ID: i32 = 4243;
/// The Plagued Nest portal object, used to mark the dungeon-specific version.
pub const PLAGUED_NEST_PORTAL_OBJECT_ID: i32 = 17570;

/// True if this item is a Green Beehemoth Quiver (regular or shiny), which uses
/// the custom Plagued Nest Killer Bee Queen drop note.
pub fn is_plagued_nest_beehemoth_quiver(item_id: i32) -> bool {
    item_id == GREEN_BEEHEMOTH_QUIVER_ITEM_ID || item_id == GREEN_BEEHEMOTH_QUIVER_SHINY_ITEM_ID
}

/// Soulful Affection and its two combine ingredients. Soulful Affection drops
/// from The Gardener but is also craftable by combining these two gems, so its
/// tooltip shows an extra combine note with both gem icons. The shiny variant is
/// deliberately excluded: combining the two non-shiny gems yields the non-shiny
/// ring, and whether combining the shiny gems yields a shiny is unconfirmed.
pub const SOULFUL_AFFECTION_ITEM_ID: i32 = 4055;
pub const SOULFUL_AFFECTION_SHINY_ITEM_ID: i32 = 7328;
pub const GEM_OF_TENDERNESS_ITEM_ID: i32 = 4053;
pub const GEM_OF_ADORATION_ITEM_ID: i32 = 4054;

/// True only for the non-shiny Soulful Affection, which carries a custom "also
/// obtainable by combining" note in addition to its drop source.
pub fn is_soulful_affection(item_id: i32) -> bool {
    item_id == SOULFUL_AFFECTION_ITEM_ID
}

/// Overridden drop source name for this item, if any.
pub fn source_name_override(item_id: i32) -> Option<&'static str> {
    SOURCE_NAME_OVERRIDE
        .iter()
        .find(|(id, _)| *id == item_id)
        .map(|(_, name)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loads_and_queries() {
        let data = get_realmeye_drops();

        // Demon Blade (17840) should drop in Abyss of Demons
        let dungeons = data.dungeons_for_item(17840);
        assert!(
            dungeons.contains(&"Abyss of Demons".to_string()),
            "Demon Blade should drop in Abyss of Demons, got: {dungeons:?}"
        );

        // Adamantine Helm (14875) should drop in Infernal Abyss
        let dungeons = data.dungeons_for_item(14875);
        assert!(
            dungeons.contains(&"Infernal Abyss of Demons".to_string()),
            "Adamantine Helm should drop in Infernal Abyss, got: {dungeons:?}"
        );

        // Dirk of Cronus (3082) should have biome sources from Cube God
        let sources = data.sources_for_item(3082);
        assert!(!sources.is_empty(), "Dirk of Cronus should have sources");
        assert!(
            sources.iter().any(|s| s.source_name == "Cube God"),
            "Dirk should drop from Cube God, got: {sources:?}"
        );
    }

    #[test]
    fn titans_heart_sources_from_sandstone_titan() {
        // Titan's Heart is a new Ancient Ruins UT not yet in RealmEye's scrape,
        // so its drop source is injected by name.
        assert_eq!(source_name_override(57471), Some("Sandstone Titan"));
    }

    #[test]
    fn alien_uts_gain_neo_sources_without_neo_collection_membership() {
        let data = get_realmeye_drops();
        // Warlord Wand (25699): original Katalund bosses + all 4 Neo Katalund bosses.
        let sources = data.sources_for_item(25699);
        let names: Vec<&str> = sources.iter().map(|s| s.source_name.as_str()).collect();
        for expected in [
            "Tin Cannon",
            "Copper Sentreenel",
            "Golden Sphinx",
            "Katalund Satellite Core",
            "Neo Golden Cannon",
            "Neo Golden Sentreenel",
            "Golden Sphinx Mk II",
            "Neo Katalund Satellite Core",
        ] {
            assert!(
                names.contains(&expected),
                "Warlord Wand missing source {expected}: {names:?}"
            );
        }
        // Mirror: a partial-drop item must NOT gain a Neo source for a boss its
        // original counterpart doesn't drop. Foramite Staff line: 25693 (Fire
        // Blade) drops from Syogian/Suesogian/SatCore but not Roonogian, so no
        // Neo Roonogian either.
        let names_25693: Vec<&str> = data
            .sources_for_item(25693)
            .iter()
            .map(|s| s.source_name.as_str())
            .collect();
        assert!(names_25693.contains(&"Neo Syogian"), "{names_25693:?}");
        assert!(!names_25693.contains(&"Neo Roonogian"), "{names_25693:?}");
        // Base UTs must stay OUT of the Neo collections' item lists.
        for neo in ["Neo Katalund", "Neo Malogia", "Neo Untaris", "Neo Forax"] {
            let items = data.items_for_dungeon(neo);
            assert!(
                !items.contains(&25699),
                "{neo} must not include base Warlord Wand"
            );
            assert!(
                !items.contains(&7488),
                "{neo} must not include base alien UT 7488"
            );
        }
    }

    #[test]
    fn forgotten_ring_has_two_shatters_sources() {
        let data = get_realmeye_drops();
        for id in [9669, 7512] {
            let names: Vec<&str> = data
                .sources_for_item(id)
                .iter()
                .map(|s| s.source_name.as_str())
                .collect();
            assert!(names.contains(&"Derelict Monument"), "item {id}: {names:?}");
            assert!(
                names.contains(&"Tablet of the Monarchy"),
                "item {id}: {names:?}"
            );
        }
    }

    #[test]
    fn gardener_and_prismimic_whites_gain_event_sources() {
        let data = get_realmeye_drops();
        // The Gardener's Valentine whites (base ids) list The Gardener as a source.
        for id in [1904, 4053, 4054, 4055] {
            let names: Vec<&str> = data
                .sources_for_item(id)
                .iter()
                .map(|s| s.source_name.as_str())
                .collect();
            assert!(names.contains(&"The Gardener"), "item {id}: {names:?}");
        }
        // Prismimic lists the Prismimic Defender as its source.
        let prismimic: Vec<&str> = data
            .sources_for_item(19254)
            .iter()
            .map(|s| s.source_name.as_str())
            .collect();
        assert!(prismimic.contains(&"Prismimic Defender"), "{prismimic:?}");
    }

    #[test]
    fn forgotten_legacy_event_whites_have_boss_sources() {
        let data = get_realmeye_drops();
        let cases = [
            (47999, "Artificial Slop"),
            (47955, "Cold Soul"),
            (48003, "Towering Perfection"),
            (48143, "Man-eating Barnacle"),
            (47998, "Stygian Mirror"),
            (47947, "Cube Deity"),
            (48139, "Daughter of Limon"),
            (47977, "Astral Rift"),
            (48142, "Possessed Pumpkin"),
            (48081, "Legion General"),
            (48129, "Ancient Kaiju"),
            (48144, "Skeletal Centipede"),
            (47907, "Ravenous Rot"),
        ];
        for (id, boss) in cases {
            let names: Vec<&str> = data
                .sources_for_item(id)
                .iter()
                .map(|s| s.source_name.as_str())
                .collect();
            assert!(
                names.contains(&boss),
                "item {id} missing source {boss}: {names:?}"
            );
        }
    }

    #[test]
    fn fishing_rods_override_to_mv_fishing_loot() {
        // The five fished rods show the generic crate as their sole source...
        for id in [20703, 20722, 20810, 20723, 3920] {
            assert_eq!(
                source_name_override(id),
                Some("MV Fishing Loot"),
                "item {id}"
            );
            assert!(
                !is_legendary_fishing_rod(id),
                "item {id} is not the Legendary rod"
            );
        }
        // ...while the Legendary rod is Tinkerer-only (custom note, no source override).
        assert_eq!(source_name_override(LEGENDARY_FISHING_ROD_ITEM_ID), None);
        assert!(is_legendary_fishing_rod(LEGENDARY_FISHING_ROD_ITEM_ID));
    }

    #[test]
    fn fished_whites_keep_dancers_and_rename_umi_to_crate() {
        let data = get_realmeye_drops();
        // Sage's Wakibiki, Ethereal Happi, Flowering Kimono are not whole-item
        // overridden -- they keep their genuine boss-dancer sources, with the
        // bogus "Village Girl Umi" source renamed to "MV Fishing Loot" (last).
        for id in [20468, 20604, 20706] {
            assert_eq!(source_name_override(id), None, "item {id}");
            let names: Vec<&str> = data
                .sources_for_item(id)
                .iter()
                .map(|s| s.source_name.as_str())
                .collect();
            assert!(names.contains(&"Dancer Miko"), "item {id}: {names:?}");
            assert!(names.contains(&"Drummer Kaguya"), "item {id}: {names:?}");
            assert!(names.contains(&"Sage Genji"), "item {id}: {names:?}");
            assert!(names.contains(&"MV Fishing Loot"), "item {id}: {names:?}");
            assert!(!names.contains(&"Village Girl Umi"), "item {id}: {names:?}");
        }
    }

    #[test]
    fn plagued_nest_beehemoth_collection_sources() {
        // Green Beehemoth Armor uses the standard single-source override.
        assert_eq!(
            source_name_override(17565),
            Some("Adolescent Green Beehemoth")
        );
        // Green Beehemoth Quiver + shiny use the custom Plagued Nest note, not
        // a plain source override.
        for id in [
            GREEN_BEEHEMOTH_QUIVER_ITEM_ID,
            GREEN_BEEHEMOTH_QUIVER_SHINY_ITEM_ID,
        ] {
            assert!(is_plagued_nest_beehemoth_quiver(id), "item {id}");
            assert_eq!(source_name_override(id), None, "item {id}");
        }
        // The armor is not a quiver.
        assert!(!is_plagued_nest_beehemoth_quiver(17565));
    }
}

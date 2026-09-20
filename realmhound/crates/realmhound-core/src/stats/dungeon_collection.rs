//! Dungeon item collection definitions.
//!
//! For each dungeon, builds a list of collectible items (UTs + STs) grouped
//! and sorted for display in the "Pokédex"-style collection view.
//!
//! Groups (in display order):
//! 1. Shiny UT items, by slot (weapon, ability, armor, ring), then alphabetically
//! 2. UT items (non-shiny), by slot, then alphabetically
//! 3. ST items, grouped by set and ordered by slot

use std::collections::HashMap;

use crate::assets::{get_asset_manager, get_realmeye_drops, ObjectAsset};

/// A single collectible item in a dungeon collection.
#[derive(Debug, Clone)]
pub struct CollectionItem {
    pub item_id: i32,
    pub name: String,
    pub group: ItemGroup,
    /// For ST items: which set this belongs to (shared prefix from id_name).
    pub st_set_key: Option<String>,
    /// ST set display name from equip.xml (e.g. "Legacy Skuld 2 The ReGhostening Set").
    pub set_name: Option<String>,
    /// Slot ordering (weapon=0, ability=1, armor=2, ring=3).
    pub slot_order: u8,
}

/// Which group an item belongs to in the collection display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ItemGroup {
    ShinyUt = 0,
    Ut = 1,
    St = 2,
    /// Upgrade materials (e.g. Vial of Soul Extract, Kogbold Enhancement Core)
    /// that aren't tagged UT/ST but are still collectible dungeon drops.
    Upgrade = 3,
}

/// The full collection definition for a single dungeon.
#[derive(Debug, Clone)]
pub struct DungeonCollectionDef {
    pub dungeon_name: String,
    pub items: Vec<CollectionItem>,
    /// For special dungeons (O3, Machine): items grouped by boss/section.
    pub sections: Option<Vec<CollectionSection>>,
}

/// A named section within a dungeon collection (e.g. a boss in O3).
#[derive(Debug, Clone)]
pub struct CollectionSection {
    pub name: String,
    pub items: Vec<CollectionItem>,
}

impl DungeonCollectionDef {
    pub fn total_items(&self) -> usize {
        self.items.len()
    }

    pub fn is_sectioned(&self) -> bool {
        self.sections.is_some()
    }
}

/// Dungeons whose scraped drop tables are wholesale duplicates of another
/// dungeon's collection (they reuse the same encounters and have no drops of
/// their own). Completions are still tracked normally; only the item
/// collection view is suppressed for these.
const NO_COLLECTION_DUNGEONS: &[&str] = &[
    "White Snake Invasion I",
    "White Snake Invasion II",
    "Stromwell's Rift I",
    "Stromwell's Rift II",
    "Legacy Heroic Abyss of Demons",
    "Legacy Heroic Undead Lair",
    "Advanced Kogbold Steamworks",
];

/// Per-dungeon allow-lists that restrict a collection to specific item IDs,
/// used when most of a dungeon's scraped drop table is duplicated from
/// another dungeon but a handful of items are genuinely exclusive to it.
/// Infernal Abyss of Demons shares its boss's loot table with Abyss of
/// Demons except for Adamantine Helm (+ shiny) and Sword of Illumination.
/// Heroic Undead Lair shares its loot table with Undead Lair except for
/// Bow of the Morning Star and Spirit's Bane.
/// Plagued Nest shares its boss's loot table with The Nest except for the
/// Green Beehemoth Quiver (+ shiny) and Green Beehemoth Armor, which are
/// exclusive to Killer Bee Queen within Plagued Nest.
const ALLOWED_ITEMS_OVERRIDE: &[(&str, &[i32])] = &[
    ("Infernal Abyss of Demons", &[7397, 14875, 8963]),
    ("Heroic Undead Lair", &[8961, 14698]),
    ("Plagued Nest", &[17564, 5346, 17565]),
];

/// Per-dungeon deny-lists that drop a handful of specific item IDs from an
/// otherwise-valid collection. Oryx's Chamber's Oryx the Mad God 1 encounter
/// drops Theurgy Wand + Ceremonial Merlot (Priest ST set), but the full
/// Priest ST set already drops in Oryx's Castle, so we keep those two items
/// there only and hide them from Chamber's collection.
/// Crystal Cavern shares Ring of Decades (+ shiny) and the full Crystal
/// Kunoichi ST set with Fungal Cavern, so we keep those in Fungal Cavern
/// only and hide them from Crystal Cavern's collection.
const DENIED_ITEMS_OVERRIDE: &[(&str, &[i32])] = &[
    ("Oryx's Chamber", &[8204, 8205]),
    ("Crystal Cavern", &[2990, 3921, 19260, 19262, 19264, 19265]),
    // Legacy duplicate "Cavalry Lance 2" (id_name); the live white is 20904.
    ("Realm", &[3851]),
];

/// Per-dungeon extra item IDs to include in a collection beyond what's
/// scraped from RealmEye's drop tables. Used for forge-crafted items that
/// aren't dropped directly but are exclusive outputs tied to a dungeon's
/// crafting material (e.g. Kogbold Enhancement Core upgrades). Precursor
/// weapons used as crafting ingredients (Doom Bow, Tezcacoatl's Tail,
/// Ancient Stone Sword, Void Blade) are intentionally excluded.
const EXTRA_ITEMS_OVERRIDE: &[(&str, &[i32])] = &[
    (
        // RealmEye's scrape no longer lists these under Plagued Nest (it's
        // identical to The Nest's table), so they must be injected manually.
        "Plagued Nest",
        &[17564, 5346, 17565], // Beehemoth Quiver + Shiny, Beehemoth Armor
    ),
    (
        "Kogbold Steamworks",
        &[
            49468, 3980, // Clockwork Repeater + Shiny
            49469, 3981, // Mechanical Kogbold Limb + Shiny
            49470, 3982, // Atavistic Soul Sabre + Shiny
            49471, 3983, // Socket Blade + Shiny
        ],
    ),
    (
        "Spectral Penitentiary",
        &[
            23627, 5334, // Deliverance + Shiny
            23623, 5335, // Pandemic Poison + Shiny
            23625, 5336, // Boundless Vessel + Shiny
            23624, 5337, // Amulet of Restoration + Shiny
        ],
    ),
    (
        "Moonlight Village",
        &[
            20745, // Fortitude (no shiny)
            20766, // Mischief (no shiny)
            20746, // Vigor (no shiny)
        ],
    ),
    (
        // Not listed in RealmEye's Realm drop table; inject manually.
        "Realm",
        &[
            65531, // Kendo Stick Shiny
            7501,  // Hama Yumi Shiny
            3990,  // Greaterhosen Shiny
            // The Gardener's Valentine whites (+ shinies where they exist).
            1904, // Heart of Gold Prism (no shiny)
            4053, 7326, // Gem of Tenderness + Shiny
            4054, 7327, // Gem of Adoration + Shiny
            4055, 7328,  // Soulful Affection + Shiny
            19254, // Prismimic (no shiny)
            // Forgotten Legacy event white bags (new; not in the scrape).
            47999, // Shifting Shroud (Artificial Slop)
            47955, // Cold Front (Cold Soul)
            48003, // Babel Blocks (Towering Perfection)
            48143, // Barnacle Shell (Man-eating Barnacle)
            47998, // Obsidian Lyre (Stygian Mirror)
            47947, // Rectangular Prism (Cube Deity)
            48139, // Wrathful Conduit (Daughter of Limon)
            47977, // Rift Rippers (Astral Rift)
            48142, // Scary Stories (Possessed Pumpkin)
            48081, // General's Arsenal (Legion General)
            48129, // Fossilized Skull (Ancient Kaiju)
            48144, // Skeletal Sigilpede (Skeletal Centipede)
            47907, // Ravenous Wand (Ravenous Rot)
            // Shiny copies of existing biome whites (not in the scrape).
            53212, 53213, 53214, 53215, 53216, 53217, 53218, 53219, 53220, 53221, 53222, 53223,
            53224,
        ],
    ),
    // Newly added shiny counterparts of existing dungeon UTs; placed in the
    // same dungeon collection as their non-shiny base until RealmEye's scrape
    // lists them.
    (
        "Crystal Cavern",
        &[7503], // Crystalline Sigil Shiny
    ),
    (
        "Fungal Cavern",
        &[7509], // Crystallized Worm Spellblade Shiny
    ),
    (
        "Ice Citadel",
        &[7506], // Esben's Vows Shiny
    ),
    (
        "Woodland Labyrinth",
        &[7511], // Elder Heartwood Staff Shiny
    ),
    (
        // New UT added to Ancient Ruins (Sandstone Titan); not yet in the scrape.
        "Ancient Ruins",
        &[57471], // Titan's Heart
    ),
    (
        // The Forgotten Ring (Ring of Decades reskin) and its shiny; not present
        // in RealmEye's scrape. Routed into the "Armors and rings" section via
        // SECTION_OVERRIDE below.
        "The Shatters",
        &[9669, 7512], // The Forgotten Ring + Shiny
    ),
];

/// Build the collection definition for a single dungeon.
pub fn build_collection_for_dungeon(dungeon_name: &str) -> DungeonCollectionDef {
    if NO_COLLECTION_DUNGEONS.contains(&dungeon_name) {
        return DungeonCollectionDef {
            dungeon_name: dungeon_name.to_string(),
            items: Vec::new(),
            sections: None,
        };
    }

    let realmeye = get_realmeye_drops();
    let asset_mgr = get_asset_manager();
    let item_ids = realmeye.items_for_dungeon(dungeon_name);
    let allowed_override = ALLOWED_ITEMS_OVERRIDE
        .iter()
        .find(|(name, _)| *name == dungeon_name)
        .map(|(_, ids)| *ids);
    let denied_override = DENIED_ITEMS_OVERRIDE
        .iter()
        .find(|(name, _)| *name == dungeon_name)
        .map(|(_, ids)| *ids)
        .unwrap_or(&[]);
    let extra_items = EXTRA_ITEMS_OVERRIDE
        .iter()
        .find(|(name, _)| *name == dungeon_name)
        .map(|(_, ids)| *ids)
        .unwrap_or(&[]);

    let mut items: Vec<CollectionItem> = Vec::new();

    for &item_id in item_ids.iter().chain(extra_items.iter()) {
        if denied_override.contains(&item_id) {
            continue;
        }
        if let Some(allowed) = allowed_override {
            if !allowed.contains(&item_id) {
                continue;
            }
        }

        let Some(obj) = asset_mgr.get_object(item_id) else {
            continue;
        };

        if !obj.is_equipment() {
            continue;
        }

        let group = if obj.is_ut() && obj.is_shiny() {
            ItemGroup::ShinyUt
        } else if obj.is_ut() {
            ItemGroup::Ut
        } else if obj.is_st() {
            ItemGroup::St
        } else if obj.is_upgrade_material() {
            ItemGroup::Upgrade
        } else {
            continue;
        };

        let slot_order = slot_order_from_labels(&obj);
        let st_set_key = if group == ItemGroup::St {
            Some(st_set_key_from_id_name(&obj.id_name))
        } else {
            None
        };
        let set_name = obj.set_name.clone();

        items.push(CollectionItem {
            item_id,
            name: obj.name().to_string(),
            group,
            st_set_key,
            set_name,
            slot_order,
        });
    }

    items.sort_by(|a, b| {
        a.group.cmp(&b.group).then_with(|| match a.group {
            ItemGroup::St => {
                let a_key = a.st_set_key.as_deref().unwrap_or("");
                let b_key = b.st_set_key.as_deref().unwrap_or("");
                a_key
                    .cmp(b_key)
                    .then_with(|| a.slot_order.cmp(&b.slot_order))
            }
            _ => a
                .slot_order
                .cmp(&b.slot_order)
                .then_with(|| {
                    manual_sort_rank(dungeon_name, a.item_id)
                        .cmp(&manual_sort_rank(dungeon_name, b.item_id))
                })
                .then_with(|| a.name.cmp(&b.name)),
        })
    });

    let sections = build_sections_if_special(dungeon_name, &items);

    DungeonCollectionDef {
        dungeon_name: dungeon_name.to_string(),
        items,
        sections,
    }
}

/// Build collection definitions for all dungeons that have collectible items.
pub fn build_all_collections() -> HashMap<String, DungeonCollectionDef> {
    let realmeye = get_realmeye_drops();
    let mut result = HashMap::new();

    for dungeon_name in realmeye.all_dungeon_names() {
        let collection = build_collection_for_dungeon(dungeon_name);
        if !collection.items.is_empty() {
            result.insert(dungeon_name.to_string(), collection);
        }
    }

    result
}

/// Manual ordering override within a slot group for specific dungeons, used
/// as a tie-breaker before falling back to alphabetical name sort. Lower
/// ranks sort first; items not listed default to rank 1 (after rank-0 items).
fn manual_sort_rank(dungeon_name: &str, item_id: i32) -> u8 {
    match (dungeon_name, item_id) {
        // Kagenohikari should appear right after armors, before the fishing rod rings.
        ("Moonlight Village", 20795) => 0,
        _ => 1,
    }
}

/// Dungeons that get sectioned (boss-grouped) display.
const SECTIONED_DUNGEONS: &[&str] = &[
    "Oryx's Sanctuary",
    "The Machine",
    "The Shatters",
    "Lair of Draconis",
    "Stromwell's Rift I",
    "Stromwell's Rift II",
    "Stromwell's Rift III",
    "Realm",
    "Kogbold Steamworks",
    "Spectral Penitentiary",
    "Moonlight Village",
];

/// O3 boss display order: O3 first, then mini-bosses alphabetically.
const O3_BOSS_ORDER: &[&str] = &[
    "Oryx the Mad God 3",
    "Archbishop Leucoryx",
    "Chancellor Dammah",
    "Chief Beisa",
    "Treasurer Gemsbok",
];

pub fn is_sectioned_dungeon(name: &str) -> bool {
    SECTIONED_DUNGEONS.iter().any(|&d| d == name)
}

/// Dungeons where ST items stay with their boss (not separated).
/// Lair of Draconis: each dragon drops exactly one UT+shiny pair plus its own
/// ST set, so keeping them together yields one compact section per dragon
/// instead of splitting the ST sets into extra sections.
const ST_MIXED_DUNGEONS: &[&str] = &["Lair of Draconis"];

/// Per-dungeon, per-item overrides that force an item into a specific
/// display section, bypassing the normal source-name/ST grouping logic.
/// Used to merge a lone one-item boss section into a related section to cut
/// down on section count, while the item's "Drops from" tooltip (driven by
/// `sources_for_item`, untouched here) still shows its real source.
/// - Chrysalis of Eternity drops from King Azamoth (The Shatters' hard-mode
///   Forgotten King) but is folded into "The Forgotten King" section.
/// - Ring of Omni-Impotence is an ST ring that drops from Null, so it's
///   folded into "Armors and Rings" (the renamed AsiaEast section) instead
///   of Null's own "Weapons" section. Useless Katana stays in "Weapons"
///   (renamed from Null) since it's a weapon regardless of drop source.
const SECTION_OVERRIDE: &[(&str, i32, &str)] = &[
    ("The Shatters", 14214, "The Forgotten King"),
    ("The Shatters", 9669, "Armors and rings"), // The Forgotten Ring
    ("The Shatters", 7512, "Armors and rings"), // The Forgotten Ring Shiny
    ("The Machine", 23359, "Armors and Rings"), // Ring of Omni-Impotence
];

/// The Machine's boss-section names (scraped as literal enemy/area names)
/// renamed to reflect what each section actually contains.
const MACHINE_SECTION_RENAME: &[(&str, &str)] = &[
    ("AsiaEast", "Armors and Rings"),
    ("Null", "Weapons"),
    ("The Servers", "Abilities"),
];

/// Build boss sections for special dungeons.
fn build_sections_if_special(
    dungeon_name: &str,
    items: &[CollectionItem],
) -> Option<Vec<CollectionSection>> {
    if !is_sectioned_dungeon(dungeon_name) {
        return None;
    }

    if dungeon_name.starts_with("Stromwell's Rift") {
        return Some(group_by_slot_type(items));
    }

    if dungeon_name == "Realm" {
        return Some(group_by_realm_category(items));
    }

    if dungeon_name == "Kogbold Steamworks"
        || dungeon_name == "Spectral Penitentiary"
        || dungeon_name == "Moonlight Village"
    {
        return Some(group_by_forge_upgrade(dungeon_name, items));
    }

    let separate_st = !ST_MIXED_DUNGEONS.iter().any(|&d| d == dungeon_name);

    let realmeye = get_realmeye_drops();
    let mut source_map: HashMap<String, Vec<CollectionItem>> = HashMap::new();
    let mut st_items: Vec<CollectionItem> = Vec::new();

    for item in items {
        if let Some(&(_, _, forced)) = SECTION_OVERRIDE
            .iter()
            .find(|&&(d, id, _)| d == dungeon_name && id == item.item_id)
        {
            source_map
                .entry(forced.to_string())
                .or_default()
                .push(item.clone());
            continue;
        }
        if separate_st && item.group == ItemGroup::St {
            st_items.push(item.clone());
            continue;
        }
        // The Shatters: armors drop from many different minions, so group them
        // all under one "Armors and rings" section instead of scattering them
        // across every boss that can drop one. The Forgotten Ring (+ shiny) is
        // folded in here too via SECTION_OVERRIDE.
        if dungeon_name == "The Shatters" && item.slot_order == 2 {
            source_map
                .entry("Armors and rings".to_string())
                .or_default()
                .push(item.clone());
            continue;
        }
        let sources = realmeye.sources_for_item(item.item_id);
        let source_name = sources
            .iter()
            .find(|s| s.location_name == dungeon_name)
            .map(|s| s.source_name.clone())
            .unwrap_or_else(|| "Other".to_string());
        let source_name = if dungeon_name == "The Machine" {
            MACHINE_SECTION_RENAME
                .iter()
                .find(|(from, _)| *from == source_name)
                .map(|(_, to)| to.to_string())
                .unwrap_or(source_name)
        } else {
            source_name
        };
        source_map
            .entry(source_name)
            .or_default()
            .push(item.clone());
    }

    let mut sections: Vec<CollectionSection> = source_map
        .into_iter()
        .map(|(name, items)| CollectionSection { name, items })
        .collect();

    if dungeon_name.contains("Sanctuary") {
        sections.sort_by(|a, b| {
            let a_pos = O3_BOSS_ORDER
                .iter()
                .position(|&n| n == a.name)
                .unwrap_or(99);
            let b_pos = O3_BOSS_ORDER
                .iter()
                .position(|&n| n == b.name)
                .unwrap_or(99);
            a_pos.cmp(&b_pos).then_with(|| a.name.cmp(&b.name))
        });
    } else if dungeon_name == "The Shatters" {
        // Keep "Armors and rings" pinned last, other bosses alphabetical.
        sections.sort_by(|a, b| {
            let a_last = a.name == "Armors and rings";
            let b_last = b.name == "Armors and rings";
            a_last.cmp(&b_last).then_with(|| a.name.cmp(&b.name))
        });
    } else {
        sections.sort_by(|a, b| a.name.cmp(&b.name));
    }

    // Append ST sets after the boss sections. The Shatters merges every ST
    // set into one "ST" section (each set's items stay grouped together, in
    // set order) to save row space; other dungeons keep one section per set.
    if separate_st && !st_items.is_empty() {
        let mut set_map: HashMap<String, Vec<CollectionItem>> = HashMap::new();
        for item in &st_items {
            let key = item
                .set_name
                .clone()
                .or_else(|| item.st_set_key.clone())
                .unwrap_or_else(|| "ST Items".to_string());
            set_map.entry(key).or_default().push(item.clone());
        }
        let mut st_sections: Vec<CollectionSection> = set_map
            .into_iter()
            .map(|(name, mut items)| {
                items.sort_by(|a, b| a.slot_order.cmp(&b.slot_order));
                CollectionSection { name, items }
            })
            .collect();
        st_sections.sort_by(|a, b| a.name.cmp(&b.name));

        if dungeon_name == "The Shatters" {
            let merged_items: Vec<CollectionItem> = st_sections
                .into_iter()
                .flat_map(|section| section.items)
                .collect();
            sections.push(CollectionSection {
                name: "ST".to_string(),
                items: merged_items,
            });
        } else {
            sections.extend(st_sections);
        }
    }

    Some(sections)
}

/// Item IDs for "Realm" collection's special drop-source categories.
/// These are named event bosses/mechanics whose drops shouldn't be lumped in
/// with ordinary ambient "biome white" ultra-rares.
const CRYSTAL_PRISONER_ITEMS: &[i32] = &[2563, 65516, 2879, 7459, 14336, 2018];
const ALIEN_GEAR_ITEMS: &[i32] = &[
    1155, 1156, 1157, 7496, 7497, 7498, 48154, 56190, 56437, 56465, 56464, 56466, 56461, 56462,
    56463,
];
const JACK_FROST_ITEMS: &[i32] = &[
    1236, 1237, 1238, 1239, 1240, 1241, 12160, 12161, 12162, 12163, 12164, 12165,
];
const RAT_MINIGAME_ITEMS: &[i32] = &[18074, 18079]; // Piper's Pan Flute, Mouse Trap (Mammoth Rat)
                                                    // Biff the Buffed Bunny's own UTs, Keychain Cutlass (The Keyper),
                                                    // Incubation Mace (Easter-only biome + Biff drop; its all-year reskin,
                                                    // Minotaur Mace, is a permanent Event White instead), and Quintessential
                                                    // Quiver (Golden Archer Set ST quiver, dropped by seasonal boss The Glitch
                                                    // rather than in The Machine like the rest of the set). Also The Gardener's
                                                    // Valentine whites (+ shinies) and the Prismimic white.
const SEASONAL_UT_ITEMS: &[i32] = &[
    2216, 2217, 7410, 41739, 19212, 10304, 38515, 1401, 1904, // Heart of Gold Prism
    4053, 7326, // Gem of Tenderness + Shiny
    4054, 7327, // Gem of Adoration + Shiny
    4055, 7328,  // Soulful Affection + Shiny
    19254, // Prismimic
];
/// Regular (non-shiny) limited-event UTs (Tavern of Fortune, Moonlight lunar
/// festival tokens, Cronus/Ray Katana/Juggernaut/etc. named-event UTs); their
/// shiny counterparts go in `EVENT_WHITE_SHINY_ITEMS`. This list matches
/// RealmEye's official "Event Whites" page (realmeye.com/wiki/event-whites).
const EVENT_WHITE_ITEMS: &[i32] = &[
    15717, 15718, 17847, 17561, 20764, 20765, 3082, 2325, 3080, 3083, 3079, 3078, 6205, 3087, 2880,
    2881, 50307, 7433, 55382, 599, 1200, 23794, 4333, 4338, 4339,
    // Forgotten Legacy event white bags.
    47999, 47955, 48003, 48143, 47998, 47947, 48139, 47977, 48142, 48081, 48129, 48144, 47907,
];
const EVENT_WHITE_SHINY_ITEMS: &[i32] = &[
    1531, 38529, 38548, 1210, 3922, 51675, 7380, 7309, 3048, 7461, 37675, 3985, 7296, 5300, 5343,
    5345, 5344, 65531, // Kendo Stick Shiny
    7501,  // Hama Yumi Shiny
    3990,  // Greaterhosen Shiny
];

/// Group "Realm" items into: Crystal Prisoner UTs, Alien Gear, Jack Frost
/// UTs, Seasonal drops (also covers the Rat Minigame UTs), Event Whites,
/// Shiny Event Whites, ST Items, Biome Whites (catch-all for ordinary ambient
/// REALMWHITE drops), and Shiny Biome Whites.
fn group_by_realm_category(items: &[CollectionItem]) -> Vec<CollectionSection> {
    const CATEGORY_ORDER: [&str; 9] = [
        "Crystal Prisoner UTs",
        "Alien Gear",
        "Jack Frost UTs",
        "Seasonal drops",
        "ST Items",
        "Event Whites",
        "Shiny Event Whites",
        "Biome Whites",
        "Shiny Biome Whites",
    ];
    let mut buckets: HashMap<&'static str, Vec<CollectionItem>> = HashMap::new();

    for item in items {
        let category = if CRYSTAL_PRISONER_ITEMS.contains(&item.item_id) {
            "Crystal Prisoner UTs"
        } else if ALIEN_GEAR_ITEMS.contains(&item.item_id) {
            "Alien Gear"
        } else if JACK_FROST_ITEMS.contains(&item.item_id) {
            "Jack Frost UTs"
        } else if RAT_MINIGAME_ITEMS.contains(&item.item_id)
            || SEASONAL_UT_ITEMS.contains(&item.item_id)
        {
            "Seasonal drops"
        } else if item.group == ItemGroup::St {
            "ST Items"
        } else if EVENT_WHITE_ITEMS.contains(&item.item_id) {
            "Event Whites"
        } else if EVENT_WHITE_SHINY_ITEMS.contains(&item.item_id) {
            "Shiny Event Whites"
        } else if item.group == ItemGroup::ShinyUt {
            "Shiny Biome Whites"
        } else {
            "Biome Whites"
        };
        buckets.entry(category).or_default().push(item.clone());
    }

    CATEGORY_ORDER
        .iter()
        .filter_map(|&name| {
            buckets.remove(name).map(|items| CollectionSection {
                name: name.to_string(),
                items,
            })
        })
        .collect()
}

/// Group items by equipment slot type for Stromwell's Rift: Weapons, Armors
/// and Rings are united into one "Equipment" category (Abilities stays on
/// its own) to keep the section count down.
fn group_by_slot_type(items: &[CollectionItem]) -> Vec<CollectionSection> {
    const ABILITIES_SLOT: u8 = 1;
    let mut equipment: Vec<CollectionItem> = Vec::new();
    let mut abilities: Vec<CollectionItem> = Vec::new();

    for item in items {
        if item.slot_order > 3 {
            continue;
        }
        if item.slot_order == ABILITIES_SLOT {
            abilities.push(item.clone());
        } else {
            equipment.push(item.clone());
        }
    }

    [("Equipment", equipment), ("Abilities", abilities)]
        .into_iter()
        .filter(|(_, items)| !items.is_empty())
        .map(|(name, items)| CollectionSection {
            name: name.to_string(),
            items,
        })
        .collect()
}

/// Split a collection into "Dungeon Drops" and "Forge Upgrades" sections,
/// where Forge Upgrades holds the dungeon's `EXTRA_ITEMS_OVERRIDE` entries
/// (forge-crafted items that don't drop directly) and everything else is a
/// normal dungeon drop.
fn group_by_forge_upgrade(dungeon_name: &str, items: &[CollectionItem]) -> Vec<CollectionSection> {
    let forge_ids = EXTRA_ITEMS_OVERRIDE
        .iter()
        .find(|(name, _)| *name == dungeon_name)
        .map(|(_, ids)| *ids)
        .unwrap_or(&[]);

    let (forge_items, drop_items): (Vec<CollectionItem>, Vec<CollectionItem>) = items
        .iter()
        .cloned()
        .partition(|item| forge_ids.contains(&item.item_id));

    let mut sections = Vec::new();
    if !drop_items.is_empty() {
        sections.push(CollectionSection {
            name: "Dungeon Drops".to_string(),
            items: drop_items,
        });
    }
    if !forge_items.is_empty() {
        sections.push(CollectionSection {
            name: "Forge Upgrades".to_string(),
            items: forge_items,
        });
    }
    sections
}

/// Extract the ST set key from an item's internal id_name.
///
/// ST items in a set share a common prefix: e.g. `TricksterST0`, `TricksterST1`,
/// `TricksterST2`, `TricksterST3`. Gen 2/3 use `3TricksterST0` etc.
/// We strip the trailing digit to get the set key.
fn st_set_key_from_id_name(id_name: &str) -> String {
    // Strip trailing digit(s) that represent slot position
    let trimmed = id_name.trim_end_matches(|c: char| c.is_ascii_digit());
    trimmed.to_string()
}

/// Determine slot ordering from item labels.
/// weapon=0, ability=1, armor=2, ring/accessory=3
fn slot_order_from_labels(obj: &ObjectAsset) -> u8 {
    if obj.labels.contains("ST_WEAPON") || obj.labels.contains("WEAPON") {
        0
    } else if obj.labels.contains("ST_ABILITY") || obj.labels.contains("ABILITY") {
        1
    } else if obj.labels.contains("ST_ARMOR") || obj.labels.contains("ARMOR") {
        2
    } else if obj.labels.contains("ST_RING") || obj.labels.contains("RING") {
        3
    } else {
        4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn st_set_key_strips_trailing_digits() {
        assert_eq!(st_set_key_from_id_name("TricksterST0"), "TricksterST");
        assert_eq!(st_set_key_from_id_name("TricksterST3"), "TricksterST");
        assert_eq!(st_set_key_from_id_name("3TricksterST0"), "3TricksterST");
        assert_eq!(st_set_key_from_id_name("PaladinST2"), "PaladinST");
    }

    #[test]
    fn slot_order_weapon_first_ring_last() {
        let mut obj = ObjectAsset {
            id: 0,
            id_name: String::new(),
            display_name: String::new(),
            class: "Equipment".to_string(),
            group: String::new(),
            labels: "EQUIPMENT,WEAPON,DAGGER,ST,TAB_ST,ST_WEAPON,STGEN_1".to_string(),
            textures: Vec::new(),
            projectiles: Vec::new(),
            mask: None,
            tex1: 0,
            tex2: 0,
            slot_type: 1,
            defense: 0,
            hitbox_scale: 1.0,
            tier: None,
            feed_power: 0,
            fame_bonus: 0,
            bag_type: 0,
            set_name: None,
            lethal_strike: None,
            collection_icon: None,
            rate_of_fire: 1.0,
            num_projectiles: 1,
            stat_bonuses: Default::default(),
            weapon_procs: Vec::new(),
        };
        assert_eq!(slot_order_from_labels(&obj), 0);

        obj.labels = "EQUIPMENT,ABILITY,PRISM,ST,TAB_ST,ST_ABILITY,STGEN_1".to_string();
        assert_eq!(slot_order_from_labels(&obj), 1);

        obj.labels = "EQUIPMENT,ARMOR,LEATHER,ST,TAB_ST,ST_ARMOR,STGEN_1".to_string();
        assert_eq!(slot_order_from_labels(&obj), 2);

        obj.labels = "EQUIPMENT,RING,ST,TAB_ST,ST_RING,STGEN_1".to_string();
        assert_eq!(slot_order_from_labels(&obj), 3);
    }

    #[test]
    #[ignore] // requires assets to be loaded
    fn abyss_collection_has_items() {
        let col = build_collection_for_dungeon("Abyss of Demons");
        assert!(!col.items.is_empty(), "Abyss should have collectible items");

        // All items should be UT or ST
        for item in &col.items {
            assert!(
                matches!(
                    item.group,
                    ItemGroup::Ut | ItemGroup::ShinyUt | ItemGroup::St
                ),
                "unexpected group for {}",
                item.name
            );
        }
    }

    #[test]
    fn realm_categorizes_event_whites_and_shiny_biome_whites() {
        let mk = |id: i32, group: ItemGroup| CollectionItem {
            item_id: id,
            name: format!("item{id}"),
            group,
            st_set_key: None,
            set_name: None,
            slot_order: 0,
        };
        let items = vec![
            mk(47999, ItemGroup::Ut),      // Shifting Shroud -> Event Whites
            mk(53212, ItemGroup::ShinyUt), // Adventurer's Scarf Shiny -> Shiny Biome Whites
            mk(999999, ItemGroup::Ut),     // uncategorized -> Biome Whites
            mk(1210, ItemGroup::ShinyUt),  // Dirk of Cronus Shiny -> Shiny Event Whites
        ];
        let sections = group_by_realm_category(&items);
        let find = |name: &str| sections.iter().find(|s| s.name == name);

        let event = find("Event Whites").expect("Event Whites section");
        assert!(event.items.iter().any(|i| i.item_id == 47999));

        let shiny_biome = find("Shiny Biome Whites").expect("Shiny Biome Whites section");
        assert!(shiny_biome.items.iter().any(|i| i.item_id == 53212));

        let shiny_event = find("Shiny Event Whites").expect("Shiny Event Whites section");
        assert!(shiny_event.items.iter().any(|i| i.item_id == 1210));

        let biome = find("Biome Whites").expect("Biome Whites section");
        assert!(biome.items.iter().any(|i| i.item_id == 999999));
    }

    #[test]
    fn forgotten_legacy_event_whites_are_realm_members() {
        let realm_extra = EXTRA_ITEMS_OVERRIDE
            .iter()
            .find(|(name, _)| *name == "Realm")
            .map(|(_, ids)| *ids)
            .expect("Realm EXTRA override");
        for id in [
            47999, 47955, 48003, 48143, 47998, 47947, 48139, 47977, 48142, 48081, 48129, 48144,
            47907,
        ] {
            assert!(
                realm_extra.contains(&id),
                "event white {id} missing from Realm members"
            );
            assert!(
                EVENT_WHITE_ITEMS.contains(&id),
                "event white {id} missing from EVENT_WHITE_ITEMS"
            );
        }
        for id in 53212..=53224 {
            assert!(
                realm_extra.contains(&id),
                "shiny biome white {id} missing from Realm members"
            );
        }
    }
}

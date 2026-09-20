//! Boss-group classification for Combat History settings and filters.
//!
//! Every recorded fight is classified into at most one [`BossGroup`] so the user
//! can gate which groups are tracked (at capture time) and filter the history
//! list by group. Classification is derived from the fight's dungeon name (via
//! the hardcoded difficulty table) and the boss object's catalog labels, so all
//! phases of one dungeon run classify identically (they share the dungeon and
//! thus the same difficulty band).

use super::dungeon_data::dungeon_difficulty;
use super::get_dungeon_portal_map;
use super::manager::get_asset_manager;

/// A curated group a recorded boss fight belongs to. Mirrors the checkboxes in
/// the Combat History settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BossGroup {
    /// Bosses inside 7+ grave-difficulty dungeons.
    Exaltation,
    /// Bosses inside 5-6.5 grave-difficulty dungeons.
    Expert,
    /// Bosses inside 2.5-4.5 grave-difficulty dungeons.
    Adept,
    /// Bosses inside 0-2 grave-difficulty dungeons.
    Beginner,
    /// Bosses carrying the BEACON_GUARDIAN label (Heroes of Oryx beacons).
    BeaconGuardian,
    /// Rookie-tier Heroes of Oryx (curated realm boss list).
    RookieHero,
    /// Adept-tier Heroes of Oryx (curated realm boss list).
    AdeptHero,
    /// Veteran-tier Heroes of Oryx (curated realm boss list).
    VeteranHero,
    /// Realm Adept Encounters (ADEPT_ENCOUNTER label).
    AdeptEncounter,
    /// Realm Veteran Encounters (VETERAN_ENCOUNTER label).
    VeteranEncounter,
    /// Seasonal / special event bosses (curated name list + curated object-type list).
    SeasonalEncounter,
    /// Lootable treasure crates (curated object-type list).
    TreasureCrate,
}

impl BossGroup {
    /// Stable string form persisted in the `boss_group` column and used as a
    /// filter key. Never change these once shipped.
    pub fn as_str(&self) -> &'static str {
        match self {
            BossGroup::Exaltation => "exaltation",
            BossGroup::Expert => "expert",
            BossGroup::Adept => "adept",
            BossGroup::Beginner => "beginner",
            BossGroup::BeaconGuardian => "beacon_guardian",
            BossGroup::RookieHero => "rookie_hero",
            BossGroup::AdeptHero => "adept_hero",
            BossGroup::VeteranHero => "veteran_hero",
            BossGroup::AdeptEncounter => "adept_encounter",
            BossGroup::VeteranEncounter => "veteran_encounter",
            BossGroup::SeasonalEncounter => "seasonal_encounter",
            BossGroup::TreasureCrate => "treasure_crate",
        }
    }

    /// Parse from the persisted string form.
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "exaltation" => BossGroup::Exaltation,
            "expert" => BossGroup::Expert,
            "adept" => BossGroup::Adept,
            "beginner" => BossGroup::Beginner,
            "beacon_guardian" => BossGroup::BeaconGuardian,
            "rookie_hero" => BossGroup::RookieHero,
            "adept_hero" => BossGroup::AdeptHero,
            "veteran_hero" => BossGroup::VeteranHero,
            "adept_encounter" => BossGroup::AdeptEncounter,
            "veteran_encounter" => BossGroup::VeteranEncounter,
            "seasonal_encounter" => BossGroup::SeasonalEncounter,
            "treasure_crate" => BossGroup::TreasureCrate,
            _ => return None,
        })
    }

    /// Every group, in display order (used to seed the settings/filter UI).
    pub const ALL: [BossGroup; 12] = [
        BossGroup::Exaltation,
        BossGroup::Expert,
        BossGroup::Adept,
        BossGroup::Beginner,
        BossGroup::BeaconGuardian,
        BossGroup::RookieHero,
        BossGroup::AdeptHero,
        BossGroup::VeteranHero,
        BossGroup::AdeptEncounter,
        BossGroup::VeteranEncounter,
        BossGroup::SeasonalEncounter,
        BossGroup::TreasureCrate,
    ];

    /// Short human-readable label for the settings/filter UI.
    pub fn label(&self) -> &'static str {
        match self {
            BossGroup::Exaltation => "Exaltation dungeon bosses",
            BossGroup::Expert => "Expert dungeon bosses",
            BossGroup::Adept => "Adept dungeon bosses",
            BossGroup::Beginner => "Rookie dungeon bosses",
            BossGroup::BeaconGuardian => "Beacon guardians",
            BossGroup::RookieHero => "Rookie Heroes Of Oryx",
            BossGroup::AdeptHero => "Adept Heroes Of Oryx",
            BossGroup::VeteranHero => "Veteran Heroes Of Oryx",
            BossGroup::AdeptEncounter => "Adept Encounters",
            BossGroup::VeteranEncounter => "Veteran Encounters",
            BossGroup::SeasonalEncounter => "Seasonal and Special bosses",
            BossGroup::TreasureCrate => "Treasure crates",
        }
    }

    /// Object type id of a representative boss whose sprite stands in for the
    /// group on the filter buttons.
    pub fn sprite_id(&self) -> i32 {
        match self {
            BossGroup::Exaltation => 4243,         // Killer Bee Queen
            BossGroup::Expert => 1902,             // Thessal the Mermaid Goddess
            BossGroup::Adept => 2314,              // Archdemon Malphas
            BossGroup::Beginner => 3523,           // Mixcoatl the Masked God
            BossGroup::BeaconGuardian => 53002,    // Legion Fey Guardian
            BossGroup::RookieHero => 2335,         // Ent Ancient
            BossGroup::AdeptHero => 34570,         // Artificial Sprite God
            BossGroup::VeteranHero => 34509,       // Organ Harvester
            BossGroup::AdeptEncounter => 3417,     // Cube God
            BossGroup::VeteranEncounter => 22146,  // Lost Sentry
            BossGroup::SeasonalEncounter => 19246, // The Keyper
            BossGroup::TreasureCrate => 620,       // Bilgewater's Booty
        }
    }

    /// Every boss belonging to this group for the filter tooltip,
    /// deduplicated by name and sorted.
    ///
    /// The four dungeon-difficulty groups come from the hand-authored
    /// [`DUNGEON_BOSSES`] catalog (one main boss per dungeon). The five realm
    /// groups are derived at load time from the object catalog: beacon /
    /// miniboss / encounter groups from their catalog labels, seasonal from the
    /// curated name fragments -- so they stay complete and self-updating.
    pub fn catalog_bosses(&self) -> Vec<CatalogEntry> {
        match self {
            BossGroup::Exaltation | BossGroup::Expert | BossGroup::Adept | BossGroup::Beginner => {
                // Order hardest dungeon first, then by name. Sorting by
                // (difficulty desc, name) before deduping keeps the highest-rated
                // instance of a boss shared by several dungeons.
                let portal_map = get_dungeon_portal_map();
                let mut rows: Vec<(f32, i32, String, Option<i32>)> = DUNGEON_BOSSES
                    .iter()
                    .filter(|(g, _, _, _, _)| g == self)
                    .map(|(_, diff, id, name, dungeon)| {
                        (
                            *diff,
                            *id,
                            name.to_string(),
                            portal_map.get_portal_id(dungeon),
                        )
                    })
                    .collect();
                rows.sort_by(|a, b| {
                    b.0.partial_cmp(&a.0)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then(a.2.to_lowercase().cmp(&b.2.to_lowercase()))
                });
                let mut seen = std::collections::HashSet::new();
                rows.retain(|(_, _, name, _)| seen.insert(name.to_lowercase()));
                rows.into_iter()
                    .map(|(_, id, name, portal)| {
                        // Dungeon portal first, boss sprite last so the boss
                        // sits right next to its name.
                        let mut sprite_ids = Vec::new();
                        if let Some(p) = portal {
                            sprite_ids.push(p);
                        }
                        sprite_ids.push(id);
                        CatalogEntry { sprite_ids, name }
                    })
                    .collect()
            }
            // Realm groups are not difficulty-rated; the manager already returns
            // them deduped and name-sorted.
            BossGroup::BeaconGuardian => {
                single_entries(get_asset_manager().objects_with_label("BEACON_GUARDIAN"))
            }
            BossGroup::RookieHero | BossGroup::AdeptHero | BossGroup::VeteranHero => {
                let mut entries: Vec<CatalogEntry> = HERO_BOSSES
                    .iter()
                    .filter(|(g, _, _)| g == self)
                    .map(|(_, id, name)| CatalogEntry::single(*id, name.to_string()))
                    .collect();
                entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                entries
            }
            BossGroup::AdeptEncounter => {
                curate_adept_encounters(get_asset_manager().objects_with_label("ADEPT_ENCOUNTER"))
            }
            BossGroup::VeteranEncounter => curate_veteran_encounters(
                get_asset_manager().objects_with_label("VETERAN_ENCOUNTER"),
            ),
            BossGroup::SeasonalEncounter => {
                // Name-fragment seasonal bosses plus the curated special-boss
                // object types, with Prismimic / Dimitus / Wanderer
                // collapsed into a single representative row each.
                let mut entries = single_entries(
                    get_asset_manager().objects_with_name_fragments(SEASONAL_BOSS_NAME_FRAGMENTS),
                );
                for &(id, name) in SPECIAL_BOSS_CATALOG {
                    if !entries.iter().any(|e| e.sprite_ids.contains(&id)) {
                        entries.push(CatalogEntry::single(id, name.to_string()));
                    }
                }
                entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                entries
            }
            BossGroup::TreasureCrate => {
                let mut entries: Vec<CatalogEntry> = TREASURE_CRATE_CATALOG
                    .iter()
                    .map(|&(id, name)| CatalogEntry::single(id, name.to_string()))
                    .collect();
                entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                entries
            }
        }
    }
}

/// In-game object type of the Kogbold Expedition Engine locomotive (`0x3987`,
/// id_name "Train Encounter"). The realm train is spawned, tracked and labeled
/// under several segment object types whose sprites are mid-train cars, so
/// tooltips normalize the train's display sprite to this front locomotive.
pub const KOGBOLD_TRAIN_LOCOMOTIVE_SPRITE: i32 = 14727;

/// Display name shared by every Kogbold Expedition Engine train segment.
pub const KOGBOLD_TRAIN_DISPLAY_NAME: &str = "Kogbold Expedition Engine";

/// Normalize a display sprite so the Kogbold Expedition Engine always shows its
/// front locomotive rather than a tracked mid-train car segment. Returns `id`
/// unchanged for any other enemy. Callers keep the original object type for
/// matching/classification and only remap the drawn sprite.
pub fn normalize_train_sprite(id: i32, name: &str) -> i32 {
    if name.eq_ignore_ascii_case(KOGBOLD_TRAIN_DISPLAY_NAME) {
        KOGBOLD_TRAIN_LOCOMOTIVE_SPRITE
    } else {
        id
    }
}

/// One row in a boss-group filter tooltip: a display name plus the sprite(s) to
/// draw beside it. Most rows carry a single sprite; a few curated rows fold
/// several bosses into one entry (e.g. the Mountain Temple's Jade and Garnet
/// Statues) and so carry multiple sprite ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub sprite_ids: Vec<i32>,
    pub name: String,
}

impl CatalogEntry {
    fn single(id: i32, name: String) -> Self {
        Self {
            sprite_ids: vec![id],
            name,
        }
    }
}

/// Convert a raw `(id, name)` label list into one-sprite catalog rows.
fn single_entries(raw: Vec<(i32, String)>) -> Vec<CatalogEntry> {
    raw.into_iter()
        .map(|(id, name)| CatalogEntry::single(id, name))
        .collect()
}

/// Apply curated fixes to the raw ADEPT_ENCOUNTER catalog: rename the "New Beach
/// Bum" marker, and fold the Mountain Temple's invisible "New Temple Encounter"
/// marker into a single "Jade and Garnet Statues" row that draws both statue
/// sprites (the standalone statue rows are hidden here to avoid duplication).
fn curate_adept_encounters(raw: Vec<(i32, String)>) -> Vec<CatalogEntry> {
    const BEACH_BUM: i32 = 22034;
    const TEMPLE_ENCOUNTER: i32 = 22148;
    const JADE_STATUE: i32 = 22149;
    const GARNET_STATUE: i32 = 22150;

    let mut entries: Vec<CatalogEntry> = raw
        .into_iter()
        // Beach Bum is a biome miniboss, not an encounter; the statues are
        // surfaced via the merged Temple Encounter row below.
        .filter(|(id, _)| *id != BEACH_BUM && *id != JADE_STATUE && *id != GARNET_STATUE)
        .map(|(id, name)| match id {
            TEMPLE_ENCOUNTER => CatalogEntry {
                sprite_ids: vec![JADE_STATUE, GARNET_STATUE],
                name: "Jade and Garnet Statues".to_string(),
            },
            _ => CatalogEntry::single(id, name),
        })
        .collect();
    // These carry no ADEPT_ENCOUNTER label, so add them explicitly.
    const ADDITIONS: &[(i32, &str)] = &[
        (18007, "Mammoth Rat"),
        (34559, "Carp Emperor"),
        (22042, "Ghost Ship"),
        // Beer God (VETERAN_ENCOUNTER + MINION) counts as both an Adept and a
        // Veteran encounter: it spawns in Shipwreck Cove (adept) and Runic
        // Tundra (veteran). The generic label scan drops it (MINION), so add it
        // to both curated catalogs.
        (22128, "Beer God"),
        // Crystal Prisoner's realm setpiece announces "Sweet Treasure awaits for
        // powerful adventurers". Its boss is categorized as an Adept Hero of
        // Oryx in Combat History, but for tracking it belongs in the Adept
        // Encounter selector. Carries the ADEPT label (not ADEPT_ENCOUNTER).
        (2471, "Mysterious Crystal"),
    ];
    for &(id, name) in ADDITIONS {
        if !entries
            .iter()
            .any(|e| e.sprite_ids.contains(&id) || e.name.eq_ignore_ascii_case(name))
        {
            entries.push(CatalogEntry::single(id, name.to_string()));
        }
    }
    // Re-sort by the (possibly overridden) display name.
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    entries
}

/// Add Beer God to the raw VETERAN_ENCOUNTER catalog. It carries the
/// VETERAN_ENCOUNTER label but is also tagged MINION, so the generic label scan
/// (which drops minions) misses it; add it back explicitly.
fn curate_veteran_encounters(raw: Vec<(i32, String)>) -> Vec<CatalogEntry> {
    let mut entries = single_entries(raw);
    // These carry no usable VETERAN_ENCOUNTER label in the generic scan (Beer
    // God and Kogbold Expedition Engine are MINION-tagged so the scan drops
    // them; Lost Sentry is unlabeled; the rest are curated type overrides), so
    // add them explicitly. Killer Bee Nest stands in for the whole hive +
    // Beehemoths card.
    const ADDITIONS: &[(i32, &str)] = &[
        (22128, "Beer God"),
        (4312, "Killer Bee Nest"),
        (22146, "Lost Sentry"),
        (22109, "Kogbold Expedition Engine"),
    ];
    for &(id, name) in ADDITIONS {
        if !entries
            .iter()
            .any(|e| e.sprite_ids.contains(&id) || e.name.eq_ignore_ascii_case(name))
        {
            entries.push(CatalogEntry::single(id, name.to_string()));
        }
    }
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    entries
}

/// Curated Heroes of Oryx catalog, splitting the former Biome
/// Minibosses group into three difficulty tiers. Each tuple is
/// `(tier, object id, display name)`. These realm bosses do not all share a
/// single label (many carry HERO / QUEST / ENCOUNTER, not MINIBOSS), so the
/// list is hand-authored and also drives classification via [`hero_tier`].
/// Object ids resolved from the game's `ObjectID.list`.
static HERO_BOSSES: &[(BossGroup, i32, &str)] = &[
    // --- Rookie Heroes Of Oryx ---
    (BossGroup::RookieHero, 44158, "Beached Buccaneer"),
    (BossGroup::RookieHero, 2331, "Lich"),
    (BossGroup::RookieHero, 2335, "Ent Ancient"),
    (BossGroup::RookieHero, 3522, "Great Coil Snake"),
    (BossGroup::RookieHero, 1656, "Oasis Giant"),
    (BossGroup::RookieHero, 1653, "Phoenix Lord"),
    (BossGroup::RookieHero, 21901, "New Phoenix Lord"),
    (BossGroup::RookieHero, 21904, "New Phoenix Reborn"),
    (BossGroup::RookieHero, 21946, "New Great Coil Snake"),
    (BossGroup::RookieHero, 2344, "Ghost King"),
    // --- Adept Heroes Of Oryx ---
    (BossGroup::AdeptHero, 34508, "Maiden of the Sea"),
    (BossGroup::AdeptHero, 22034, "Beach Bum"),
    (BossGroup::AdeptHero, 34570, "Artificial Sprite God"),
    (BossGroup::AdeptHero, 44151, "Celestial Sprite"),
    (BossGroup::AdeptHero, 44153, "Cosmic Sprite"),
    (BossGroup::AdeptHero, 2907, "Elder Sprite Tree"),
    (BossGroup::AdeptHero, 24012, "Alpha Werewolf"),
    (BossGroup::AdeptHero, 34515, "Flesh Golem"),
    (BossGroup::AdeptHero, 34600, "Sinister Scarecrow"),
    (BossGroup::AdeptHero, 24023, "Animal Merchant"),
    (BossGroup::AdeptHero, 34499, "Washed-Up Captain"),
    (BossGroup::AdeptHero, 44164, "Death Knight"),
    (BossGroup::AdeptHero, 21943, "Kage Kami"),
    (BossGroup::AdeptHero, 17004, "Lantern Holder"),
    (BossGroup::AdeptHero, 24015, "Shady Sect Leader"),
    (BossGroup::AdeptHero, 34551, "Demonic Effigy"),
    (BossGroup::AdeptHero, 34573, "Eternal Tormentor"),
    (BossGroup::AdeptHero, 34476, "Infernal Ironsmith"),
    (BossGroup::AdeptHero, 1640, "Red Demon"),
    (BossGroup::AdeptHero, 34599, "Dragon's Treasure"),
    (BossGroup::AdeptHero, 34598, "Slumbering Dragon"),
    (BossGroup::AdeptHero, 16995, "Insurgent Rebel Commander"),
    (BossGroup::AdeptHero, 34553, "Sword in the Stone?"),
    (BossGroup::AdeptHero, 2369, "Crystal Prisoner"),
    (BossGroup::AdeptHero, 56341, "Alien Reactor"),
    // --- Veteran Heroes Of Oryx ---
    (BossGroup::VeteranHero, 1033, "Sea Dragon"),
    (BossGroup::VeteranHero, 34519, "Sunken Treasure"),
    (BossGroup::VeteranHero, 34481, "Critter Brood"),
    (BossGroup::VeteranHero, 34509, "Organ Harvester"),
    (BossGroup::VeteranHero, 1986, "Scout Colony"),
    (BossGroup::VeteranHero, 34480, "Assembled Giant"),
    (BossGroup::VeteranHero, 34518, "Monstrous Grizzly"),
    (BossGroup::VeteranHero, 16999, "Alluring Blossom"),
    (BossGroup::VeteranHero, 51055, "Elder Ent Ancient"),
    (BossGroup::VeteranHero, 34603, "Hornet's Nest"),
    (BossGroup::VeteranHero, 56342, "Alien Reactor"),
];

/// Classify a recorded boss into its Heroes of Oryx tier by object id, falling
/// back to an exact (case-insensitive) name match so id variants of the same
/// boss (e.g. the several "Lich" objects) still resolve. Returns `None` when the
/// boss is not a curated hero. Id matches take priority over the name fallback,
/// so bosses that share a display name across tiers (Alien Reactor's Adept and
/// Veteran objects) each resolve to their own tier.
fn hero_tier(boss_object_type: i32, boss_name: &str) -> Option<BossGroup> {
    HERO_BOSSES
        .iter()
        .find(|(_, id, _)| *id == boss_object_type)
        .or_else(|| {
            HERO_BOSSES
                .iter()
                .find(|(_, _, name)| name.eq_ignore_ascii_case(boss_name))
        })
        .map(|(g, _, _)| *g)
}

/// Hand-authored main boss for every rated dungeon: a tuple of
/// `(group, dungeon difficulty, object id, display name, dungeon name)`. One
/// "main" boss per dungeon (extra / co-bosses omitted). Object ids resolved from
/// the game's `ObjectID.list`; the dungeon name resolves each boss's portal icon
/// for the tooltip. Used to list every boss in a difficulty group in the filter
/// tooltip, ordered hardest-first, regardless of whether the player recorded it.
/// A boss shared by several dungeons carries the highest of their difficulties
/// (duplicates collapse at query time).
static DUNGEON_BOSSES: &[(BossGroup, f32, i32, &str, &str)] = &[
    // --- Exaltation (difficulty >= 7.0) ---
    (
        BossGroup::Exaltation,
        9.0,
        50391,
        "Factory Control Core",
        "Kogbold Steamworks",
    ),
    (
        BossGroup::Exaltation,
        7.5,
        10025,
        "Crystal Entity",
        "Crystal Cavern",
    ),
    (
        BossGroup::Exaltation,
        7.0,
        45231,
        "Malus",
        "Cultist Hideout",
    ),
    (
        BossGroup::Exaltation,
        7.5,
        45712,
        "Crystal Worm Mother",
        "Fungal Cavern",
    ),
    (
        BossGroup::Exaltation,
        7.0,
        14474,
        "The Wanderer",
        "Hidden Interregnum",
    ),
    (
        BossGroup::Exaltation,
        7.0,
        40166,
        "Esben the Neurotic",
        "Ice Citadel",
    ),
    (
        BossGroup::Exaltation,
        8.0,
        45073,
        "Marble Colossus",
        "Lost Halls",
    ),
    (
        BossGroup::Exaltation,
        9.0,
        20493,
        "Kitsune Umi",
        "Moonlight Village",
    ),
    (BossGroup::Exaltation, 8.0, 56393, "Neo Acidus", "Neo Forax"),
    (
        BossGroup::Exaltation,
        8.0,
        56410,
        "Neo Golden Sphinx",
        "Neo Katalund",
    ),
    (
        BossGroup::Exaltation,
        8.0,
        56358,
        "Neo Suesogian",
        "Neo Malogia",
    ),
    (
        BossGroup::Exaltation,
        8.0,
        56375,
        "Neo Tarul",
        "Neo Untaris",
    ),
    (
        BossGroup::Exaltation,
        9.5,
        45363,
        "Oryx the Mad God 3",
        "Oryx's Sanctuary",
    ),
    (
        BossGroup::Exaltation,
        8.5,
        4243,
        "Killer Bee Queen",
        "The Nest",
    ),
    (
        BossGroup::Exaltation,
        8.0,
        23681,
        "Soulwarden Murcian",
        "Spectral Penitentiary",
    ),
    (
        BossGroup::Exaltation,
        10.0,
        15791,
        "Inspector Stromwell",
        "Stromwell's Rift",
    ),
    (
        BossGroup::Exaltation,
        10.0,
        29039,
        "The Forgotten King",
        "The Shatters",
    ),
    (BossGroup::Exaltation, 8.5, 45076, "Void Entity", "The Void"),
    (
        BossGroup::Exaltation,
        8.5,
        59922,
        "The White Snake",
        "White Snake Invasion",
    ),
    (
        BossGroup::Exaltation,
        7.5,
        59902,
        "Primal Snake",
        "White Snake Invasion",
    ),
    // --- Expert (5.0 - 6.9) ---
    (
        BossGroup::Expert,
        5.0,
        29070,
        "Oryx the Mad God Deux",
        "Battle for the Nexus",
    ),
    (
        BossGroup::Expert,
        5.5,
        29580,
        "Belladonna",
        "Belladonna's Garden",
    ),
    (
        BossGroup::Expert,
        5.5,
        2551,
        "Royal Cnidarian",
        "Cnidarian Reef",
    ),
    (
        BossGroup::Expert,
        5.0,
        3634,
        "Davy Jones",
        "Davy Jones' Locker",
    ),
    (
        BossGroup::Expert,
        5.5,
        2352,
        "Jon Bilgewater the Pirate King",
        "Deadwater Docks",
    ),
    (BossGroup::Expert, 5.0, 45827, "Acidus", "Forax"),
    (
        BossGroup::Expert,
        6.0,
        14694,
        "Septavius, Master of the Undead",
        "Heroic Undead Lair",
    ),
    (
        BossGroup::Expert,
        6.5,
        2458,
        "F.E.R.A.L.",
        "High Tech Terror",
    ),
    (BossGroup::Expert, 6.0, 32692, "Polaris", "Ice Tomb"),
    (
        BossGroup::Expert,
        6.0,
        14718,
        "Malphas, Gilded Forgemaster",
        "Abyss of Demons",
    ),
    (BossGroup::Expert, 5.0, 45849, "Golden Sphinx", "Katalund"),
    (
        BossGroup::Expert,
        6.0,
        45449,
        "Ivory Wyvern",
        "Lair of Draconis",
    ),
    (
        BossGroup::Expert,
        6.0,
        28058,
        "Shaitan the Advisor",
        "Lair of Shaitan",
    ),
    (BossGroup::Expert, 6.0, 1335, "Decaract", "Mad God Mayhem"),
    (BossGroup::Expert, 5.0, 45764, "Suesogian", "Malogia"),
    (
        BossGroup::Expert,
        6.0,
        43681,
        "Daichi the Fallen",
        "Mountain Temple",
    ),
    (
        BossGroup::Expert,
        5.0,
        1902,
        "Thessal the Mermaid Goddess",
        "Ocean Trench",
    ),
    (
        BossGroup::Expert,
        5.5,
        2048,
        "Nightmare Colony",
        "Parasite Chambers",
    ),
    (
        BossGroup::Expert,
        5.5,
        29793,
        "The Puppet Master",
        "Puppet Master's Encore",
    ),
    (
        BossGroup::Expert,
        5.5,
        1431,
        "Queen Alien Bunny",
        "Queen Bunny Chamber",
    ),
    (
        BossGroup::Expert,
        6.5,
        13990,
        "Tezcacoatl the Great Basilisk",
        "Secluded Thicket",
    ),
    (
        BossGroup::Expert,
        5.0,
        13095,
        "Inspector Stromwell",
        "Stromwell's Rift",
    ),
    (
        BossGroup::Expert,
        6.0,
        25598,
        "Heart of the Wetlands",
        "Sulfurous Wetlands",
    ),
    (
        BossGroup::Expert,
        5.5,
        28714,
        "Son of Arachna",
        "The Crawling Depths",
    ),
    (
        BossGroup::Expert,
        5.5,
        17746,
        "Bradley the Barkeep",
        "The Tavern",
    ),
    (
        BossGroup::Expert,
        6.0,
        44878,
        "Tesseract Goddess",
        "The Third Dimension",
    ),
    (
        BossGroup::Expert,
        6.5,
        22056,
        "Perfected Cube God",
        "The Trials of Cronus",
    ),
    (BossGroup::Expert, 6.0, 3367, "Geb", "Tomb of the Ancients"),
    (BossGroup::Expert, 5.0, 45811, "Tarul", "Untaris"),
    (
        BossGroup::Expert,
        6.5,
        59902,
        "Primal Snake",
        "White Snake Invasion",
    ),
    (
        BossGroup::Expert,
        6.0,
        2354,
        "Oryx the Mad God 2",
        "Wine Cellar",
    ),
    (
        BossGroup::Expert,
        5.5,
        28795,
        "Murderous Megamoth",
        "Woodland Labyrinth",
    ),
    // --- Adept (2.5 - 4.9) ---
    (
        BossGroup::Adept,
        4.0,
        2314,
        "Archdemon Malphas",
        "Abyss of Demons",
    ),
    (
        BossGroup::Adept,
        3.5,
        24129,
        "Cupcake",
        "Candyland Hunting Grounds",
    ),
    (
        BossGroup::Adept,
        2.5,
        24108,
        "Golden Oryx Effigy",
        "Cave of a Thousand Treasures",
    ),
    (
        BossGroup::Adept,
        4.0,
        43923,
        "Avalon the Archivist",
        "Cursed Library",
    ),
    (
        BossGroup::Adept,
        4.5,
        7099,
        "Ghost of Skuld",
        "Haunted Cemetery",
    ),
    (BossGroup::Adept, 4.0, 2422, "Dr Terrible", "Mad Lab"),
    (
        BossGroup::Adept,
        3.0,
        2096,
        "Fountain Spirit",
        "Magic Woods",
    ),
    (
        BossGroup::Adept,
        4.0,
        5920,
        "Lord Ruthven",
        "Manor of the Immortals",
    ),
    (
        BossGroup::Adept,
        4.5,
        3448,
        "Stone Guardians",
        "Oryx's Castle",
    ),
    (
        BossGroup::Adept,
        4.5,
        8200,
        "Janus the Doorwarden",
        "Oryx's Castle",
    ),
    (
        BossGroup::Adept,
        4.5,
        5952,
        "Oryx the Mad God 1",
        "Oryx's Chamber",
    ),
    (
        BossGroup::Adept,
        4.0,
        29747,
        "The Puppet Master",
        "Puppet Master's Theatre",
    ),
    (
        BossGroup::Adept,
        2.5,
        2327,
        "Stheno the Snake Queen",
        "Snake Pit",
    ),
    (
        BossGroup::Adept,
        2.5,
        3334,
        "Limon the Sprite God",
        "Sprite World",
    ),
    (BossGroup::Adept, 4.5, 43989, "Null", "The Machine"),
    (
        BossGroup::Adept,
        4.0,
        29013,
        "Gulpord the Slime God",
        "Toxic Sewers",
    ),
    (
        BossGroup::Adept,
        3.5,
        3472,
        "Septavius the Ghost God",
        "Undead Lair",
    ),
    // --- Beginner (< 2.5) ---
    (
        BossGroup::Beginner,
        1.5,
        3523,
        "Mixcoatl the Masked God",
        "Forbidden Jungle",
    ),
    (
        BossGroup::Beginner,
        1.0,
        24162,
        "Mama Megamoth",
        "Forest Maze",
    ),
    (
        BossGroup::Beginner,
        1.0,
        2343,
        "Dreadstump the Pirate King",
        "Pirate Cave",
    ),
    (
        BossGroup::Beginner,
        0.0,
        5709,
        "Pot of Gold",
        "Rainbow Road",
    ),
    (
        BossGroup::Beginner,
        0.0,
        46310,
        "Suspiciously Large Present",
        "Santa's Workshop",
    ),
    (
        BossGroup::Beginner,
        1.5,
        2358,
        "Arachna the Spider Queen",
        "Spider Den",
    ),
    (BossGroup::Beginner, 2.0, 297, "Queen Bee", "The Hive"),
    (
        BossGroup::Beginner,
        0.0,
        3670,
        "Masked Party God",
        "Beachzone",
    ),
];

/// Difficulty band for a dungeon rating, or `None` when the dungeon has no
/// hardcoded difficulty (realm / event maps).
fn difficulty_band(difficulty: f32) -> BossGroup {
    if difficulty >= 7.0 {
        BossGroup::Exaltation
    } else if difficulty >= 5.0 {
        BossGroup::Expert
    } else if difficulty >= 2.5 {
        BossGroup::Adept
    } else {
        BossGroup::Beginner
    }
}

/// Curated Daily Quest / Forge **mark** to canonical dungeon mapping. Marks are
/// named after their dungeon's boss (e.g. "Mark of Skuld" -> Haunted Cemetery);
/// a handful are named after the dungeon itself or a mini-boss. Keyed by the
/// full item display name so the resolver is an unambiguous exact lookup. Marks
/// whose dungeon is genuinely ambiguous (e.g. "Mark of Oryx", "Mark of the
/// Puppet Master") are intentionally omitted so no misleading drop tooltip is
/// shown. Dungeon names match the canonical portal names used elsewhere.
static MARK_TO_DUNGEON: &[(&str, &str)] = &[
    ("Advanced Mark of the Killer Bee Queen", "The Nest"),
    ("Mark of Arachna", "Spider Den"),
    ("Mark of Belladonna", "Belladonna's Garden"),
    ("Mark of Bilgewater", "Deadwater Docks"),
    ("Mark of Daichi", "Mountain Temple"),
    ("Mark of Davy Jones", "Davy Jones' Locker"),
    ("Mark of Dr Terrible", "Mad Lab"),
    ("Mark of Dreadstump", "Pirate Cave"),
    ("Mark of Esben", "Ice Citadel"),
    ("Mark of Geb", "Tomb of the Ancients"),
    ("Mark of Gulpord", "Toxic Sewers"),
    ("Mark of Janus", "Oryx's Castle"),
    ("Mark of Limon", "Sprite World"),
    ("Mark of Malphas", "Abyss of Demons"),
    ("Mark of Malus", "Cultist Hideout"),
    ("Mark of Mama Megamoth", "Forest Maze"),
    ("Mark of Mixcoatl", "Forbidden Jungle"),
    ("Mark of Moonlight", "Moonlight Village"),
    ("Mark of Oryx", "Oryx's Chamber"),
    ("Mark of Parasitic Horrors", "Parasite Chambers"),
    ("Mark of Ruthven", "Manor of the Immortals"),
    ("Mark of Septavius", "Undead Lair"),
    ("Mark of Shaitan", "Lair of Shaitan"),
    ("Mark of Skuld", "Haunted Cemetery"),
    ("Mark of Stheno", "Snake Pit"),
    ("Mark of the Advanced Control Core", "Kogbold Steamworks"),
    ("Mark of the Archivist", "Cursed Library"),
    ("Mark of the Barkeep", "The Tavern"),
    ("Mark of the Control Core", "Kogbold Steamworks"),
    ("Mark of the Crystal Entity", "Crystal Cavern"),
    ("Mark of the Effigy", "Cave of a Thousand Treasures"),
    ("Mark of the Exalted God", "Oryx's Sanctuary"),
    ("Mark of the Forgotten King", "The Shatters"),
    ("Mark of the Fountain Spirit", "Magic Woods"),
    ("Mark of the Interregnum", "Hidden Interregnum"),
    ("Mark of the Killer Bee Queen", "The Nest"),
    ("Mark of the Kitsune", "Moonlight Village"),
    ("Mark of the Marble Colossus", "Lost Halls"),
    ("Mark of the Megamoth", "Woodland Labyrinth"),
    ("Mark of the Puppet Master", "Puppet Master's Theatre"),
    ("Mark of the Queen Bee", "The Hive"),
    ("Mark of the Sandstone Titan", "Ancient Ruins"),
    ("Mark of the Son of Arachna", "The Crawling Depths"),
    ("Mark of the Soulwarden", "Spectral Penitentiary"),
    ("Mark of the Tesseract Goddess", "The Third Dimension"),
    ("Mark of the Void Entity", "The Void"),
    ("Mark of the Wetlands", "Sulfurous Wetlands"),
    ("Mark of the Wyvern", "Lair of Draconis"),
    ("Mark of Thessal", "Ocean Trench"),
];

/// Resolve a Daily Quest / Forge mark item name to its canonical dungeon name,
/// or `None` when the mark is unknown or its dungeon is ambiguous. Used to attach
/// realm drop-source tooltips to mark-quest pills.
pub fn dungeon_for_mark_name(name: &str) -> Option<&'static str> {
    let n = name.trim();
    MARK_TO_DUNGEON
        .iter()
        .find(|(mark, _)| mark.eq_ignore_ascii_case(n))
        .map(|(_, dungeon)| *dungeon)
}

/// Realm encounters capped per realm (`PerRealmMax` in the game's
/// `encounters.xml`): they spawn only a limited number of times over a realm's
/// life, unlike ordinary encounters that respawn all realm long. Keyed by the
/// display names used in the drop model (`dungeon_drops.json`). Because the
/// authoritative `encounters.xml` object ids don't always match those display
/// names (e.g. the Kogbold Expedition Engine ships as "New Train Encounter
/// Spawner", the Killer Bee Nest as "New EH Event Hive Summoner", the Ghost Ship
/// as "Ghost Ship Whirlpool", and the Crystal Worm Father's cap sits on its
/// "Dwarf Miner" parent), this list is curated and human-verified against each
/// object's own `PerRealmMax` rather than derived automatically. Caps are per
/// object and are NOT inherited by seasonal reskins: e.g. Flying Behemoth is
/// capped but its Behemoth's Egg reskin is not, so reskins are excluded unless
/// individually confirmed.
static LIMITED_SPAWN_ENCOUNTERS: &[&str] = &[
    "Avatar of the Forgotten King",
    "Bilgewater's Galleon",
    "Commander Calbrik",
    "Crystal Worm Father",
    "Ethereal Shrine",
    "Flying Behemoth",
    "Ghost Ship",
    "Grand Sphinx",
    "Hermit God",
    "Killer Bee Nest",
    "Kogbold Expedition Engine",
    "Lord of the Lost Lands",
    "Lost Sentry",
    "Permafrost Lord",
    "Rock Dragon",
    "Sentient Monolith",
    "The Gardener",
];

/// True when `name` is a realm encounter with a per-realm spawn cap (see
/// [`LIMITED_SPAWN_ENCOUNTERS`]). Used by dungeon drop-location tooltips to flag
/// and de-prioritise one-off encounters against respawnable ones.
pub fn encounter_spawn_limited(name: &str) -> bool {
    let n = name.trim();
    LIMITED_SPAWN_ENCOUNTERS
        .iter()
        .any(|e| e.eq_ignore_ascii_case(n))
}

/// Exaltation-dungeon mini-bosses that must never earn the Combat History
/// "flawless" marker, keyed by object type (stable across name changes). All
/// Marble Defender variants share the same display name and are listed so the
/// exemption holds whichever one a fight is recorded under.
const FLAWLESS_EXEMPT_BOSS_TYPES: &[i32] = &[
    0xb010, // Agonized Titan (Lost Halls)
    0xb136, // Marble Defender F (Lost Halls)
    0xb13b, // Marble Defender
    0xb13d, // Marble Defender Activator E
    0xb13e, // Marble Defender Activator W
    0xb13f, // Marble Defender Activator N
    0xb140, // Marble Defender Activator S
    0xb14a, // Marble Defender S
    0xc4ad, // Kogbold Flying Machine (Kogbold Steamworks)
];

/// True when the given boss object type is an Exaltation mini-boss that is
/// exempt from the "flawless" marker (see [`FLAWLESS_EXEMPT_BOSS_TYPES`]).
pub fn boss_exempt_from_flawless(boss_object_type: i32) -> bool {
    FLAWLESS_EXEMPT_BOSS_TYPES.contains(&boss_object_type)
}

/// Case-insensitive substring fragments identifying the curated seasonal event
/// bosses. Matched against the resolved boss name so no
/// object-type table is required (names are stable and already persisted).
const SEASONAL_BOSS_NAME_FRAGMENTS: &[&str] = &[
    "keyper",
    "gardener",
    "appetizer",
    "biff",  // Biff the Buffed Bunny
    "snowy", // Snowy the Frost God
    "jack frost",
    "permafrost lord",
    "totalia",   // Totalia the Malevolent
    "bonegrind", // Bonegrind the Undead Butcher (Zombie Horde event boss)
    "zombie horde",
];

fn is_seasonal_boss_name(boss_name: &str) -> bool {
    let lower = boss_name.to_lowercase();
    SEASONAL_BOSS_NAME_FRAGMENTS
        .iter()
        .any(|frag| lower.contains(frag))
}

/// Seasonal/special bosses that spawn *inside dungeons* and are therefore
/// excluded from the "Seasonal and special bosses" realm-event notifications
/// (and their picker). Ids reference [`SPECIAL_BOSS_CATALOG`].
pub const SEASONAL_NOTIFY_EXCLUDED_IDS: &[i32] = &[
    8813,  // Dimitus
    49743, // Black Blade Ozuchi
    38284, // Crackjaw Gregg
    49718, // Izel the Grand Shaman
    49730, // Nefret the Pharaoh
    49708, // Pirate Queen Ramm
    38282, // Pitchfork Pete
    19247, // Prismimic
    38283, // Sticky Fudgefoot
    49755, // The Wanderer
];

/// Curated seasonal/special bosses shown in the "Seasonal and Special bosses"
/// filter tooltip. One representative sprite id per boss: multi-
/// variant bosses (Prismimic, Dimitus, The Wanderer, Key Fairy) collapse to a
/// single row so the catalog is not cluttered with difficulty duplicates.
const SPECIAL_BOSS_CATALOG: &[(i32, &str)] = &[
    (44020, "The Glitch"),
    (42371, "Jotunn"),
    (42255, "The Hemomancer"),
    (38282, "Pitchfork Pete"),
    (38283, "Sticky Fudgefoot"),
    (38284, "Crackjaw Gregg"),
    (19247, "Prismimic"),
    (19233, "Keyper Towers"),
    (8813, "Dimitus"),
    (49755, "The Wanderer"),
    (49708, "Pirate Queen Ramm"),
    (49718, "Izel the Grand Shaman"),
    (49730, "Nefret the Pharaoh"),
    (49743, "Black Blade Ozuchi"),
];

/// Curated treasure crates shown in the "Treasure crates" filter tooltip.
/// Egg / Bilgewater variants collapse to a single row each.
const TREASURE_CRATE_CATALOG: &[(i32, &str)] = &[
    (620, "Bilgewater's Booty"),
    (2078, "Infested Chest"),
    (28065, "Sunken Treasure"),
    (28671, "Old Chest"),
    (9110, "Woodland Skysplitter Stone"),
    (5893, "Coral Gift"),
    (33018, "Oryxmas Realm Present"),
    (2202, "Easter Egg"),
    (18376, "Key Fairy"),
    (45906, "Satellite Core"),
    (20789, "MV Fishing Loot"),
    (5943, "Manor Coffin"),
    (46404, "Master Rat Box"),
    (29023, "Golden Rat"),
    (28867, "Magic Lamp"),
];

fn has_label(boss_object_type: i32, tag: &str) -> bool {
    get_asset_manager()
        .object_labels(boss_object_type)
        .map(|labels| labels.split(',').any(|l| l == tag))
        .unwrap_or(false)
}

/// Curated object-type -> group overrides for bosses whose `ObjectID.list`
/// entry ships without the catalog label its group is normally derived from
/// (game data gaps), or realm-fought dungeon minibosses the difficulty table
/// cannot reach because the fight logs its dungeon as "Realm". Without these
/// they fall through to no group and get hidden by every Combat History filter.
/// Checked before the generic label scans. Keep grouped by category.
const TYPE_GROUP_OVERRIDES: &[(i32, BossGroup)] = &[
    // Realm encounters whose ObjectID entry ships with empty / missing
    // ADEPT_ENCOUNTER / VETERAN_ENCOUNTER labels carried by their siblings.
    (22042, BossGroup::AdeptEncounter), // New Ghost Ship (empty labels)
    // Killer Bee Nest realm event: the event Hive and its three Beehemoths are
    // Veteran Encounters (they carry only MINIBOSS/BOSSFIGHT, no tier label).
    (4312, BossGroup::VeteranEncounter), // Killer Bee Nest (event hive)
    (4324, BossGroup::VeteranEncounter), // Yellow Beehemoth
    (4325, BossGroup::VeteranEncounter), // Red Beehemoth
    (4326, BossGroup::VeteranEncounter), // Blue Beehemoth
    // Oryx's Castle minibosses fought from the realm (the fight logs its dungeon
    // as "Realm", so the difficulty table never classifies them). Adept dungeon
    // bosses, matching their Oryx's Castle difficulty band.
    (8200, BossGroup::Adept), // Janus the Doorwarden
    (3448, BossGroup::Adept), // Stone Guardian (right)
    (3449, BossGroup::Adept), // Stone Guardian (left)
    // Hermit God realm event: the tentacle add carries only the "New Hermit God
    // Encounter" group, not the ADEPT_ENCOUNTER label its boss (22023) has. The
    // aggregated tentacle fight uses the legacy repr type 3428, whose encounter
    // anchor (legacy 3425) is absent from current assets, so the encounter
    // fallback can't classify it and it leaks past the Adept Encounters filter.
    // Pin both tentacle types to Adept Encounter, matching the boss.
    (3428, BossGroup::AdeptEncounter), // Hermit God Tentacle (aggregated repr)
    (22026, BossGroup::AdeptEncounter), // New Hermit God Tentacle
];

/// Curated group for an object type whose catalog labels can't classify it, if
/// any (see [`TYPE_GROUP_OVERRIDES`]).
fn type_group_override(boss_object_type: i32) -> Option<BossGroup> {
    TYPE_GROUP_OVERRIDES
        .iter()
        .find(|(t, _)| *t == boss_object_type)
        .map(|(_, g)| *g)
}

/// Classify a recorded fight into its [`BossGroup`], or `None` when it belongs
/// to no curated group (always tracked, never gated).
///
/// Priority (first match wins):
/// 1. Seasonal boss name (so seasonal bosses in rated dungeons still count as
///    seasonal, e.g. Biff the Buffed Bunny in the Queen Bunny Chamber).
/// 2. Dungeon difficulty band (Exaltation / Expert / Adept / Beginner).
/// 3. Heroes of Oryx tier (curated realm boss list). Checked before
///    the label groups so e.g. Beach Bum (which also carries ADEPT_ENCOUNTER)
///    lands in its hero tier.
/// 4. BEACON_GUARDIAN label.
/// 5. ADEPT_ENCOUNTER / VETERAN_ENCOUNTER labels.
/// 6. Curated encounter fallback: a member add of a grouped realm encounter
/// inherits its encounter anchor's group, so member fights whose own
///    object type lacks the group label (e.g. Pentaract Towers) stay filterable.
pub fn boss_group(dungeon: &str, boss_object_type: i32, boss_name: &str) -> Option<BossGroup> {
    // Curated object-type groups win over everything else so a
    // crate or special boss classifies consistently regardless of the dungeon
    // or realm it spawns in.
    let assets = get_asset_manager();
    if assets.is_treasure_crate(boss_object_type) {
        return Some(BossGroup::TreasureCrate);
    }
    if assets.is_special_boss(boss_object_type) {
        return Some(BossGroup::SeasonalEncounter);
    }
    if is_seasonal_boss_name(boss_name) {
        return Some(BossGroup::SeasonalEncounter);
    }
    if let Some(difficulty) = dungeon_difficulty(dungeon) {
        return Some(difficulty_band(difficulty));
    }
    if let Some(tier) = hero_tier(boss_object_type, boss_name) {
        return Some(tier);
    }
    if let Some(group) = type_group_override(boss_object_type) {
        return Some(group);
    }
    if has_label(boss_object_type, "BEACON_GUARDIAN") {
        return Some(BossGroup::BeaconGuardian);
    }
    if has_label(boss_object_type, "ADEPT_ENCOUNTER") {
        return Some(BossGroup::AdeptEncounter);
    }
    if has_label(boss_object_type, "VETERAN_ENCOUNTER") {
        return Some(BossGroup::VeteranEncounter);
    }
    // Grouped realm encounters record member fights under an add / tower
    // object type that may lack the group label its encounter marker carries
    // (e.g. the Pentaract Towers vs. the invisible New Pentaract marker). Fall
    // back to the encounter anchor's classification so every member fight shares
    // its card's group and stays filterable.
    if let Some(enc) = super::encounter_for_boss_type(boss_object_type) {
        if enc.anchor_type != boss_object_type {
            if let Some(group) = anchor_group(enc.anchor_type, enc.display_name) {
                return Some(group);
            }
        }
    }
    None
}

/// Classify a curated encounter's anchor into its group by hero tier or catalog
/// label, without recursing into the encounter fallback (the anchor is a single
/// object, never another encounter member here).
fn anchor_group(anchor_type: i32, anchor_name: &str) -> Option<BossGroup> {
    if let Some(tier) = hero_tier(anchor_type, anchor_name) {
        return Some(tier);
    }
    if let Some(group) = type_group_override(anchor_type) {
        return Some(group);
    }
    if has_label(anchor_type, "BEACON_GUARDIAN") {
        return Some(BossGroup::BeaconGuardian);
    }
    if has_label(anchor_type, "ADEPT_ENCOUNTER") {
        return Some(BossGroup::AdeptEncounter);
    }
    if has_label(anchor_type, "VETERAN_ENCOUNTER") {
        return Some(BossGroup::VeteranEncounter);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_names_resolve_to_dungeons() {
        assert_eq!(
            dungeon_for_mark_name("Mark of Skuld"),
            Some("Haunted Cemetery")
        );
        assert_eq!(
            dungeon_for_mark_name("Mark of the Marble Colossus"),
            Some("Lost Halls")
        );
        assert_eq!(
            dungeon_for_mark_name("Mark of the Void Entity"),
            Some("The Void")
        );
        assert_eq!(
            dungeon_for_mark_name("mark of thessal"),
            Some("Ocean Trench")
        );
        assert_eq!(
            dungeon_for_mark_name("Mark of the Sandstone Titan"),
            Some("Ancient Ruins")
        );
        assert_eq!(
            dungeon_for_mark_name("Mark of the Puppet Master"),
            Some("Puppet Master's Theatre")
        );
        assert_eq!(
            dungeon_for_mark_name("Mark of Janus"),
            Some("Oryx's Castle")
        );
        assert_eq!(
            dungeon_for_mark_name("Mark of Oryx"),
            Some("Oryx's Chamber")
        );
        assert_eq!(dungeon_for_mark_name("Concentrated Soul Fire"), None);
    }

    #[test]
    fn flawless_exempt_bosses_are_flagged() {
        assert!(boss_exempt_from_flawless(0xb010)); // Agonized Titan
        assert!(boss_exempt_from_flawless(0xb136)); // Marble Defender F
        assert!(boss_exempt_from_flawless(0xb14a)); // Marble Defender S
        assert!(boss_exempt_from_flawless(0xc4ad)); // Kogbold Flying Machine
                                                    // Real Exaltation headline bosses stay eligible.
        assert!(!boss_exempt_from_flawless(45073)); // Marble Colossus
        assert!(!boss_exempt_from_flawless(50391)); // Factory Control Core
        assert!(!boss_exempt_from_flawless(45712)); // Crystal Worm Mother
    }

    #[test]
    fn limited_spawn_encounters_are_flagged() {
        assert!(encounter_spawn_limited("Kogbold Expedition Engine"));
        assert!(encounter_spawn_limited("kogbold expedition engine"));
        assert!(encounter_spawn_limited("Killer Bee Nest"));
        assert!(encounter_spawn_limited("Lord of the Lost Lands"));
        assert!(encounter_spawn_limited("Ghost Ship"));
        assert!(encounter_spawn_limited("Grand Sphinx"));
        assert!(encounter_spawn_limited("Crystal Worm Father"));
        assert!(encounter_spawn_limited("The Gardener"));
        // Respawnable: no per-realm cap on their own object (seasonal reskins do
        // not inherit their base encounter's cap).
        assert!(!encounter_spawn_limited("Aerial Warship"));
        assert!(!encounter_spawn_limited("Beer God"));
        assert!(!encounter_spawn_limited("Jade Statue"));
        assert!(!encounter_spawn_limited("Behemoth's Egg"));
        assert!(!encounter_spawn_limited("Kogbold Warrior"));
    }

    #[test]
    fn difficulty_bands_split_at_documented_boundaries() {
        assert_eq!(difficulty_band(9.5), BossGroup::Exaltation);
        assert_eq!(difficulty_band(7.0), BossGroup::Exaltation);
        assert_eq!(difficulty_band(6.5), BossGroup::Expert);
        assert_eq!(difficulty_band(5.0), BossGroup::Expert);
        assert_eq!(difficulty_band(4.5), BossGroup::Adept);
        assert_eq!(difficulty_band(2.5), BossGroup::Adept);
        assert_eq!(difficulty_band(2.0), BossGroup::Beginner);
        assert_eq!(difficulty_band(0.0), BossGroup::Beginner);
    }

    #[test]
    fn known_dungeons_classify_by_band() {
        // The Shatters (10.0) -> Exaltation; Undead Lair (3.5) -> Adept;
        // The Hive (2.0) -> Beginner. Boss type/name irrelevant for rated maps.
        assert_eq!(
            boss_group("The Shatters", 0, ""),
            Some(BossGroup::Exaltation)
        );
        assert_eq!(boss_group("Undead Lair", 0, ""), Some(BossGroup::Adept));
        assert_eq!(boss_group("The Hive", 0, ""), Some(BossGroup::Beginner));
        assert_eq!(boss_group("Ocean Trench", 0, ""), Some(BossGroup::Expert));
    }

    #[test]
    fn seasonal_name_wins_over_dungeon_band() {
        // Biff in a rated dungeon still classifies as seasonal.
        assert_eq!(
            boss_group("Queen Bunny Chamber", 0, "Biff the Buffed Bunny"),
            Some(BossGroup::SeasonalEncounter)
        );
        assert_eq!(
            boss_group("Realm of the Mad God", 0, "Jack Frost"),
            Some(BossGroup::SeasonalEncounter)
        );
    }

    #[test]
    fn unrated_map_without_labels_is_ungated() {
        assert_eq!(boss_group("Realm of the Mad God", 0, "Realm Critter"), None);
    }

    #[test]
    fn ghost_ship_override_classifies_as_adept_encounter() {
        // New Ghost Ship (22042) ships with empty catalog labels, so without the
        // curated override it would fall through to no group and be hidden by
        // every filter. It must classify as an Adept Encounter like its siblings.
        assert_eq!(type_group_override(22042), Some(BossGroup::AdeptEncounter));
        assert_eq!(
            boss_group("Realm", 22042, "Ghost Ship"),
            Some(BossGroup::AdeptEncounter)
        );
    }

    #[test]
    fn alien_reactors_classify_as_hero_tiers() {
        // Alien Reactor Adept/Veteran carry only ENEMY,CONSTRUCT,QUEST and are
        // curated as Adept/Veteran Heroes of Oryx. They share the display name
        // "Alien Reactor", so id-priority in hero_tier keeps each in its tier.
        assert_eq!(
            boss_group("Realm", 56341, "Alien Reactor"),
            Some(BossGroup::AdeptHero)
        );
        assert_eq!(
            boss_group("Realm", 56342, "Alien Reactor"),
            Some(BossGroup::VeteranHero)
        );
    }

    #[test]
    fn curated_type_overrides_classify_label_gap_bosses() {
        // Killer Bee Nest hive + Beehemoths -> Veteran Encounters.
        for t in [4312, 4324, 4325, 4326] {
            assert_eq!(
                boss_group("Realm", t, "Beehemoth"),
                Some(BossGroup::VeteranEncounter)
            );
        }
        // Oryx's Castle minibosses fought from the realm -> Adept dungeon bosses.
        assert_eq!(
            boss_group("Realm", 8200, "Janus the Doorwarden"),
            Some(BossGroup::Adept)
        );
        assert_eq!(
            boss_group("Realm", 3448, "Stone Guardian"),
            Some(BossGroup::Adept)
        );
        // Slumbering Dragon (empty labels) resolves as an Adept Hero by name/id.
        assert_eq!(
            boss_group("Realm", 34598, "Slumbering Dragon"),
            Some(BossGroup::AdeptHero)
        );
    }

    #[test]
    fn hermit_god_tentacle_override_classifies_as_adept_encounter() {
        // The Hermit God Tentacle add carries only the "New Hermit God Encounter"
        // group, not the ADEPT_ENCOUNTER label its boss (22023) has. The
        // aggregated tentacle fight uses the legacy repr type 3428, whose
        // encounter anchor (legacy 3425) is absent from current assets, so
        // without the override it falls through to no group and leaks past the
        // Adept Encounters filter.
        assert_eq!(type_group_override(3428), Some(BossGroup::AdeptEncounter));
        assert_eq!(type_group_override(22026), Some(BossGroup::AdeptEncounter));
        assert_eq!(
            boss_group("Realm", 3428, "Hermit God Tentacle"),
            Some(BossGroup::AdeptEncounter)
        );
        assert_eq!(
            boss_group("Realm", 22026, "New Hermit God Tentacle"),
            Some(BossGroup::AdeptEncounter)
        );
    }

    #[test]
    fn oryx_2_maps_to_wine_cellar_expert() {
        // Oryx the Mad God 2 (2354) fought from the realm is the Wine Cellar
        // boss; its fight is relabelled "Wine Cellar" so it classifies Expert.
        assert_eq!(
            super::super::canonical_dungeon_for_boss(2354),
            Some("Wine Cellar")
        );
        assert_eq!(
            boss_group("Wine Cellar", 2354, "Oryx the Mad God 2"),
            Some(BossGroup::Expert)
        );
    }

    #[test]
    fn oryx_castle_bosses_map_to_castle_adept() {
        // Janus the Doorwarden and the Stone Guardians fought from the realm
        // belong to Oryx's Castle; their fights are relabelled so they classify
        // as Adept dungeon bosses.
        for t in [8200, 3448, 3449] {
            assert_eq!(
                super::super::canonical_dungeon_for_boss(t),
                Some("Oryx's Castle")
            );
            assert_eq!(
                boss_group("Oryx's Castle", t, "Stone Guardian"),
                Some(BossGroup::Adept)
            );
        }
    }

    #[test]
    fn hero_bosses_classify_into_their_tier() {
        // Ghost King (a curated Rookie hero) resolves by name even on a realm map.
        assert_eq!(
            boss_group("Realm of the Mad God", 0, "Ghost King"),
            Some(BossGroup::RookieHero)
        );
        // Beach Bum carries ADEPT_ENCOUNTER but is curated as an Adept hero; the
        // hero-tier check runs first, so it lands in AdeptHero.
        assert_eq!(hero_tier(22034, "Beach Bum"), Some(BossGroup::AdeptHero));
        // Organ Harvester is a Veteran hero.
        assert_eq!(hero_tier(34509, ""), Some(BossGroup::VeteranHero));
        // "New" rookie hero variants resolve by object id even without labels.
        assert_eq!(hero_tier(21901, ""), Some(BossGroup::RookieHero));
        assert_eq!(hero_tier(21904, ""), Some(BossGroup::RookieHero));
        assert_eq!(hero_tier(21946, ""), Some(BossGroup::RookieHero));
    }

    #[test]
    fn pentaract_towers_inherit_adept_encounter_group() {
        // The recorded Pentaract fights use Tower object types that carry no
        // ADEPT_ENCOUNTER label; only the invisible marker (22019) does. The
        // encounter fallback must classify the towers as Adept Encounters so
        // they stay filterable. Skips when game assets are not extracted.
        let mgr = get_asset_manager();
        if let Some(dir) = crate::assets::find_assets_dir() {
            mgr.set_assets_dir(&dir);
        }
        if !mgr.try_load() || mgr.object_labels(22019).is_none() {
            eprintln!("skipping: game assets not available");
            return;
        }
        for tower in [1337, 3422, 22018] {
            assert_eq!(
                boss_group("Realm of the Mad God", tower, "Pentaract Tower"),
                Some(BossGroup::AdeptEncounter),
                "tower {} should inherit the Pentaract encounter group",
                tower
            );
        }
    }

    #[test]
    fn special_bosses_and_crates_classify_by_object_type() {
        // Curated object-type groups are static binary-search lists,
        // so they classify without loaded game assets and win over dungeon band.
        assert_eq!(
            boss_group("Parasite Chambers", 2078, "Infested Chest"),
            Some(BossGroup::TreasureCrate)
        );
        assert_eq!(
            boss_group("Deadwater Docks", 620, "Bilgewater's Booty"),
            Some(BossGroup::TreasureCrate)
        );
        assert_eq!(
            boss_group("Realm of the Mad God", 44020, "The Glitch"),
            Some(BossGroup::SeasonalEncounter)
        );
        // The MOTMG event Wanderer classifies as special...
        assert_eq!(
            boss_group("Realm of the Mad God", 49755, "The Wanderer"),
            Some(BossGroup::SeasonalEncounter)
        );
        // ...but the Interregnum dungeon Wanderer stays with its Exaltation
        // dungeon (Hidden Interregnum), not the special group.
        assert_eq!(
            boss_group("Hidden Interregnum", 14474, "The Wanderer"),
            Some(BossGroup::Exaltation)
        );
        // Key Fairy is a moving loot piñata -> treasure crate category.
        assert_eq!(
            boss_group("Realm of the Mad God", 18376, "Key Fairy"),
            Some(BossGroup::TreasureCrate)
        );
        // Alien Invasion Satellite Cores are lootable crates, not dungeon
        // bosses, so they override the Expert/Exaltation dungeon band.
        for t in [6869, 45906, 45909, 56425, 56432] {
            assert_eq!(
                boss_group("Malogia", t, "Satellite Core"),
                Some(BossGroup::TreasureCrate),
                "Satellite Core {t} should be a treasure crate"
            );
        }
        // Prismimic variants both land in the seasonal/special group.
        assert_eq!(
            boss_group("Realm of the Mad God", 19249, "Prismimic Defender"),
            Some(BossGroup::SeasonalEncounter)
        );
    }

    #[test]
    fn string_round_trip() {
        for g in BossGroup::ALL {
            assert_eq!(BossGroup::from_str(g.as_str()), Some(g));
        }
        assert_eq!(BossGroup::from_str("nope"), None);
    }

    #[test]
    fn dungeon_groups_have_deduped_catalog_bosses() {
        // The four difficulty groups draw from the static table (no asset manager
        // needed). Each is non-empty and free of duplicate boss names.
        for g in [
            BossGroup::Exaltation,
            BossGroup::Expert,
            BossGroup::Adept,
            BossGroup::Beginner,
        ] {
            let bosses = g.catalog_bosses();
            assert!(!bosses.is_empty(), "{:?} has no catalog bosses", g);
            let mut names: Vec<String> = bosses.iter().map(|e| e.name.to_lowercase()).collect();
            let count = names.len();
            names.sort();
            names.dedup();
            assert_eq!(count, names.len(), "{:?} has duplicate boss names", g);
        }
        // A boss shared by two dungeons collapses to one row.
        let exalt = BossGroup::Exaltation.catalog_bosses();
        assert_eq!(
            exalt
                .iter()
                .filter(|e| e.name == "Factory Control Core")
                .count(),
            1,
        );
    }

    #[test]
    fn veteran_curation_includes_unlabeled_additions() {
        // These bosses are missing from the raw VETERAN_ENCOUNTER label scan
        // (minion-tagged or unlabeled), so curate_veteran_encounters adds them
        // back. This guards Lost Sentry and Kogbold Expedition Engine, which
        // were previously absent from the picker and Combat History.
        let entries = curate_veteran_encounters(Vec::new());
        for (id, name) in [
            (22128, "Beer God"),
            (4312, "Killer Bee Nest"),
            (22146, "Lost Sentry"),
            (22109, "Kogbold Expedition Engine"),
        ] {
            assert!(
                entries
                    .iter()
                    .any(|e| e.sprite_ids.contains(&id) && e.name == name),
                "veteran catalog missing {name} ({id})",
            );
        }
        // Alien Reactor was moved out of Veteran Encounters into Veteran Heroes
        // of Oryx, so it must no longer appear in the encounter catalog.
        assert!(
            !entries.iter().any(|e| e.name == "Alien Reactor"),
            "Alien Reactor should not be a Veteran Encounter",
        );
    }

    #[test]
    fn adept_curation_includes_beer_god_and_additions() {
        // Beer God counts as both an Adept and Veteran encounter, so it must be
        // present in the Adept catalog too. Mammoth Rat / Carp Emperor / Ghost
        // Ship carry no ADEPT_ENCOUNTER label and are added explicitly.
        let entries = curate_adept_encounters(Vec::new());
        for (id, name) in [
            (22128, "Beer God"),
            (18007, "Mammoth Rat"),
            (34559, "Carp Emperor"),
            (22042, "Ghost Ship"),
        ] {
            assert!(
                entries
                    .iter()
                    .any(|e| e.sprite_ids.contains(&id) && e.name == name),
                "adept catalog missing {name} ({id})",
            );
        }
    }
}

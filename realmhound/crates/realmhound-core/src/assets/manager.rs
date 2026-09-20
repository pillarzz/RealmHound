//! Global asset manager for lazy-loaded, cached game assets.
//!
//! The AssetManager provides thread-safe access to game assets with:
//! - Lazy initialization (loads on first access)
//! - Single load guarantee (never reloads the same asset)
//! - Efficient lookups (HashMap-based O(1) access)
//! - Automatic extraction from game files if needed
//!
//! # Usage
//!
//! ```ignore
//! use realmhound_core::assets::ASSET_MANAGER;
//!
//! // Get item name by ID
//! if let Some(name) = ASSET_MANAGER.object_name(0x0b25) {
//!     println!("Item: {}", name);
//! }
//! ```

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use super::dungeon_category::{build_categories, DungeonCategory};
use super::dungeon_data::DungeonData;
use super::dungeon_mods_xml::{ModifierDef, ModifierTable};
use super::enchantments::EnchantmentList;
use super::item_category::ItemCategorizer;
use super::object_list::{
    AbilityEffects, LethalStrikeParams, ObjectAsset, ObjectList, TileAsset, TileList,
};
use super::sprite_atlas::{SpriteAtlas, SpriteData};
use super::unity::{find_resources_assets, UnityExtractor};

/// How the dye/cloth color is applied.
#[derive(Debug, Clone)]
pub enum DyeStyle {
    /// Solid RGB color dye – tint the masked area with this color.
    SolidColor(u8, u8, u8),
    /// Textile pattern – tile this sprite into the masked area.
    TextilePattern(SpriteData),
}

/// Dye rendering information for an item (mask sprite + fill style).
#[derive(Debug, Clone)]
pub struct DyeInfo {
    /// Sprite atlas data for the mask texture
    pub mask_sprite: SpriteData,
    /// How to fill the masked region
    pub style: DyeStyle,
}

/// Character dye rendering information (two independent dye channels).
///
/// RotMG character masks use three channels:
/// - **Red + Alpha** = large cloth / clothing region (tex1)
/// - **Green + Alpha** = small cloth / accessory region (tex2)
/// - Alpha = 0 means "don't dye" regardless of R/G values.
#[derive(Debug, Clone)]
pub struct CharacterDyeInfo {
    /// Mask sprite coordinates on the characters_masks atlas (atlas 3)
    pub mask_sprite: SpriteData,
    /// Clothing dye (tex1) - applied to red-channel mask region
    pub clothing: Option<DyeStyle>,
    /// Accessory dye (tex2) - applied to green-channel mask region
    pub accessory: Option<DyeStyle>,
}

/// Global asset manager instance.
pub static ASSET_MANAGER: OnceLock<AssetManager> = OnceLock::new();

/// Get or initialize the global asset manager.
pub fn get_asset_manager() -> &'static AssetManager {
    ASSET_MANAGER.get_or_init(AssetManager::new)
}

/// Pure predicate behind [`AssetManager::is_boss_like`]: whether an object's
/// catalog `labels` / `group` mark it as a boss, mini-boss, Hero of Oryx, or
/// notable encounter. Excludes over-broad mechanical labels (`GOD`, `BOSSFIGHT`)
/// that also appear on trash minions and spawned adds.
fn labels_are_boss_like(labels: &str, group: &str) -> bool {
    labels.split(',').any(|l| {
        matches!(l, "BOSS" | "MINIBOSS" | "HERO" | "QUEST" | "ENCOUNTER" | "BEACON_GUARDIAN")
                // Tier-prefixed encounter markers (ADEPT_ENCOUNTER, VETERAN_ENCOUNTER)
                // that the newer catalog puts on realm-event bosses whose only
                // other label may be missing (Man-eating Barnacle, Artificial
                // Slop carry ADEPT_ENCOUNTER alone). Minion adds that also carry
                // one are still gated by labels_are_strong_boss / is_minion.
                || l.ends_with("_ENCOUNTER")
    }) || group.ends_with("Encounter")
}

/// A stricter variant of [`labels_are_boss_like`] that omits the weak `QUEST`
/// marker. `QUEST` is the in-game quest-arrow tag carried by plenty of realm
/// trash (e.g. Urgle the Traptosser is `MINION,QUEST,MINION_MID`), so it is not
/// strong enough on its own to promote a spawned minion to a real boss fight.
/// Genuine minibosses that are also minion-tagged (Heroes of Oryx like Red Demon
/// = `MINION,HERO,QUEST,MINION_STRONG`) always carry BOSS/HERO/ENCOUNTER too.
fn labels_are_strong_boss(labels: &str, group: &str) -> bool {
    labels.split(',').any(|l| {
        matches!(
            l,
            "BOSS" | "MINIBOSS" | "HERO" | "ENCOUNTER" | "BEACON_GUARDIAN"
        )
    }) || group.ends_with("Encounter")
}

/// Pure predicate behind [`AssetManager::is_minion`]: whether an object's catalog
/// `labels` mark it as a spawned minion / clone / stage-add. Matches the exact
/// `MINION` label and the tier variants (`MINION_WEAK`/`MINION_MID`/etc.). These
/// never contribute to soulbound loot and must not be tracked as boss fights,
/// even when their max HP is boss-tier (e.g. Crystal Prisoner Clone, Mysterious
/// Crystal). `MINIBOSS` is intentionally NOT matched.
fn labels_are_minion(labels: &str) -> bool {
    labels
        .split(',')
        .any(|l| l == "MINION" || l.starts_with("MINION_"))
}

/// Curated allow-list of object type-ids that ARE their own tracked fight even
/// though the classifier would otherwise reject them: dungeon minibosses the
/// catalog tags `MINION` (so [`labels_are_minion`] demotes them, e.g. Sea Dragon
/// vs Sea Dragon Carp), labelled adds/gate bosses that carry no boss label, and
/// treasure-room loot chests (`TRACKLOOT`/`CHEST` but no boss label) that the
/// player breaks open and expects to see on the dungeon card. Neither labels nor
/// an HP threshold can separate these from noise, so an explicit list is the only
/// reliable signal.
///
/// Kept deliberately minimal: only add a type here after a real capture confirms
/// it is a genuine standalone fight (not a swarm add). Note the `Deep Sea` gods
/// (Abyssal Kraken/Squid/Jellyfish/Siren/Hydra, Hadopelagic Submarine) share the
/// same `MINION,DEEPSEA,GOD` labels but appear as ~2.5k-HP swarm adds, so they
/// are intentionally excluded. Seeded incrementally. Keep sorted.
const CURATED_BOSS_TYPES: &[i32] = &[
    620,   // Bilgewater's Booty A (Deadwater Docks treasure chest) -- ENEMY,MINION,TRACKLOOT
    621,   // Bilgewater's Booty B (Deadwater Docks treasure chest) -- ENEMY,MINION,TRACKLOOT
    622,   // Bilgewater's Booty C (Deadwater Docks treasure chest) -- ENEMY,MINION,TRACKLOOT
    1033,  // Sea Dragon (Deep Sea Abyss) -- MINION,BEAST,GOD,MINION_MID, 50k HP
    2078,  // Infested Chest (Parasite Chambers treasure crate) -- CHEST,BOSSFIGHT,MINIBOSS
    2202,  // BB Low Egg (Buffed Bunny Easter egg crate) -- ENEMY,QUEST,NATURE
    2203,  // BB Mid Egg (Buffed Bunny Easter egg crate) -- ENEMY,QUEST,NATURE
    2204,  // BB High Egg (Buffed Bunny Easter egg crate) -- ENEMY,QUEST,NATURE
    2205,  // BB God Egg (Buffed Bunny Easter egg crate) -- ENEMY,QUEST,NATURE
    3369, // Active Sarcophagus (Tomb of the Ancients miniboss) -- ENEMY,MINION,QUEST,TRACKLOOT,GOD,MINION_STRONG
    3372, // Treasure Sarcophagus (Tomb of the Ancients treasure chest) -- ENEMY,MINION,TRACKLOOT,GOD,MINION_STRONG
    5893, // Coral Gift (Ocean Trench treasure crate) -- no labels
    5943, // Manor Coffin (Manor of the Immortals treasure room) -- ENEMY,MINION,TRACKLOOT,MINION_STRONG
    8813, // Dimitus (special boss) -- ENEMY,MINION,BEAST,GOD,MINION_STRONG
    9110, // Woodland Skysplitter Stone (Woodland Labyrinth secret chest) -- ENEMY,MINION,MINION_WEAK
    13208, // mgm2 Silver Son of Arachna Giant Egg Sac (Crawling Depths) -- ENEMY,MINION,TRACKLOOT,MINION_STRONG
    13209, // mgm2 Red Son of Arachna Giant Egg Sac (Crawling Depths) -- ENEMY,MINION,TRACKLOOT,MINION_STRONG
    13210, // mgm2 Blue Son of Arachna Giant Egg Sac (Crawling Depths) -- ENEMY,MINION,TRACKLOOT,MINION_STRONG
    13211, // mgm2 Green Son of Arachna Giant Egg Sac (Crawling Depths) -- ENEMY,MINION,TRACKLOOT,MINION_STRONG
    14474, // The Wanderer (Ultra Easy variant, special boss) -- no labels
    14477, // The Wanderer (Easy variant, special boss) -- no labels
    17391, // The Wanderer (Hard variant, special boss) -- no labels
    18346, // The Wanderer (Mid variant, special boss) -- no labels
    18376, // Key Fairy Easy (special boss) -- no labels
    18377, // Key Fairy Mid (special boss) -- no labels
    18378, // Key Fairy Hard (special boss) -- no labels
    19247, // Prismimic Attacker (special boss) -- ENEMY,MINION,CONSTRUCT,MINION_STRONG
    19249, // Prismimic Defender (special boss) -- ENEMY,MINION,CONSTRUCT,MINION_STRONG
    21901, // New Phoenix Lord (Rookie Hero of Oryx) -- not always labeled MINIBOSS
    21904, // New Phoenix Reborn (Rookie Hero of Oryx) -- not always labeled MINIBOSS
    21943, // New Kage Kami (Adept Hero of Oryx) -- ENEMY,MINION,UNDEAD,MINION_MID, 3000 HP, no boss label
    21946, // New Great Coil Snake (Rookie Hero of Oryx) -- not always labeled MINIBOSS
    22109, // Kogbold Expedition Engine (realm event) -- MINION,BEAST,MINION_WEAK,VETERAN_ENCOUNTER, 700k HP
    24020, // Criminal Monkey (Animal Merchant realm set-piece spawn) -- no labels
    24021, // Fraudulent Tiger (Animal Merchant realm set-piece spawn) -- no labels
    24023, // Animal Merchant (Adept Hero of Oryx realm set-piece) -- no labels
    28065, // Sunken Treasure (Davy Jones' Locker treasure chest) -- ENEMY,MINION,TRACKLOOT,MINION_STRONG
    28671, // Old Chest (Mountain Temple treasure chest) -- CHEST,MOUNTAIN_TEMPLE (no boss label)
    28796, // Bartholomew the Massive Parrot (Deadwater Docks) -- ENEMY,BEAST (no boss label), 17.5k HP gate boss for Jon Bilgewater
    28797, // Silver Son of Arachna Giant Egg Sac (Crawling Depths) -- ENEMY,MINION,TRACKLOOT,MINION_STRONG
    28798, // Red Son of Arachna Giant Egg Sac (Crawling Depths) -- ENEMY,MINION,TRACKLOOT,MINION_STRONG
    28799, // Blue Son of Arachna Giant Egg Sac (Crawling Depths) -- ENEMY,MINION,TRACKLOOT,MINION_STRONG
    28810, // Green Son of Arachna Giant Egg Sac (Crawling Depths) -- ENEMY,MINION,TRACKLOOT,MINION_STRONG
    28867, // Ruins Lamp ("Magic Lamp") -- Ancient Ruins reward crate spawned by the Genie -- ENEMY,MINION,MINION_STRONG
    29023, // DS Golden Rat -- Toxic Sewers wandering loot piñata that drops loot directly -- ENEMY,MINION,MINION_STRONG
    33018, // Oryxmas Realm Present (realm treasure crate) -- ENEMY,MINION,MINION_STRONG
    33280, // Shatters Stone Idol (secret hardmode boss) -- GOD,CONSTRUCT, 25k HP, no boss label
    42255, // The Hemomancer (special boss) -- no labels
    42371, // Jotunn (special boss) -- no labels
    44020, // The Glitch (special boss) -- ENEMY,MINION,CUBE,GOD,MINION_STRONG
    47414, // Dimitus Easy (special boss) -- ENEMY,MINION,BEAST,GOD,MINION_STRONG
    52556, // Retro shtrs Bridge Sentinel ("The Forgotten Sentinel") -- Legacy Shatters boss 1, no labels
    52557, // Retro shtrs Twilight Archmage -- Legacy Shatters boss 2 anchor, no labels
    52560, // Retro shtrs The Forgotten King -- Legacy Shatters boss 3 (final), the real King
    52662, // Retro shtrs / Woodland Retro Epic Larva ("Retro Megamoth Larva") -- Legacy Woodland boss phase 1, no labels
    52663, // Retro Epic Mama Megamoth ("Retro Mammoth Megamoth") -- Legacy Woodland boss phase 2, no labels
];

/// Whether `id` is in the curated boss allow-list ([`CURATED_BOSS_TYPES`]).
fn is_curated_boss_type(id: i32) -> bool {
    CURATED_BOSS_TYPES.binary_search(&id).is_ok()
}

/// Curated deny-list of object type-ids that must NEVER be their own fight even
/// though they pass the label-less HP fallback. These are boss summons / stage
/// adds that carry boss-tier HP but no catalog labels at all, so the classifier
/// cannot otherwise tell them apart from genuinely-uncatalogued bosses (Elder
/// Sprite Tree). Like [`CURATED_BOSS_TYPES`], seeded incrementally.
/// Keep sorted by id.
const CURATED_NON_BOSS_TYPES: &[i32] = &[
    3426,  // Hermit Minion -- Hermit God Encounter minion (grp ends "Encounter"), hidden
    3427,  // Whirlpool -- Hermit God Encounter add, hidden
    3428,  // Hermit God Tentacle -- routed to the aggregated tentacle row
    3429,  // Hermit God Tentacle Spawner -- invisible spawner, hidden
    5534,  // CC Crystal Worm Child -- Fungal Cavern add, routed to the aggregated child row
    5535,  // CC Crystal Worm Child Body -- Fungal Cavern add, routed to the child row
    5536,  // CC Crystal Worm Child Tail -- Fungal Cavern add, routed to the child row
    16907, // Galleon Parrot -- Bilgewater's Galleon add, Character/boss-tier HP, no labels
    16933, // Flying Behemoth Tornado ("Tornado") -- Flying Behemoth add, no boss labels
    16936, // Monolith Siphon 1 -- Sentient Monolith add, routed to the siphon row
    16937, // Monolith Siphon 2 -- Sentient Monolith add, routed to the siphon row
    16938, // Monolith Siphon 3 -- Sentient Monolith add, routed to the siphon row
    16970, // Monolith Siphon 4 -- Sentient Monolith add, routed to the siphon row
    16971, // Sentient Monolith Protector ("Monolith Protector") -- Sentient Monolith add, no boss labels
    16993, // Ravenous Rot Overgrowth -- Ravenous Rot last-phase tentacle, damaged to progress, not its own fight
    18000, // Giant City Rat -- Rat Extermination minigame minion, no labels, event-scales above the HP fallback
    18001, // Small Rat -- Rat Extermination minigame minion, no labels, event-scales above the HP fallback
    18002, // Medium Rat -- Rat Extermination minigame minion, no labels, event-scales above the HP fallback
    18003, // Large City Rat -- Rat Extermination minigame minion, no labels, event-scales above the HP fallback
    18047, // Special Small City Rat -- Rat Extermination minigame minion, 10k HP, no labels
    18048, // Medium Rat -- Rat Extermination minigame minion, no labels, event-scales above the HP fallback
    18049, // Special Large City Rat -- Rat Extermination minigame minion, 10k HP, no labels
    18050, // Golden Rat -- Rat Extermination minigame minion, no labels, event-scales above the HP fallback
    20553, // Challenge Gate -- Moonlight Village gate object, boss-tier HP, not a fight
    20577, // MV Easy Dropper -- Moonlight Village loot dropper, not a fight
    20658, // MV Dungeon Complete -- invisible completion marker, not a fight
    21862, // New Urgle -- realm Traptosser minion (MINION/QUEST tags), not a boss
    21963, // Shades of the Avatar (new) -- Avatar add, routed to the shades row
    21964, // ShadowBombs (new) -- Avatar add, routed to the ShadowBombs row
    21965, // ShadowBombs Deflated (new) -- Avatar add, routed to the ShadowBombs row
    21967, // Killer Pillar 1 (new) -- Avatar add, routed to the pillar row
    21968, // Killer Pillar 2 (new) -- Avatar add, routed to the pillar row
    21969, // Killer Pillar 3 (new) -- Avatar add, routed to the pillar row
    21970, // Killer Pillar 4 (new) -- Avatar add, routed to the pillar row
    21971, // Killer Pillar 1a (new) -- Avatar add, routed to the pillar row
    21972, // Killer Pillar 2a (new) -- Avatar add, routed to the pillar row
    21973, // Killer Pillar 3a (new) -- Avatar add, routed to the pillar row
    21974, // Killer Pillar 4a (new) -- Avatar add, routed to the pillar row
    21976, // Eye of the Avatar (new) -- Avatar add, routed to the eye row
    22024, // New Hermit Minion -- New Hermit God Encounter minion, hidden
    22025, // New Whirlpool -- New Hermit God Encounter add, hidden
    22026, // New Hermit God Tentacle -- routed to the aggregated tentacle row
    22027, // New Hermit God Tentacle Spawner -- invisible spawner, hidden
    23708, // SpecPen Garden Gravestone L -- SpecPen Gretch-branch prop, Character/boss-tier HP, no labels
    23747, // SpecPen Garden Gravestone -- SpecPen prop, Character/boss-tier HP, no labels
    23840, // SpecPen Door Switch -- SpecPen branch-door prop, Character/boss-tier HP, no labels
    23932, // SpecPen TortureBranch Floor Switch -- SpecPen prop, Character/boss-tier HP, no labels
    23934, // SpecPen LBT Transformation 4B -- Sentipede body segment, folded into the Lobotomik head row
    23935, // SpecPen LBT Transformation 4T -- Sentipede body segment, folded into the Lobotomik head row
    23961, // SpecPen LBT Transformation 4 -- Sentipede head, aggregates its body-segment damage
    24006, // Legion Footsoldier -- Legion General summon, ~15k HP, no labels
    24007, // Legion Major -- Legion General summon, ~27k HP, no labels
    24061, // Administration Turret ("SpecPen Admin Turret") -- Oculon fight add, boss-tier HP, no labels
    24412, // SpecPen Cell Switch -- SpecPen prop, Character/boss-tier HP, no labels
    24454, // SpecPen GardenBranch Tutorial Object 1 -- tutorial dummy, boss-tier HP, no labels
    24455, // SpecPen GardenBranch Tutorial Object 2 -- tutorial dummy, boss-tier HP, no labels
    24456, // SpecPen CellBranch Tutorial Object -- tutorial dummy, boss-tier HP, no labels
    24457, // SpecPen TortureBranch Tutorial Object 1 -- tutorial dummy, boss-tier HP, no labels
    24458, // SpecPen TortureBranch Tutorial Object 2 ("Skull Pylon") -- tutorial dummy, boss-tier HP, no labels
    24459, // SpecPen AdminBranch Tutorial Object ("Administration Turret") -- tutorial dummy, boss-tier HP, no labels
    29518, // Shades of the Avatar (legacy) -- Avatar add, routed to the shades row
    29519, // Killer Pillar 1 (legacy) -- Avatar add, routed to the pillar row
    29530, // Killer Pillar 2 (legacy) -- Avatar add, routed to the pillar row
    29531, // Killer Pillar 3 (legacy) -- Avatar add, routed to the pillar row
    29532, // Killer Pillar 4 (legacy) -- Avatar add, routed to the pillar row
    29535, // Eye of the Avatar (legacy) -- Avatar add, routed to the eye row
    34448, // ShadowBombs (legacy) -- Avatar add, routed to the ShadowBombs row
    34449, // ShadowBombs Deflated (legacy) -- Avatar add, routed to the ShadowBombs row
    34451, // Killer Pillar 1a (legacy) -- Avatar add, routed to the pillar row
    34452, // Killer Pillar 2a (legacy) -- Avatar add, routed to the pillar row
    34453, // Killer Pillar 3a (legacy) -- Avatar add, routed to the pillar row
    34454, // Killer Pillar 4a (legacy) -- Avatar add, routed to the pillar row
    34457, // Baneserpent Head 1 -- Adult Baneserpent segment, DisplayId "Adult Baneserpent", no labels
    34458, // Baneserpent Head 2 -- Adult Baneserpent segment, DisplayId "Adult Baneserpent", no labels
    34459, // Baneserpent Head 3 -- Adult Baneserpent segment, DisplayId "Adult Baneserpent", no labels
    34462, // Well of Souls Skeleton ("Possessed Skeleton") -- Well of Souls add, no boss labels
    34465, // Galleon Admiral -- Bilgewater's Galleon add, event-scaled HP, no boss labels, not its own fight
    34467, // Baneserpent Neck 1 -- Adult Baneserpent segment, DisplayId "Adult Baneserpent", no labels
    34468, // Baneserpent Neck 2 -- Adult Baneserpent segment, DisplayId "Adult Baneserpent", no labels
    34469, // Baneserpent Neck 3 -- Adult Baneserpent segment, DisplayId "Adult Baneserpent", no labels
    34474, // Baneserpent Impact Telegraph -- Adult Baneserpent segment, DisplayId "Adult Baneserpent", no labels
    34490, // Lethal Cure A -- The Plague Doctor add, routed to the aggregated cure row
    34491, // Lethal Cure B -- The Plague Doctor add, routed to the aggregated cure row
    34492, // Lethal Cure C -- The Plague Doctor add, routed to the aggregated cure row
    34504, // Lethal Cure A (Effect) -- The Plague Doctor add, routed to the cure row
    34505, // Lethal Cure B (Effect) -- The Plague Doctor add, routed to the cure row
    34506, // Lethal Cure C (Effect) -- The Plague Doctor add, routed to the cure row
    34546, // World's Pearl -- World's Oyster add, routed to the pearl row
    34547, // World's Oyster Coral (Spawner) -- World's Oyster add, no labels
    34552, // Goblin Patriarch Villager ("Goblin Villager") -- Goblin Patriarch Adept Encounter minion, event-scaled HP, no labels
    34556, // Goblin Villager -- Goblin Patriarch Adept Encounter minion, event-scaled HP, no labels
    34583, // White Blood Cell -- Bloodroot Heart add, no labels, event-scaled >10k HP
    34587, // Lich King Grave -- The Lich King add, 5k HP, no boss labels
    34604, // Angry Hornet -- Hornet's Nest add, routed to the aggregated hornet row
    44354, // SpecPen TortureBranch Floor Switch Red -- SpecPen prop, Character/boss-tier HP, no labels
    44355, // SpecPen TortureBranch Floor Switch Orange -- SpecPen prop, Character/boss-tier HP, no labels
    44356, // SpecPen TortureBranch Floor Switch Yellow -- SpecPen prop, Character/boss-tier HP, no labels
    44357, // SpecPen TortureBranch Floor Switch Green -- SpecPen prop, Character/boss-tier HP, no labels
    44358, // SpecPen TortureBranch Floor Switch Blue -- SpecPen prop, Character/boss-tier HP, no labels
    44359, // SpecPen TortureBranch Floor Switch Purple -- SpecPen prop, Character/boss-tier HP, no labels
    44360, // SpecPen TortureBranch Floor Switch Pink -- SpecPen prop, Character/boss-tier HP, no labels
    44361, // SpecPen TortureBranch Floor Switch Brown -- SpecPen prop, Character/boss-tier HP, no labels
    45408, // LH Spawn Pillar -- Lost Halls spawner, Character/boss-tier HP, no labels
    45711, // GC Boss Support (Crystal Worm Father) -- Fungal Cavern add, 37.5k HP, no labels, routed to the father row
    47928, // Deity Overseer -- Cube Deity respawnable swarming add, not its own fight
    47929, // Deity Defender -- Cube Deity respawnable swarming add, not its own fight
    49339, // MV Umi Complete -- invisible completion marker, not a fight
    51033, // Eye of the Storm Tornado -- Eye of the Storm add, routed to the tornado row
    51034, // Possessed Small Pumpkin -- Halloween event add, event-scaled HP, no labels
    51062, // Astral Guard -- Astral Rift spawn, routed to the aggregated spawns row
    51063, // Astral Beast -- Astral Rift spawn, routed to the aggregated spawns row
    51073, // Royal Crab -- Crab Sovereign add, routed to the royal crab row
    51077, // Aerial Warship Crew -- Aerial Warship add, 6.75k HP, no boss labels
    51080, // Bramblethorn Bud -- Corrupted Bramblethorn add, no labels, event-scaled >10k HP
    // Legacy The Shatters ("Retro shtrs") arena trash. The real bosses are the
    // Forgotten Sentinel (52556), Twilight Archmage (52557) and the King
    // (52560); Blizzard/Inferno fold into the Archmage as aux rows. The six
    // bridge guards (Titanums 52532/52533/52535/52536, Paladin Obelisks
    // 52534/52537) gate the Sentinel, and the two Royal Guardians L (52558) plus
    // the four colored crystals (52572-52575) gate the King, so they fold into
    // those boss cards as aux rows (see AUX_TARGETS) instead of being hidden.
    // Royal Guardian J (52559) spawns only after the King is already vulnerable
    // (the fight has begun), so it stays hidden. Everything else here
    // (generators, portals, mages, lasers, loot balloons, the other Titanums)
    // cleared the HP fallback and bloated the dungeon card.
    52513, // Retro shtrs Bridge Titanum
    52522, // Retro shtrs MagiGenerators ("Magi-Generator")
    52531, // Retro shtrs Blobomb
    52538, // Retro shtrs Titanum
    52539, // Retro shtrs Paladin Obelisk
    52540, // Retro shtrs Ice Mage ("Forgotten Ice Mage")
    52541, // Retro shtrs Ice Adept
    52542, // Retro shtrs Glassier Archmage ("Glacier Archmage")
    52543, // Retro shtrs Fire Mage
    52544, // Retro shtrs Fire Adept
    52545, // Retro shtrs Archmage of Flame
    52546, // Retro shtrs Stone Mage
    52547, // Retro shtrs Stone Knight
    52548, // Retro shtrs Stone Paladin
    52549, // Retro shtrs Spike
    52550, // Retro shtrs Ice Shield ("Ice Sphere")
    52551, // Retro shtrs Ice Portal
    52552, // Retro shtrs Fire Portal
    52553, // Retro shtrs S Archmage of Flame
    52554, // Retro shtrs S Glassier Archmage
    52555, // Retro shtrs Eyeguy ("Eyes of the King")
    52559, // Retro shtrs Royal Guardian J -- spawns after the King is vulnerable
    52562, // Retro shtrs Firebomb
    52563, // Retro shtrs Inferno -- Twilight Archmage phoenix, routed to the Inferno aux row
    52564, // Retro shtrs Blizzard -- Twilight Archmage phoenix, routed to the Blizzard aux row
    52568, // Retro shtrs Ice Shield 2 ("Ice Sphere")
    52569, // Retro shtrs Ice Portal 2
    52570, // Retro shtrs Fire Portal 2
    52571, // Retro shtrs The Cursed Crown
    52579, // Retro shtrs Laser1 (King laser, DisplayId "The Forgotten King")
    52580, // Retro shtrs Laser2 (King laser)
    52581, // Retro shtrs Laser3 (King laser)
    52582, // Retro shtrs Laser4 (King laser)
    52583, // Retro shtrs Laser5 (King laser)
    52584, // Retro shtrs Laser6 (King laser)
    52585, // Retro shtrs Lava Souls ("Helpless Souls")
    52590, // Retro shtrs Loot Balloon Bridge ("Sentinel Chest")
    52591, // Retro shtrs Loot Balloon Mage ("Mage Chest")
    52592, // Retro shtrs Loot Balloon King ("King Chest")
    // Legacy Woodland Labyrinth trash: only the three Megamoth forms
    // (52662/52663/52665) are the boss; deny the squirrels, turrets and goblins
    // so the Murderous Megamoth (52665) anchors the grouped card.
    52653, // Retro Woodland Paralyze Turret
    52654, // Retro Woodland Silence Turret
    52655, // Retro Woodland Weakness Turret
    52657, // Retro Woodland Mini Megamoth ("Micro Megamoth Sentinel")
    52658, // Retro Woodland Ultimate Squirrel ("Mecha Squirrel")
    52659, // Retro Woodland Goblin Mage ("Forest Goblin Necromancer")
    52660, // Retro Woodland Goblin ("Forest Goblin Bruiser")
    53017, // Legion Missionary Holy Orb -- Legion Missionary add, no labels, 12.5k HP
    53018, // Legion Missionary Chaos Orb -- Legion Missionary add, no labels, 17.5k HP
];

/// Whether `id` is in the curated deny-list ([`CURATED_NON_BOSS_TYPES`]).
fn is_curated_non_boss_type(id: i32) -> bool {
    CURATED_NON_BOSS_TYPES.binary_search(&id).is_ok()
}

/// Object type-ids that carry a group label (e.g. BEACON_GUARDIAN) but belong to
/// unused / unreachable content, so they can never actually be fought. They are
/// excluded from the boss-group filter tooltip catalogs. Keep sorted by id.
const UNFIGHTABLE_CATALOG_TYPES: &[i32] = &[
    24008, // Mutant Overgrowth -- Veteran Encounter marker, in files but not in-game
    34463, // Eye of the Underworld -- Adept Encounter marker, no longer spawns
    34539, // Gates of the Nether -- Adept Encounter marker, no longer spawns
    52990, // Legion Beach Guard -- unused Shore beacon biome
    52996, // Legion Reef Guardian -- unused Coral Reefs beacon biome
    53008, // Legion Abyssal Defender -- unused Deep Sea Abyss beacon biome
];

/// Curated list of boss object type-ids that WANDER in and out of the local
/// player's view within a single map instance (e.g. the Deadwater Docks roaming
/// miniboss). Each re-entry gives the boss a fresh object id, so without special
/// handling one wandering boss is recorded as several duplicate fights. The
/// combat engine suspends such a boss's fight when it despawns un-killed and
/// resumes it under the new object id when it returns. Seeded incrementally like
/// the other curated lists. Keep sorted by id.
///
/// Deliberately EXCLUDES the Commotion Crab (17410), the dungeon-modifier variant
/// that replaces the lone Calamity Crab with 4-6 smaller roaming crabs: they
/// share one object type, so suspend/resume (keyed by type) would collapse them
/// into a single entry. Being a labelled MINIBOSS they are each tracked as their
/// own fight instead, which is the desired per-crab breakdown.
const WANDERING_BOSS_TYPES: &[i32] = &[
    47387, // Calamity Crab -- Deadwater Docks roaming miniboss
];

/// Whether `id` is in the curated wandering-boss list ([`WANDERING_BOSS_TYPES`]).
fn is_wandering_boss_type(id: i32) -> bool {
    WANDERING_BOSS_TYPES.binary_search(&id).is_ok()
}

/// Curated list of boss object type-ids that SELF-DESTRUCT on death: the boss
/// explodes/despawns when killed, so its object is removed from the world at
/// high HP and the usual zero/<=10% HP kill heuristic never fires -- it would be
/// logged as "Escaped" despite being cleared. The combat engine
/// instead treats such a boss as killed on removal when the party dealt
/// essentially all of its remaining HP, which distinguishes a real kill from a
/// genuine escape (where only partial HP was dealt). Keep sorted by id.
const SELF_DESTRUCT_BOSS_TYPES: &[i32] = &[
    22109, // Kogbold Expedition Engine (realm train event) -- explodes when killed
];

/// Whether `id` is in the curated self-destruct list ([`SELF_DESTRUCT_BOSS_TYPES`]).
fn is_self_destruct_boss_type(id: i32) -> bool {
    SELF_DESTRUCT_BOSS_TYPES.binary_search(&id).is_ok()
}

/// Treasure crates / loot piñatas grouped under the "Treasure crates" Combat
/// History category. These are lootable objects the player breaks
/// open; classifying them as their own group lets the user gate tracking and
/// filter them out of the history independently of the dungeon they sit in.
/// Keep sorted by id.
const TREASURE_CRATE_TYPES: &[i32] = &[
    620,   // Bilgewater's Booty A (Deadwater Docks)
    621,   // Bilgewater's Booty B (Deadwater Docks)
    622,   // Bilgewater's Booty C (Deadwater Docks)
    2078,  // Infested Chest (Parasite Chambers)
    2202,  // BB Low Egg (Buffed Bunny Easter event)
    2203,  // BB Mid Egg (Buffed Bunny Easter event)
    2204,  // BB High Egg (Buffed Bunny Easter event)
    2205,  // BB God Egg (Buffed Bunny Easter event)
    3372,  // Treasure Sarcophagus (Tomb of the Ancients loot chest)
    5893,  // Coral Gift (Ocean Trench)
    5943,  // Manor Coffin (Manor of the Immortals treasure room)
    6869,  // Satellite Core - Malogia (tokens)
    6870,  // Satellite Core - Untaris (tokens)
    6871,  // Satellite Core - Forax (tokens)
    6872,  // Satellite Core - Katalund (tokens)
    9110,  // Woodland Skysplitter Stone (Woodland Labyrinth)
    18376, // Key Fairy Easy (moving loot fairy, does not shoot back)
    18377, // Key Fairy Mid
    18378, // Key Fairy Hard
    20789, // MV Fishing Loot 1 (Moonlight Village)
    20790, // MV Fishing Loot 2
    20791, // MV Fishing Loot 3
    20792, // MV Fishing Loot 4
    20793, // MV Fishing Loot 5
    20794, // MV Fishing Loot 6
    20812, // MV Fishing Loot 7
    28065, // Sunken Treasure (Davy Jones' Locker)
    28671, // Old Chest (Mountain Temple)
    28867, // Ruins Lamp / Magic Lamp (Ancient Ruins reward crate, spawned by the Genie)
    29023, // DS Golden Rat (Toxic Sewers wandering loot piñata)
    33018, // Oryxmas Realm Present (Oryxmas event)
    45906, // Satellite Core - Katalund
    45907, // Satellite Core - Malogia
    45908, // Satellite Core - Untaris
    45909, // Satellite Core - Forax
    46404, // DS Master Rat Box (Toxic Sewers reward crate, spawned by the Master Rat miniboss)
    56425, // Satellite Core - Neo Malogia
    56426, // Satellite Core - Neo Malogia (tokens)
    56427, // Satellite Core - Neo Untaris
    56428, // Satellite Core - Neo Untaris (tokens)
    56429, // Satellite Core - Neo Forax
    56430, // Satellite Core - Neo Forax (tokens)
    56431, // Satellite Core - Neo Katalund
    56432, // Satellite Core - Neo Katalund (tokens)
];

/// Whether `id` is a curated treasure crate ([`TREASURE_CRATE_TYPES`]).
pub fn is_treasure_crate_type(id: i32) -> bool {
    TREASURE_CRATE_TYPES.binary_search(&id).is_ok()
}

/// Prismimic mirror-boss object types. A Prismimic spawns *after* a dungeon's
/// main boss is defeated (SpecialSpawnOnDeath) and can appear in any dungeon, so
/// it is an appended bonus boss rather than the run's headline -- anchor
/// selection demotes it below the real boss it follows. The game gives both
/// mirror halves the same DisplayId ("Prismimic"); [`prismimic_display_name`]
/// restores the Attacker/Defender distinction so the two history rows read
/// clearly.
const PRISMIMIC_TYPES: &[i32] = &[19247, 19249];

/// Curated name -> object id overrides for `killer_sprite_id`.
///
/// Some distinct objects share a display name (different IDs), so resolving a
/// killer/drop-source name to a sprite picks the wrong object. List those names
/// here with the correct object id to resolve them authoritatively by id.
/// - "Sunken Treasure": Davy Jones' Locker chest (28065), not the Shipwreck
///   "Sunken Treasure" (34519).
/// - "King Azamoth": RealmEye's lore name for The Forgotten King (29039); no
///   game object carries this name, so alias it to the Forgotten King sprite.
/// - "MV Fishing Loot": the generic Moonlight Village fishing-loot crate; the
///   7 tier objects have no display name, so alias it to the tier-1 object.
/// - "Killer Bee Queen": The Nest boss (EH King Bee, 4243); the display name
///   otherwise resolves to the Killer Bee Queen *pet* sprite.
/// - "Calamity Crab": Deadwater Docks boss (DWD Mr Krabs, 47387); the display
///   name otherwise resolves to the Calamity Crab *pet* sprite.
/// - The alien "Satellite Core" crates all share the generic display name
///   "Satellite Core", so the per-dungeon source names can't resolve by name;
///   alias each to its dungeon's crate object. Neo and regular cores share the
///   same sprite within a dungeon (only the color differs between dungeons).
const KILLER_SPRITE_ID_OVERRIDES: &[(&str, i32)] = &[
    ("Sunken Treasure", 28065),
    ("King Azamoth", 29039),
    ("MV Fishing Loot", 20789),
    ("Killer Bee Queen", 4243),
    ("Calamity Crab", 47387),
    ("Katalund Satellite Core", 45906),
    ("Malogia Satellite Core", 45907),
    ("Untaris Satellite Core", 45908),
    ("Forax Satellite Core", 45909),
    ("Neo Katalund Satellite Core", 56431),
    ("Neo Malogia Satellite Core", 56425),
    ("Neo Untaris Satellite Core", 56427),
    ("Neo Forax Satellite Core", 56429),
];

/// The Wanderer's event-spawn object types. When a dungeon carries the
/// "Wanderer" modifier, killing its main boss spawns The Wanderer
/// (SpecialSpawnOnDeath), so it is an appended bonus boss that must never
/// headline the run -- anchor selection demotes it below the real dungeon boss
/// it follows. In its own dungeon (Hidden Interregnum) no higher-
/// tier boss is present, so it still anchors there. The MOTMG realm/dungeon-main
/// Wanderer (49755) is a real boss and is deliberately excluded.
const WANDERER_EVENT_TYPES: &[i32] = &[14474, 14477, 17391, 18346];

/// Optional modifier-spawn bosses that appear only *after* a dungeon's real
/// boss is defeated (Dimitus and the four Syndicate Takeover mercenaries).
/// Like Prismimic and the event Wanderer, they can attach to any dungeon, so
/// they must never headline the run -- anchor selection demotes them below the
/// real dungeon boss they follow. (The Key Fairy is handled as a treasure
/// crate, so it is already demoted.)
const MODIFIER_SPAWN_BOSS_TYPES: &[i32] = &[
    8813,  // Dimitus
    47414, // Dimitus Easy
    49708, // Pirate Queen Ramm (Syndicate Takeover I)
    49718, // Izel the Grand Shaman (Syndicate Takeover II)
    49730, // Nefret the Pharaoh (Syndicate Takeover III)
    49743, // Black Blade Ozuchi (Syndicate Takeover IV)
];

/// Whether `id` is an appended post-boss spawn (a Prismimic mirror half, an
/// event Wanderer, Dimitus, or a Syndicate Takeover mercenary) that must not
/// hijack a run's headline.
pub fn is_post_boss_bonus_type(id: i32) -> bool {
    PRISMIMIC_TYPES.contains(&id)
        || WANDERER_EVENT_TYPES.contains(&id)
        || MODIFIER_SPAWN_BOSS_TYPES.contains(&id)
}

/// Optional secondary bosses that share a dungeon with its true main boss and
/// often finish *after* it (a treasure-room or side boss), so anchor selection
/// would otherwise let them steal the run's headline and "Main" badge. Each is
/// dungeon-specific, so demoting them below the real main boss globally is safe;
/// where the main boss isn't tracked they still anchor over crates/adds. Event
/// reskins (mgm2 / MOTMG variants) are excluded so their standalone cards keep
/// their own main.
const OPTIONAL_SECONDARY_BOSS_TYPES: &[i32] = &[
    2227,  // The Beekeeper (The Nest) -- Killer Bee Queen is main
    3613,  // Abyss Idol (Abyss of Demons) -- Malphas is main
    8200,  // Janus the Doorwarden (Oryx's Castle) -- Stone Guardians are main
    14887, // Infernal Abyss Idol (Abyss of Demons) -- Malphas is main
    15940, // Retro Abyss Idol (Legacy Abyss of Demons) -- Malphas is main
    16889, // Swarm Tree (Woodland Labyrinth) -- Megamoth is main
    24092, // Horrific Creation (Mad Lab) -- Dr Terrible is main
    25594, // Warped Ent Ancient (Wetlands) -- Heart of the Wetlands is main
    29764, // Oryx Puppet (Puppet Master's Theatre) -- Puppet Master is main
    43924, // Cursed Phantom (Cursed Library) -- Avalon the Archivist is main
    46385, // Infested Janus the Doorwarden (Oryx's Castle) -- Stone Guardians are main
    47387, // Calamity Crab (Deadwater Docks) -- Bilgewater is main
];

/// Whether `id` is a dungeon's optional secondary boss ([`OPTIONAL_SECONDARY_BOSS_TYPES`])
/// that must never headline the run over the dungeon's true main boss.
pub fn is_optional_secondary_boss_type(id: i32) -> bool {
    OPTIONAL_SECONDARY_BOSS_TYPES.contains(&id)
}

/// The disambiguated Prismimic name for its two mirror halves (both share the
/// game DisplayId "Prismimic"), or `None` for any other type.
pub fn prismimic_display_name(id: i32) -> Option<&'static str> {
    match id {
        19247 => Some("Prismimic Attacker"),
        19249 => Some("Prismimic Defender"),
        _ => None,
    }
}

/// Whether `id` is an invulnerable-finish boss: one that never reaches 0 HP but
/// is instead scored by a completion marker spawning (a Moonlight Village dancer
/// or Umi, or a Legacy Lair of Draconis dragon whose loot balloon chest marks
/// it done). The combat tracker suspends these by type so a re-detection or the
/// self-destruct despawn folds into one fight scored when its marker fires.
pub fn is_invuln_finish_boss(id: i32) -> bool {
    COMPLETION_MARKER_MAP
        .iter()
        .any(|(_, targets)| targets.contains(&id))
}

/// Towering Perfection (Sprite Forest realm event) types. The tower repeatedly
/// splits, re-spawning its core (47909), the two Imperfection segments -- Lower
/// (47916) and Upper (47917) -- and its four Toppled Cube segments (44894 /
/// 44895 / 47914 / 47915) as fresh objects, so the combat tracker logs each wave
/// as a separate fight. Folding duplicate same-`(type, name)` phases collapses
/// them to one row per segment.
const TOWERING_PERFECTION_TYPES: &[i32] = &[44894, 44895, 47909, 47914, 47915, 47916, 47917];

/// Boss types that legitimately appear once per run but the combat tracker may
/// record several times: the Prismimic mirror halves and the Moonlight Village
/// invulnerable-finish bosses. Duplicate same-`(type, name)` history phases of
/// these types are folded into one; genuinely multi-instance bosses (Pentaract
/// towers, Nest hornets, Legion orbs) are excluded so they stay separate.
pub fn is_dedup_prone_boss(id: i32) -> bool {
    is_post_boss_bonus_type(id)
        || is_invuln_finish_boss(id)
        || TOWERING_PERFECTION_TYPES.contains(&id)
}

/// Completion markers. These objects spawn when an invulnerable-finish boss's
/// encounter is scored (a Moonlight Village progress bar fills and drops loot, or
/// a Legacy Lair of Draconis dragon self-destructs into its loot balloon chest).
/// Such bosses go invulnerable / self-destruct instead of dying (they never
/// reach 0 HP), so the combat engine would otherwise log them as "Escaped". When
/// a marker spawns the engine marks the mapped boss types as completed for the
/// current run. Each tuple is `(marker_type, &[boss_type, ...])`.
const COMPLETION_MARKER_MAP: &[(i32, &[i32])] = &[
    // MV Dungeon Complete -> the three dancers (Sage Genji, Dancer Miko, Drummer Kaguya).
    (20658, &[20450, 20451, 20452]),
    // MV Umi Complete -> Kitsune Umi (secret boss).
    (49339, &[20493]),
    // Legacy Lair of Draconis dragons self-destruct on defeat and spawn a loot
    // balloon chest; the dragon's HP never reaches 0, so it would otherwise log
    // as Escaped. Each chest spawn marks its dragon Completed.
    (30009, &[29849]), // Blue chest -> Nikao (blue)
    (30049, &[29978]), // Black chest -> Feargus (black)
    (30035, &[30017]), // Green chest -> Limoz (green)
    (30034, &[30019]), // Red chest -> Pyyr (red)
];

/// Legacy Lair of Draconis: each elemental dragon self-destructs on defeat and
/// spawns a loot balloon chest that emits the dragon's loot (the dragon drops
/// nothing directly). The chest is the loot source, so its bags must correlate
/// back to the dragon's Combat History card. `(dragon_type, chest_type)`, kept
/// sorted by dragon type.
const LOD_DRAGON_CHEST_PAIRS: &[(i32, i32)] = &[
    (29849, 30009), // Nikao (blue)
    (29978, 30049), // Feargus (black)
    (30017, 30035), // Limoz (green)
    (30019, 30034), // Pyyr (red)
];

/// The loot-emitter (chest) type whose bags belong to boss `boss_type`'s Combat
/// History card, or `None` when the boss emits its own loot. Used to correlate a
/// self-destructing boss's chest drops back to its fight.
pub fn loot_emitter_for_boss(boss_type: i32) -> Option<i32> {
    LOD_DRAGON_CHEST_PAIRS
        .iter()
        .find(|(dragon, _)| *dragon == boss_type)
        .map(|(_, chest)| *chest)
}

/// The boss whose card a `chest_type` loot-emitter's bags should be attributed
/// to, or `None` when `chest_type` is not a known loot-emitter proxy. The
/// inverse of [`loot_emitter_for_boss`].
pub fn boss_for_loot_emitter(chest_type: i32) -> Option<i32> {
    LOD_DRAGON_CHEST_PAIRS
        .iter()
        .find(|(_, chest)| *chest == chest_type)
        .map(|(dragon, _)| *dragon)
}

/// The Legacy Lair of Draconis `(dragon_type, chest_type)` pairs, for the combat
/// database's loot-completion backfill.
pub fn lod_dragon_chest_pairs() -> &'static [(i32, i32)] {
    LOD_DRAGON_CHEST_PAIRS
}

/// Legacy Lair of Draconis final boss. Its portal only drops inside the Lair
/// once all four elemental dragons are defeated, so an Ivory Wyvern fight proves
/// the immediately preceding Lair run was a full clear.
pub const LEGACY_LOD_IVORY_BOSS: i32 = 30026;

/// Whether `object_type` is the Legacy Lair of Draconis Ivory Wyvern final boss.
pub fn is_legacy_lod_ivory_boss(object_type: i32) -> bool {
    object_type == LEGACY_LOD_IVORY_BOSS
}

/// The boss object types a completion marker signals as cleared, or `None` when
/// `id` is not a known completion marker ([`COMPLETION_MARKER_MAP`]).
fn completion_marker_targets_for(id: i32) -> Option<&'static [i32]> {
    COMPLETION_MARKER_MAP
        .iter()
        .find(|(marker, _)| *marker == id)
        .map(|(_, targets)| *targets)
}

/// Seasonal / special event bosses grouped under the "Seasonal and Special
/// bosses" Combat History category. These are classified by object
/// type (not name) so every difficulty variant lands in the group regardless of
/// which dungeon or realm it spawns in. Keep sorted by id.
const SPECIAL_BOSS_TYPES: &[i32] = &[
    8813,  // Dimitus
    19247, // Prismimic Attacker
    19249, // Prismimic Defender
    38282, // Pitchfork Pete
    38283, // Sticky Fudgefoot
    38284, // Crackjaw Gregg
    42255, // The Hemomancer
    42371, // Jotunn
    44020, // The Glitch
    47414, // Dimitus Easy
    49708, // Pirate Queen Ramm (Mercenary A)
    49718, // Izel the Grand Shaman (Mercenary B)
    49730, // Nefret the Pharaoh (Mercenary C)
    49743, // Black Blade Ozuchi (Mercenary D)
    49755, // The Wanderer (MOTMG event boss; the Interregnum dungeon Wanderers
           // 14474/14477/17391/18346 stay classified by their Exaltation dungeon)
];

/// Whether `id` is a curated seasonal/special boss ([`SPECIAL_BOSS_TYPES`]).
fn is_special_boss_type(id: i32) -> bool {
    SPECIAL_BOSS_TYPES.binary_search(&id).is_ok()
}

/// The character tier a player gravestone represents, decoded from the grave
/// object's in-game name. Fame graves (`1/8` .. `8/8`) are always Level 20.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraveTier {
    /// The character level bucket: "1", "2-19", or "20".
    pub level: &'static str,
    /// The fame star rank (`1..=8`) for a fame grave, else `None`.
    pub fame: Option<u8>,
}

/// Decode a gravestone object's in-game name into its [`GraveTier`]. Names take
/// the form "<tier prefix> <skin> Gravestone", where the prefix is one of
/// "Level 1", "Level 2-19", "Level 20", or "N/8" (N in 1..=8). Returns `None`
/// for names that don't start with a recognized tier prefix.
pub fn parse_grave_tier(name: &str) -> Option<GraveTier> {
    let name = name.trim();
    if let Some(rest) = name.strip_prefix("Level ") {
        let level = match rest.split_whitespace().next()? {
            "1" => "1",
            "2-19" => "2-19",
            "20" => "20",
            _ => return None,
        };
        return Some(GraveTier { level, fame: None });
    }
    // Fame graves: "N/8 <skin> Gravestone" -- always a Level 20 character.
    let (num, denom) = name.split_whitespace().next()?.split_once('/')?;
    if denom == "8" {
        if let Ok(fame @ 1..=8) = num.parse::<u8>() {
            return Some(GraveTier {
                level: "20",
                fame: Some(fame),
            });
        }
    }
    None
}

/// A multi-phase boss encounter whose constituent phase-bosses should be grouped
/// into a single Combat History card headed by a canonical name.
///
/// Each phase boss finalizes as its own fight (they die at different times); the
/// combat engine tags every member fight with this encounter's [`Encounter::id`]
/// so the UI can fold them under one card and show a per-phase breakdown.
#[derive(Debug, Clone, Copy)]
pub struct Encounter {
    /// Stable identifier persisted with each member fight.
    pub id: &'static str,
    /// Canonical display name for the grouped card.
    pub display_name: &'static str,
    /// The final loot-dropping form; its kill defines the encounter's `killed`.
    pub anchor_type: i32,
    /// Every phase-boss object type that belongs to this encounter (incl. anchor).
    pub member_types: &'static [i32],
}

/// Curated registry of multi-phase encounters. Seeded with Cultist
/// Hideout; expand incrementally after a capture confirms each member type-id.
const ENCOUNTERS: &[Encounter] = &[
    Encounter {
        id: "cultist_hideout",
        display_name: "Malus",
        anchor_type: 45231, // LH Malus 2 (final TRACKLOOT form)
        member_types: &[
            45215, // Malus
            45217, // Argus
            45219, // Gaius
            45221, // Basaran
            45223, // Dirge
            45225, // Molek
            45227, // Balaam
            45228, // Balaam 1
            45229, // Balaam 2
            45230, // Balaam 3
            45231, // Malus 2 (anchor)
        ],
    },
    Encounter {
        id: "astral_rift",
        display_name: "Astral Rift",
        anchor_type: 51061, // Astral Rift (realm event boss)
        member_types: &[
            51061, // Astral Rift
            51062, // Astral Guard (aggregated spawns repr)
        ],
    },
    Encounter {
        id: "hermit_god",
        display_name: "Hermit God",
        anchor_type: 3425, // Hermit God (realm event boss)
        member_types: &[
            3425,  // Hermit God (legacy)
            3428,  // Hermit God Tentacle (aggregated tentacle repr)
            22023, // New Hermit God
        ],
    },
    Encounter {
        id: "the_plague_doctor",
        display_name: "The Plague Doctor",
        anchor_type: 34489, // The Plague Doctor (realm event boss)
        member_types: &[
            34489, // The Plague Doctor
            34490, // Lethal Cure (aggregated cure repr)
        ],
    },
    Encounter {
        id: "crystal_cavern",
        display_name: "Crystal Entity",
        anchor_type: 10025, // Crystal Entity (final loot-dropping boss)
        member_types: &[
            10003, // Crystallised Monstrosity
            10004, // Crystallised Fish
            10005, // Crystallised Cyclops
            10006, // Crystallised Scorpion
            10025, // Crystal Entity (anchor)
        ],
    },
    Encounter {
        id: "fungal_cavern",
        display_name: "Crystal Worm Mother",
        anchor_type: 45712, // GC Boss (Crystal Worm Mother, loot form)
        // Fungal-only GC Boss family plus the "CC Crystal Worm Child" spawn
        // (5534-5536): despite the "CC" id prefix it is defined in
        // fungalCavernObjects.xml and spawns in this fight. Both the Crystal Worm
        // Father (45711) and Child are curated out as standalone fights and
        // aggregated into this card via AUX_TARGETS.
        member_types: &[
            5534,  // CC Crystal Worm Child (aux repr for the aggregated child row)
            45711, // GC Boss Support (Crystal Worm Father, aux repr for the father row)
            45712, // GC Boss (Crystal Worm Mother, anchor)
            45713, // GC Boss Body
            45714, // GC Boss Body Weakpoint
            45715, // GC Boss Body Bomb
            45719, // GC Boss Tail
            45729, // GC Boss Child (Crystal Worm Child spawn)
            45730, // GC Boss Child Body
            45731, // GC Boss Child Tail
        ],
    },
    Encounter {
        id: "marble_colossus",
        display_name: "Marble Colossus",
        anchor_type: 45073, // LH Marble Colossus (loot form)
        // Colossus body/forms plus a representative Core and Pillar type so the
        // aggregated Core/Pillar summary fights group into this same card.
        member_types: &[
            45073, // LH Marble Colossus (anchor)
            45074, // LH Marble Colossus Anchor
            45253, // LH Marble Colossus Laser A
            45254, // LH Marble Colossus Laser B
            45116, // LH Marble Core 2 (Core summary representative)
            45107, // LH Colossus Pillar (Pillar summary representative)
        ],
    },
    Encounter {
        id: "nest",
        display_name: "Killer Bee Queen",
        anchor_type: 4243, // EH King Bee (Killer Bee Queen, loot form)
        // The Queen plus every per-color Adolescent Beehemoth and Killer Bee Nest
        // she is fought alongside, so their aggregated summaries group into this
        // card. Covers both The Nest and its advanced Plagued Nest (green forms).
        member_types: &[
            4243,  // EH King Bee (Killer Bee Queen, anchor)
            4285,  // Adolescent Yellow Beehemoth
            4286,  // Adolescent Red Beehemoth
            4287,  // Adolescent Blue Beehemoth
            17627, // Adolescent Green Beehemoth (Plagued)
            4278,  // EH Queen Blue Hive (Killer Bee Nest)
            4279,  // EH Queen Red Hive (Killer Bee Nest)
            4280,  // EH Queen Yellow Hive (Killer Bee Nest)
            17503, // EH Queen Green Hive (Killer Bee Nest, Plagued)
        ],
    },
    Encounter {
        id: "kogbold_steamworks",
        display_name: "Factory Control Core",
        anchor_type: 50391, // KSW Factory Control Core (loot form)
        // The Core plus the Shield Generator representative so the aggregated
        // generator summary groups into this card. Covers both Kogbold Steamworks
        // and Advanced Kogbold Steamworks (same object types).
        member_types: &[
            50391, // KSW Factory Control Core (anchor)
            50388, // KSW Shield Generator (summary representative)
        ],
    },
    // Spectral Penitentiary: only the three bosses that need extra tracking are
    // curated. Gretch and Zole auto-detect cleanly as plain fights (their helpers
    // are label-less). HM Loot forms are folded so Advanced runs group correctly.
    Encounter {
        id: "spec_pen_lobotomik",
        display_name: "Doctor Lobotomik",
        anchor_type: 23920, // SpecPen Doctor Lobotomik (loot form)
        // The doctor plus every separate "animal transformation" object (each a
        // MINIBOSS with its own HP bar that would otherwise be its own card).
        member_types: &[
            23920, // Doctor Lobotomik (anchor)
            23622, // Lobotomik HM Loot
            23958, // LBT Transformation 1
            23959, // LBT Transformation 2
            23960, // LBT Transformation 3
            23961, // LBT Transformation 4
            23934, // LBT Transformation 4B
            23935, // LBT Transformation 4T
        ],
    },
    Encounter {
        id: "spec_pen_oculon",
        display_name: "Overseer Oculon",
        anchor_type: 24071, // SpecPen Overseer Oculon (loot form)
        member_types: &[
            24071, // Overseer Oculon (anchor)
            23513, // Oculon HM Loot
            23509, // Overseer Eyesmall (summary representative)
        ],
    },
    Encounter {
        id: "spec_pen_murcian",
        display_name: "Soulwarden Murcian",
        anchor_type: 23681, // SpecPen Soulwarden Murcian (loot form)
        member_types: &[
            23681, // Soulwarden Murcian (anchor)
            44368, // Murcian HM Loot
            23804, // Spectral Key (summary representative)
        ],
    },
    Encounter {
        id: "ice_citadel",
        display_name: "Esben the Neurotic",
        anchor_type: 40166, // ic Esben the Neurotic (loot form)
        // Esben plus his two summoned sub-boss forms, each a separate 66.6k-HP
        // entity with its own health bar and display name (Munin = bird form,
        // Wolfie/Fenrir = wolf form). They auto-detect as their own cards via the
        // HP fallback, so grouping folds them under Esben as phase rows.
        member_types: &[
            40166, // Esben the Neurotic (anchor)
            40177, // Munin (bird form)
            40191, // Wolfie / Fenrir (wolf form)
        ],
    },
    // The Shatters: only the Twilight Archmage needs extra tracking. Bridge
    // Sentinel and The Forgotten King auto-detect cleanly as their own cards
    // (the King's clones take no player damage, so they never form a fight).
    Encounter {
        id: "twilight_archmage",
        display_name: "Twilight Archmage",
        anchor_type: 29021, // shtrs Twilight Archmage
        // The Archmage plus his three "Archmage Phoenix" bird spawns and the
        // arena generators, so their aggregated summaries group into this card.
        member_types: &[
            29021, // Twilight Archmage (anchor)
            29341, // Inferno (bird, summary representative)
            29342, // Blizzard (bird, summary representative)
            17490, // Tempest (bird, hardmode; summary representative)
            33054, // Generator (summary representative)
        ],
    },
    // Legacy The Shatters: the Twilight Archmage plus his two "Retro Archmage
    // Phoenix" spawns (Blizzard/Inferno), each aggregated into this card via
    // AUX_TARGETS. The Forgotten Sentinel and the King (52560) are allow-listed
    // as their own phase rows in the dungeon card.
    Encounter {
        id: "legacy_twilight_archmage",
        display_name: "Twilight Archmage",
        anchor_type: 52557, // Retro shtrs Twilight Archmage
        member_types: &[
            52557, // Retro shtrs Twilight Archmage (anchor)
            52563, // Retro shtrs Inferno (bird, summary representative)
            52564, // Retro shtrs Blizzard (bird, summary representative)
        ],
    },
    // Legacy The Shatters boss 1 (The Forgotten Sentinel): the six bridge guards
    // must be cleared before the Sentinel becomes vulnerable. The four Titanums
    // fold into one aux row and the two Paladin Obelisks into another (see
    // AUX_TARGETS), so their repr types resolve to this encounter and damaging
    // them then leaving before the Sentinel is hit still records the fight
    // (Escaped) on the Sentinel card.
    Encounter {
        id: "legacy_forgotten_sentinel",
        display_name: "The Forgotten Sentinel",
        anchor_type: 52556, // Retro shtrs Bridge Sentinel
        member_types: &[
            52556, // The Forgotten Sentinel (anchor)
            52532, // Titanum of Hate (Bridge Obelisk A)
            52533, // Titanum of Despair (Bridge Obelisk B)
            52535, // Titanum of Lies (Bridge Obelisk D)
            52536, // Titanum of Cruelty (Bridge Obelisk E)
            52534, // Paladin Obelisk (Bridge Obelisk C)
            52537, // Paladin Obelisk (Bridge Obelisk F)
        ],
    },
    // Legacy The Shatters boss 3 (The Forgotten King): the two Royal Guardians L
    // and the four colored crystals gate the King's vulnerability. The Guardians
    // fold into one aux row and each crystal into its own row (see AUX_TARGETS),
    // so damaging them then leaving before the King is hit still records the
    // fight (Escaped) on the King card. Royal Guardian J spawns only after the
    // King is already vulnerable, so it stays hidden (curated non-boss).
    Encounter {
        id: "legacy_forgotten_king",
        display_name: "The Forgotten King",
        anchor_type: 52560, // Retro shtrs The Forgotten King (final)
        member_types: &[
            52560, // The Forgotten King (anchor)
            52558, // Royal Guardian L
            52572, // Green Crystal
            52573, // Yellow Crystal
            52574, // Red Crystal
            52575, // Blue Crystal
        ],
    },
    // self-destructing forms (Larva -> Mammoth Megamoth -> Murderous Megamoth),
    // each its own high-HP entity that auto-detects as a card. Grouping folds
    // them under the final Murderous Megamoth so killing it marks every phase
    // Completed, and a nexus before it leaves them all Escaped.
    Encounter {
        id: "legacy_murderous_megamoth",
        display_name: "Murderous Megamoth",
        anchor_type: 52665, // Retro Murderous Megamoth (final form, BOSS label)
        member_types: &[
            52662, // Retro Megamoth Larva (phase 1)
            52663, // Retro Mammoth Megamoth (phase 2)
            52665, // Retro Murderous Megamoth (final, anchor)
        ],
    },
    // Forax / Neo Forax: the "Waste" miniboss starts as one body then splits into
    // two smaller identical copies. All three are MINIBOSS+TRACKLOOT and auto-
    // detect as separate cards, so grouping folds each trio into one card.
    Encounter {
        id: "forax_waste",
        display_name: "Waste",
        anchor_type: 45917, // AI Waste (main body, full HP bar / icon)
        member_types: &[
            45917, // AI Waste (main, anchor)
            56070, // AI Waste Split A
            56071, // AI Waste Split B
        ],
    },
    Encounter {
        id: "neo_forax_waste",
        display_name: "Neo Waste",
        anchor_type: 56390, // AI Neo Waste (main body, full HP bar / icon)
        member_types: &[
            56390, // AI Neo Waste (main, anchor)
            56391, // AI Neo Waste Split A
            56392, // AI Neo Waste Split B
        ],
    },
    // Oryx's Sanctuary: the five bosses (Beisa, Gemsbok, Leucoryx, Dammah, Oryx 3)
    // all auto-detect as their own MINIBOSS/BOSS+TRACKLOOT cards. Only two need an
    // encounter, purely to host their aggregated add summaries.
    Encounter {
        id: "o3_leucoryx",
        display_name: "Archbishop Leucoryx",
        anchor_type: 6622, // Archbishop Leucoryx
        member_types: &[
            6622, // Archbishop Leucoryx (anchor)
            6617, // Orb of Light (summary representative)
            6618, // Orb of Chaos (summary representative)
        ],
    },
    Encounter {
        id: "o3_oryx",
        display_name: "Oryx the Mad God",
        anchor_type: 45363, // Oryx the Mad God 3
        member_types: &[
            45363, // Oryx the Mad God 3 (anchor)
            45365, // Messenger of Wrath (Messengers summary representative)
        ],
    },
    // Mountain Temple: Daichi the Fallen summons the four elementals during his
    // fight (they are MINIBOSS but carry no TRACKLOOT), so they auto-detect as
    // their own cards. Group them under Daichi as phase rows.
    Encounter {
        id: "mountain_temple",
        display_name: "Daichi the Fallen",
        anchor_type: 43681, // Daichi the Fallen (loot form)
        member_types: &[
            43681, // Daichi the Fallen (anchor)
            43696, // Fire Elemental
            43698, // Air Elemental
            43700, // Water Elemental
            43701, // Earth Elemental
        ],
    },
    // Lair of Shaitan: Shaitan the Advisor (the Head) fought alongside his Left/
    // Right Hands, which are aggregated into one "Hand of Shaitan" summary row on
    // this card. Covers both dungeon variants (md2 current anchor, md1 legacy).
    Encounter {
        id: "lair_of_shaitan",
        display_name: "Shaitan the Advisor",
        anchor_type: 28058, // md2 Head of Shaitan (loot form)
        member_types: &[
            28058, // Head of Shaitan (md2, anchor)
            29723, // Head of Shaitan (md1 legacy)
            28059, // Hand of Shaitan (summary representative)
        ],
    },
    // --- Realm set-piece event encounters ---
    // World's Oyster: the Oyster is the real boss; its two World's Pearls are
    // aggregated into one summary row. Corals are ignored (spawner curated
    // non-boss; the small/large corals are GameObjects, never fights).
    Encounter {
        id: "worlds_oyster",
        display_name: "World's Oyster",
        anchor_type: 34538, // World's Oyster
        member_types: &[
            34538, // World's Oyster (anchor)
            34546, // World's Pearl (summary representative)
        ],
    },
    // Jade and Garnet Statues: two twin minibosses fought together. Both are real
    // bosses (separate phase rows); the card is always titled by the encounter.
    Encounter {
        id: "jade_garnet_statues",
        display_name: "Jade and Garnet Statues",
        anchor_type: 22149, // New Jade Statue
        member_types: &[
            22149, // New Jade Statue (anchor)
            22150, // New Garnet Statue
            28619, // Jade Statue (legacy)
            28618, // Garnet Statue (legacy)
        ],
    },
    // Flying Behemoth: the Behemoth and its Egg are both real bosses shown as two
    // phase rows in one card; the Behemoth is the anchor (icon/completion).
    Encounter {
        id: "flying_behemoth",
        display_name: "Flying Behemoth",
        anchor_type: 20744, // Flying Behemoth
        member_types: &[
            20744, // Flying Behemoth (anchor)
            20799, // Behemoth's Egg
        ],
    },
    // Crab Sovereign: the Sovereign is the real boss; its Royal Crabs are
    // aggregated into one summary row.
    Encounter {
        id: "crab_sovereign",
        display_name: "Crab Sovereign",
        anchor_type: 51072, // Crab Sovereign
        member_types: &[
            51072, // Crab Sovereign (anchor)
            51073, // Royal Crab (summary representative)
        ],
    },
    // Killer Bee Nest (realm event): the event Hive plus its three Beehemoths
    // (each a real boss / phase row) and the aggregated Mini Killer Bee Nests.
    // The card counts as killed if any Beehemoth was killed (see COMPLETION_ANY).
    Encounter {
        id: "killer_bee_nest",
        display_name: "Killer Bee Nest",
        anchor_type: 4312, // EH Event Hive (Killer Bee Nest)
        member_types: &[
            4312, // Killer Bee Nest (event hive, anchor)
            4324, // Yellow Beehemoth
            4325, // Red Beehemoth
            4326, // Blue Beehemoth
            4238, // Mini Killer Bee Nest (summary representative)
        ],
    },
    // Avatar of the Forgotten King (Shatters final boss): its Killer Pillars,
    // Shades of the Avatar and Eyes of the Avatar are each aggregated into one
    // summary row. Also groups inside the Shatters dungeon card.
    Encounter {
        id: "avatar_forgotten_king",
        display_name: "Avatar of the Forgotten King",
        anchor_type: 21959, // New shtrs Defense System (Avatar)
        member_types: &[
            21959, // Avatar of the Forgotten King (new, anchor)
            29517, // Avatar of the Forgotten King (legacy)
            21967, // Killer Pillar (summary representative)
            21963, // Shades of the Avatar (summary representative)
            21976, // Eye of the Avatar (summary representative)
            21964, // ShadowBombs (summary representative)
        ],
    },
    // Towering Perfection (Sprite Forest realm event): the central Towering
    // Perfection core plus its damage-sponge segments -- the Lower / Upper
    // Imperfection halves and the four Toppled Cubes -- each shown as its own
    // phase row folded into one card. The core has low HP and is often never
    // seen dying, so completion also latches on a loot bag dropped by the core
    // (see LOOT_COMPLETES_ENCOUNTERS). The card is headline-only, so its name and
    // picture are always the core's even when only segments took damage.
    Encounter {
        id: "towering_perfection",
        display_name: "Towering Perfection",
        anchor_type: 47909, // Towering Perfection (0xbb25, core)
        member_types: &[
            47909, // Towering Perfection (0xbb25, anchor / core)
            47916, // Tower Bottom (0xbb2c, Lower Imperfection)
            47917, // Tower Top (0xbb2d, Upper Imperfection)
            44894, // Tower Red Cube (0xaf5e, Toppled Red Cube)
            44895, // Tower Yellow Cube (0xaf5f, Toppled Yellow Cube)
            47914, // Tower Green Cube (0xbb2a, Toppled Green Cube)
            47915, // Tower Blue Cube (0xbb2b, Toppled Blue Cube)
        ],
    },
    // Sentient Monolith: the Monolith is the real boss; its Siphons are
    // aggregated into one summary row.
    Encounter {
        id: "sentient_monolith",
        display_name: "Sentient Monolith",
        anchor_type: 16935, // Sentient Monolith
        member_types: &[
            16935, // Sentient Monolith (anchor)
            16936, // Monolith Siphon (summary representative)
        ],
    },
    // Eye of the Storm: the Eye is the real boss; its Tornadoes are aggregated
    // into one summary row.
    Encounter {
        id: "eye_of_the_storm",
        display_name: "Eye of the Storm",
        anchor_type: 51031, // Eye of the Storm
        member_types: &[
            51031, // Eye of the Storm (anchor)
            51033, // Eye of the Storm Tornado (summary representative)
        ],
    },
    // Pentaract: the five Towers are shown as separate phase rows collapsed into
    // one card. The invisible Pentaract marker never forms a fight; the card
    // counts as killed if any Tower was killed (see COMPLETION_ANY).
    Encounter {
        id: "pentaract",
        display_name: "Pentaract",
        anchor_type: 22019, // New Pentaract (invisible marker)
        member_types: &[
            22019, // New Pentaract (marker)
            3423,  // Pentaract (legacy marker)
            1337,  // Pentaract Tower (legacy, Oryx Tower)
            3422,  // Pentaract Tower (legacy)
            22018, // New Pentaract Tower
        ],
    },
    // Slumbering Dragon (realm event): the Dragon and its Dragon's Treasure are
    // folded into one card titled after the Dragon (the treasure is a phase row,
    // not a standalone card). Both ship with empty labels; classified as Adept
    // Heroes of Oryx. Card counts as killed if either was killed (COMPLETION_ANY).
    Encounter {
        id: "slumbering_dragon",
        display_name: "Slumbering Dragon",
        anchor_type: 34598, // Slumbering Dragon
        member_types: &[
            34598, // Slumbering Dragon (anchor)
            34599, // Dragon's Treasure
        ],
    },
    // Assembled Giant: its Head, Body and Arm parts are shown as separate phase
    // rows in one card. The Body is the anchor -> the card is killed if the Body
    // was killed.
    Encounter {
        id: "assembled_giant",
        display_name: "Assembled Giant",
        anchor_type: 34485, // Assembled Giant Body
        member_types: &[
            34480, // Assembled Giant Head
            34485, // Assembled Giant Body (anchor)
            34486, // Assembled Giant Arm
        ],
    },
    // Animal Merchant (Adept Hero of Oryx realm set-piece): the Merchant is the
    // real boss (becomes vulnerable and dies after its spawns), folded into one
    // card with its Criminal Monkey and Fraudulent Tiger spawns. All three ship
    // with empty labels, so they are curated bosses; the Merchant anchor is in
    // the Adept Hero catalog and its kill defines the card's completion.
    Encounter {
        id: "animal_merchant",
        display_name: "Animal Merchant",
        anchor_type: 24023, // Animal Merchant (anchor)
        member_types: &[
            24023, // Animal Merchant (anchor)
            24020, // Criminal Monkey
            24021, // Fraudulent Tiger
        ],
    },
    // Hornet's Nest (Veteran Hero of Oryx, Floral Escape set-piece): the Nest is
    // the real loot-dropping boss; the Angry Hornets circling it are aggregated
    // into one summary row inside its card.
    Encounter {
        id: "hornets_nest",
        display_name: "Hornet's Nest",
        anchor_type: 34603, // Hornet's Nest (miniboss, loot form, anchor)
        member_types: &[
            34603, // Hornet's Nest (anchor)
            34604, // Angry Hornet (summary representative)
        ],
    },
    // Legion Missionary (Sanguine Forest Beacon Guardian, realm set-piece): the
    // Missionary is the real loot-dropping miniboss; its Holy and Chaos Orbs are
    // aggregated into one summary row inside its card.
    Encounter {
        id: "legion_missionary",
        display_name: "Legion Missionary",
        anchor_type: 53013, // Legion Missionary (miniboss anchor)
        member_types: &[
            53013, // Legion Missionary (anchor)
            53017, // Legion Missionary Holy Orb (summary representative)
            53018, // Legion Missionary Chaos Orb
        ],
    },
    // Tomb of the Ancients: three main bosses (Nut, Geb, Bes) plus the two
    // Sarcophagi. The dungeon only counts as completed once ALL THREE bosses are
    // defeated (the Tomb Mark and exit portal drop on the third kill), so it uses
    // the all-of completion rule in COMPLETION_ALL. Both Sarcophagi are listed so
    // a card that only opened a Sarcophagus still resolves to this encounter and
    // is correctly marked Escaped rather than Completed.
    Encounter {
        id: "tomb_of_the_ancients",
        display_name: "Tomb of the Ancients",
        anchor_type: 3367, // Geb
        member_types: &[
            3366, // Nut
            3367, // Geb
            3368, // Bes
            3369, // Active Sarcophagus (gate miniboss)
            3372, // Treasure Sarcophagus (loot chest)
        ],
    },
];

/// Bosses that revive with a full HP pool for a distinct second phase under the
/// same object id (the Marble Colossus survival phase). Damage tracking splits
/// such a fight into a pre-survival and post-survival segment rather than folding
/// the heal-back into one fight.
pub fn is_second_coming_boss(boss_type: i32) -> bool {
    matches!(boss_type, 45073)
}

/// The Marble Colossus emits a fixed taunt (via its own `TextPacket`) when it
/// leaves the invulnerable survival phase, heals to full and begins its second
/// coming. This is the authoritative, state-machine signal to split the fight
/// into pre- and post-survival segments -- far more reliable than watching for
/// a heal-back HP tick, which is often culled. (`"No, I cannot..."` marks the
/// entry into survival; `"...!"` marks its end.)
pub fn is_second_coming_transition_taunt(text: &str) -> bool {
    text.trim() == "...!"
}

/// A non-boss damage target whose damage is rolled up into a single per-run
/// summary entry (Marble Cores, Marble Colossus Pillars) grouped into the boss's
/// encounter card.
#[derive(Debug, Clone, Copy)]
pub struct AuxTarget {
    /// Stable category key (one aggregate per category per run).
    pub category: &'static str,
    /// Display name for the summary entry.
    pub display_name: &'static str,
    /// Representative object type (drives encounter grouping via `member_types`).
    pub repr_type: i32,
}

const AUX_TARGETS: &[(&[i32], AuxTarget)] = &[
    (
        // LH Marble Core 1-8.
        &[45367, 45116, 45117, 45372, 45119, 45120, 45136, 45137],
        AuxTarget {
            category: "marble_core",
            display_name: "Marble Core",
            repr_type: 45116,
        },
    ),
    (
        // LH Colossus Pillar + LH Halls Colossus Pillar.
        &[45107, 45387],
        AuxTarget {
            category: "marble_pillar",
            display_name: "Marble Colossus Pillar",
            repr_type: 45107,
        },
    ),
    // The Nest / Plagued Nest: per-color Adolescent Beehemoths and Killer Bee
    // Nests, each summed into its own row inside the Killer Bee Queen card.
    (
        &[4285],
        AuxTarget {
            category: "nest_beehemoth_yellow",
            display_name: "Adolescent Yellow Beehemoth",
            repr_type: 4285,
        },
    ),
    (
        &[4286],
        AuxTarget {
            category: "nest_beehemoth_red",
            display_name: "Adolescent Red Beehemoth",
            repr_type: 4286,
        },
    ),
    (
        &[4287],
        AuxTarget {
            category: "nest_beehemoth_blue",
            display_name: "Adolescent Blue Beehemoth",
            repr_type: 4287,
        },
    ),
    (
        &[17627],
        AuxTarget {
            category: "nest_beehemoth_green",
            display_name: "Adolescent Green Beehemoth",
            repr_type: 17627,
        },
    ),
    (
        &[4280],
        AuxTarget {
            category: "nest_hive_yellow",
            display_name: "Yellow Killer Bee Nest",
            repr_type: 4280,
        },
    ),
    (
        &[4279],
        AuxTarget {
            category: "nest_hive_red",
            display_name: "Red Killer Bee Nest",
            repr_type: 4279,
        },
    ),
    (
        &[4278],
        AuxTarget {
            category: "nest_hive_blue",
            display_name: "Blue Killer Bee Nest",
            repr_type: 4278,
        },
    ),
    (
        &[17503],
        AuxTarget {
            category: "nest_hive_green",
            display_name: "Green Killer Bee Nest",
            repr_type: 17503,
        },
    ),
    // Kogbold Steamworks / Advanced: all Shield Generators summed into one row
    // inside the Factory Control Core card.
    (
        &[50388],
        AuxTarget {
            category: "shield_generator",
            display_name: "Shield Generator",
            repr_type: 50388,
        },
    ),
    // Spectral Penitentiary: Oculon's Overseer Eyesmall minions summed together.
    (
        &[23509, 44197],
        AuxTarget {
            category: "overseer_eyesmall",
            display_name: "Overseer Eyesmall",
            repr_type: 23509,
        },
    ),
    // Spectral Penitentiary: Murcian's two sets of Spectral Keys summed together.
    (
        &[23804],
        AuxTarget {
            category: "spectral_key",
            display_name: "Spectral Key",
            repr_type: 23804,
        },
    ),
    // The Shatters: Twilight Archmage's three "Archmage Phoenix" birds, each
    // summed into its own row, plus all arena generators summed into one row.
    (
        &[29341],
        AuxTarget {
            category: "archmage_inferno",
            display_name: "Inferno",
            repr_type: 29341,
        },
    ),
    (
        &[29342],
        AuxTarget {
            category: "archmage_blizzard",
            display_name: "Blizzard",
            repr_type: 29342,
        },
    ),
    (
        &[17490],
        AuxTarget {
            category: "archmage_tempest",
            display_name: "Tempest",
            repr_type: 17490,
        },
    ),
    (
        // Shatters Generator TL/TR/BL/BR + Source Generator TL/TR/BL/BR.
        &[33054, 33055, 33056, 33057, 33069, 33070, 33071, 33072],
        AuxTarget {
            category: "archmage_generators",
            display_name: "Twilight Archmage Generators",
            repr_type: 33054,
        },
    ),
    // Legacy The Shatters: the Twilight Archmage's two "Retro Archmage Phoenix"
    // spawns, each summed into its own row inside the Twilight Archmage card.
    (
        &[52563],
        AuxTarget {
            category: "legacy_archmage_inferno",
            display_name: "Inferno",
            repr_type: 52563,
        },
    ),
    (
        &[52564],
        AuxTarget {
            category: "legacy_archmage_blizzard",
            display_name: "Blizzard",
            repr_type: 52564,
        },
    ),
    // Legacy The Shatters (boss 1, Forgotten Sentinel gate): the four Bridge
    // Titanums summed into one row and the two Paladin Obelisks into another,
    // inside the Forgotten Sentinel card.
    (
        &[52532, 52533, 52535, 52536],
        AuxTarget {
            category: "legacy_bridge_titanum",
            display_name: "Titanum",
            repr_type: 52532,
        },
    ),
    (
        &[52534, 52537],
        AuxTarget {
            category: "legacy_paladin_obelisk",
            display_name: "Paladin Obelisk",
            repr_type: 52534,
        },
    ),
    // Legacy The Shatters (boss 3, Forgotten King gate): the two Royal Guardians
    // L summed into one row, and each colored crystal into its own row, inside
    // the Forgotten King card. (Royal Guardian J is hidden; it spawns only once
    // the King is already vulnerable.)
    (
        &[52558],
        AuxTarget {
            category: "legacy_royal_guardian",
            display_name: "Royal Guardian",
            repr_type: 52558,
        },
    ),
    (
        &[52572],
        AuxTarget {
            category: "legacy_green_crystal",
            display_name: "Green Crystal",
            repr_type: 52572,
        },
    ),
    (
        &[52573],
        AuxTarget {
            category: "legacy_yellow_crystal",
            display_name: "Yellow Crystal",
            repr_type: 52573,
        },
    ),
    (
        &[52574],
        AuxTarget {
            category: "legacy_red_crystal",
            display_name: "Red Crystal",
            repr_type: 52574,
        },
    ),
    (
        &[52575],
        AuxTarget {
            category: "legacy_blue_crystal",
            display_name: "Blue Crystal",
            repr_type: 52575,
        },
    ),
    // Oryx's Sanctuary: Leucoryx's two orb types (each summed into its own row)
    // and Oryx 3's three Messengers (all summed into one row).
    (
        &[6617],
        AuxTarget {
            category: "o3_orb_light",
            display_name: "Orb of Light",
            repr_type: 6617,
        },
    ),
    (
        &[6618],
        AuxTarget {
            category: "o3_orb_chaos",
            display_name: "Orb of Chaos",
            repr_type: 6618,
        },
    ),
    (
        &[45365, 45431, 45445],
        AuxTarget {
            category: "o3_messengers",
            display_name: "Messengers",
            repr_type: 45365,
        },
    ),
    // Lair of Shaitan: every Left/Right Hand of Shaitan (a large part of the boss
    // fight) summed into one row alongside the Head of Shaitan card. Covers both
    // dungeon variants (md2 current, md1 legacy); only one set spawns per run.
    // Excludes the invisible landspawner/playerlock (28167/28177) and the fake
    // decoy hands (29729/29730), which take no player damage.
    (
        &[28059, 28060, 28061, 29724, 29725, 29739, 29740],
        AuxTarget {
            category: "shaitan_hands",
            display_name: "Hand of Shaitan",
            repr_type: 28059,
        },
    ),
    // Astral Rift (realm event): the Astral Guard and Astral Beast spawns are
    // summed into a single "Astral Rift Spawns" row instead of spamming a card
    // per spawn.
    (
        &[51062, 51063],
        AuxTarget {
            category: "astral_rift_spawns",
            display_name: "Astral Rift Spawns",
            repr_type: 51062,
        },
    ),
    // Hermit God (realm event): every Hermit God Tentacle (both the legacy and
    // "New" variants) summed into one row alongside the Hermit God card. Minions,
    // whirlpools and the invisible tentacle spawners are hidden (curated non-boss).
    (
        &[3428, 22026],
        AuxTarget {
            category: "hermit_tentacle",
            display_name: "Hermit God Tentacle",
            repr_type: 3428,
        },
    ),
    // The Plague Doctor (realm event): all Lethal Cure orbs (A/B/C plus their
    // effect forms) summed into one "Lethal Cure" row alongside the boss card.
    (
        &[34490, 34491, 34492, 34504, 34505, 34506],
        AuxTarget {
            category: "lethal_cure",
            display_name: "Lethal Cure",
            repr_type: 34490,
        },
    ),
    // --- Realm set-piece event adds ---
    // World's Oyster: both World's Pearls summed into one row.
    (
        &[34546],
        AuxTarget {
            category: "worlds_pearl",
            display_name: "World's Pearl",
            repr_type: 34546,
        },
    ),
    // Crab Sovereign: every Royal Crab summed into one row.
    (
        &[51073],
        AuxTarget {
            category: "royal_crab",
            display_name: "Royal Crab",
            repr_type: 51073,
        },
    ),
    // Killer Bee Nest (realm event): all Mini Killer Bee Nests summed into one row.
    (
        &[4238],
        AuxTarget {
            category: "mini_bee_nest",
            display_name: "Mini Killer Bee Nest",
            repr_type: 4238,
        },
    ),
    // Avatar of the Forgotten King: Killer Pillars (new + legacy, both bar/half
    // forms) summed into one row.
    (
        &[
            21967, 21968, 21969, 21970, 21971, 21972, 21973, 21974, // new
            29519, 29530, 29531, 29532, 34451, 34452, 34453, 34454, // legacy
        ],
        AuxTarget {
            category: "killer_pillars",
            display_name: "Killer Pillar",
            repr_type: 21967,
        },
    ),
    // Avatar of the Forgotten King: Shades of the Avatar summed into one row.
    (
        &[21963, 29518],
        AuxTarget {
            category: "shades_of_avatar",
            display_name: "Shades of the Avatar",
            repr_type: 21963,
        },
    ),
    // Avatar of the Forgotten King: Eyes of the Avatar summed into one row.
    (
        &[21976, 29535],
        AuxTarget {
            category: "eye_of_avatar",
            display_name: "Eye of the Avatar",
            repr_type: 21976,
        },
    ),
    // Avatar of the Forgotten King: ShadowBombs (new + legacy, inflated and
    // deflated forms) summed into one row.
    (
        &[21964, 21965, 34448, 34449],
        AuxTarget {
            category: "shadowbombs",
            display_name: "ShadowBombs",
            repr_type: 21964,
        },
    ),
    // Sentient Monolith: all four Monolith Siphons summed into one row.
    (
        &[16936, 16937, 16938, 16970],
        AuxTarget {
            category: "monolith_siphon",
            display_name: "Monolith Siphon",
            repr_type: 16936,
        },
    ),
    // Eye of the Storm: every Tornado summed into one row.
    (
        &[51033],
        AuxTarget {
            category: "eye_storm_tornado",
            display_name: "Tornado",
            repr_type: 51033,
        },
    ),
    // Hornet's Nest: every Angry Hornet summed into one row inside the Nest card.
    (
        &[34604],
        AuxTarget {
            category: "angry_hornet",
            display_name: "Angry Hornet",
            repr_type: 34604,
        },
    ),
    // Legion Missionary: Holy and Chaos Orbs each summed into their own row in
    // its card.
    (
        &[53017],
        AuxTarget {
            category: "legion_orb_holy",
            display_name: "Orb of Light",
            repr_type: 53017,
        },
    ),
    (
        &[53018],
        AuxTarget {
            category: "legion_orb_chaos",
            display_name: "Orb of Chaos",
            repr_type: 53018,
        },
    ),
    // Fungal Cavern: the Crystal Worm Father (a single 37.5k-HP support) and the
    // Crystal Worm Child spawns (head/body/tail segments) each summed into their
    // own row inside the Crystal Worm Mother card.
    (
        &[45711],
        AuxTarget {
            category: "crystal_worm_father",
            display_name: "Crystal Worm Father",
            repr_type: 45711,
        },
    ),
    (
        &[5534, 5535, 5536],
        AuxTarget {
            category: "crystal_worm_child",
            display_name: "Crystal Worm Child",
            repr_type: 5534,
        },
    ),
    (
        // SpecPen Doctor Lobotomik "Sentipede" (LBT Transformation 4): the head
        // (23961) plus its body-segment forms 4B/4T (23934/23935), which spawn as
        // many separate MINIBOSS instances. Summed into one row drawn with the
        // head sprite instead of a row per body segment.
        &[23961, 23934, 23935],
        AuxTarget {
            category: "specpen_lobotomik_sentipede",
            display_name: "Doctor Lobotomik",
            repr_type: 23961,
        },
    ),
];

/// Classify a damaged object type as an aggregated summary target, if any.
pub fn aux_target_for_type(object_type: i32) -> Option<AuxTarget> {
    AUX_TARGETS
        .iter()
        .find(|(types, _)| types.contains(&object_type))
        .map(|(_, target)| *target)
}

/// Aux categories whose aggregate row omits the instance-count suffix. A count
/// like "x7" reads as several separate bosses; for a single multi-part form
/// (the Lobotomik Sentipede: one head plus its body segments) a clean name is
/// clearer.
pub fn aux_category_hides_count(category: &str) -> bool {
    matches!(category, "specpen_lobotomik_sentipede")
}

/// Find the curated encounter a boss object type belongs to, if any.
pub fn encounter_for_boss_type(boss_type: i32) -> Option<&'static Encounter> {
    ENCOUNTERS
        .iter()
        .find(|e| e.member_types.contains(&boss_type))
}

/// Look up a curated encounter by its stable id.
pub fn encounter_by_id(id: &str) -> Option<&'static Encounter> {
    ENCOUNTERS.iter().find(|e| e.id == id)
}

/// Curated boss-type -> canonical dungeon overrides for bosses fought from the
/// realm that actually belong to a rated dungeon the raw map name doesn't
/// reflect. Oryx the Mad God 2 (2354) is the realm-spawned Wine Cellar boss;
/// Janus the Doorwarden (8200) and the Stone Guardians (3448/3449) belong to
/// Oryx's Castle. Logging their fights under the real dungeon makes them
/// classify by grave difficulty and show the correct portal / card instead of
/// a bare realm entry.
const BOSS_DUNGEON_OVERRIDES: &[(i32, &str)] = &[
    (2354, "Wine Cellar"),   // Oryx the Mad God 2
    (8200, "Oryx's Castle"), // Janus the Doorwarden
    (3448, "Oryx's Castle"), // Stone Guardian (variant a)
    (3449, "Oryx's Castle"), // Stone Guardian (variant b)
];

/// Canonical dungeon name for a boss type when the raw map name doesn't reflect
/// it (see [`BOSS_DUNGEON_OVERRIDES`]).
pub fn canonical_dungeon_for_boss(boss_object_type: i32) -> Option<&'static str> {
    BOSS_DUNGEON_OVERRIDES
        .iter()
        .find(|(t, _)| *t == boss_object_type)
        .map(|(_, d)| *d)
}

/// Encounters whose members are alternative/mini bosses that funnel into a single
/// headline boss. Their history card should list only the headline boss (the
/// encounter's `display_name`) instead of every killed member. Cultist Hideout
/// spawns several cult leaders (Molek, Balaam, ...) but only Malus is meaningful.
/// Towering Perfection's segments (Imperfections, Toppled Cubes) are damage
/// sponges that routinely escape, so the card is always headlined by the core.
const HEADLINE_ONLY_ENCOUNTERS: &[&str] = &["cultist_hideout", "towering_perfection"];

/// Whether the encounter's history card should show only its headline boss name.
pub fn encounter_headline_only(id: &str) -> bool {
    HEADLINE_ONLY_ENCOUNTERS.contains(&id)
}

/// Realm-event encounters whose members spawn in the Realm (a non-groupable map).
/// These fold their boss and aggregated adds into one card even outside a
/// dungeon; scoped per encounter id so distinct events in one realm visit don't
/// collapse into each other.
const REALM_ENCOUNTERS: &[&str] = &[
    "astral_rift",
    "hermit_god",
    "the_plague_doctor",
    // Realm set-piece events.
    "worlds_oyster",
    "jade_garnet_statues",
    "flying_behemoth",
    "crab_sovereign",
    "killer_bee_nest",
    "sentient_monolith",
    "eye_of_the_storm",
    "towering_perfection",
    "pentaract",
    "assembled_giant",
    "slumbering_dragon",
    "animal_merchant",
    "hornets_nest",
    "legion_missionary",
    // Avatar of the Forgotten King spawns as a realm event; group its adds into
    // one card headlined by the Avatar.
    "avatar_forgotten_king",
];

/// Whether an encounter should still group when it occurs in the Realm/Nexus.
pub fn encounter_realm_grouped(id: &str) -> bool {
    REALM_ENCOUNTERS.contains(&id)
}

/// Encounters whose card counts as killed if ANY of the listed member types was
/// killed, rather than deferring to the run anchor. Covers realm events without
/// a single killable anchor: the Killer Bee Nest hive can despawn un-killed while
/// its Beehemoths are the real kill, and the Pentaract marker is invisible while
/// its Towers are the ones destroyed. Keep the type lists small.
const COMPLETION_ANY: &[(&str, &[i32])] = &[
    ("killer_bee_nest", &[4324, 4325, 4326]),
    ("pentaract", &[1337, 3422, 22018]),
    ("assembled_giant", &[34480, 34485, 34486]), // Head / Body / Arm
    ("slumbering_dragon", &[34598, 34599]),      // Dragon / Dragon's Treasure
    // Legacy Woodland Labyrinth: the run is only "Completed" when the final
    // Murderous Megamoth (52665) dies. The earlier self-destructing forms
    // (Larva/Mammoth) must never complete the card on their own, so anchor on
    // the final form's kill instead of the latest-ended phase.
    ("legacy_murderous_megamoth", &[52665]),
];

/// Member types whose kill marks the encounter's card as killed, if the
/// encounter uses "any member killed" completion (see [`COMPLETION_ANY`]).
pub fn encounter_completion_any(id: &str) -> Option<&'static [i32]> {
    COMPLETION_ANY
        .iter()
        .find(|(eid, _)| *eid == id)
        .map(|(_, types)| *types)
}

/// Encounters whose card counts as killed only when EVERY listed member type was
/// killed, rather than deferring to a single anchor. Covers dungeons whose
/// completion (mark / exit portal) requires clearing multiple mandatory bosses:
/// the Tomb of the Ancients drops its Mark and portal only after all three main
/// bosses (Nut, Geb, Bes) are defeated, so killing one boss or merely opening a
/// Sarcophagus must not mark the run Completed. Keep the lists small.
const COMPLETION_ALL: &[(&str, &[i32])] = &[
    ("tomb_of_the_ancients", &[3366, 3367, 3368]), // Nut / Geb / Bes
];

/// Member types that must ALL be killed for the encounter's card to count as
/// killed, if the encounter uses "all members killed" completion (see
/// [`COMPLETION_ALL`]).
pub fn encounter_completion_all(id: &str) -> Option<&'static [i32]> {
    COMPLETION_ALL
        .iter()
        .find(|(eid, _)| *eid == id)
        .map(|(_, types)| *types)
}

/// Encounters whose card latches Completed when a loot bag dropped by the given
/// mob type is detected, even if that boss was never observed dying. Covers
/// events whose killable anchor is often offscreen: Towering Perfection's core
/// (47909) has low HP and can drop its bag without ever entering the viewport,
/// while the visible damage lands on its Lower/Upper Imperfection segments.
const LOOT_COMPLETES_ENCOUNTERS: &[(i32, &str)] = &[
    (47909, "towering_perfection"), // Towering Perfection core bag
];

/// The encounter whose card should latch Completed when a bag dropped by
/// `mob_type` is detected (see [`LOOT_COMPLETES_ENCOUNTERS`]).
pub fn encounter_loot_completes(mob_type: i32) -> Option<&'static str> {
    LOOT_COMPLETES_ENCOUNTERS
        .iter()
        .find(|(t, _)| *t == mob_type)
        .map(|(_, id)| *id)
}

/// Whether `encounter_id` supports loot-driven completion, i.e. its card may be
/// marked Completed from a run-level `killed` flag even when no member phase was
/// scored as a kill. Only these encounters honor the `encounter_runs.killed`
/// override; every other card derives completion purely from its anchor phase.
pub fn encounter_supports_loot_completion(encounter_id: &str) -> bool {
    LOOT_COMPLETES_ENCOUNTERS
        .iter()
        .any(|(_, id)| *id == encounter_id)
}

/// Return curated encounter ids whose display names match `text`.
pub fn encounter_ids_matching_name(text: &str) -> Vec<&'static str> {
    let text = text.to_lowercase();
    ENCOUNTERS
        .iter()
        .filter(|encounter| encounter.display_name.to_lowercase().contains(&text))
        .map(|encounter| encounter.id)
        .collect()
}

/// Asset manager for RotMG game assets.
///
/// Provides lazy-loaded, cached access to:
/// - Object definitions (items, enemies, NPCs)
/// - Tile definitions
/// - Sprite atlas data
/// - Enchantment definitions
#[derive(Debug)]
pub struct AssetManager {
    /// Base directory for asset files
    assets_dir: RwLock<Option<PathBuf>>,
    /// Object list (lazy loaded)
    objects: RwLock<Option<ObjectList>>,
    /// Tile list (lazy loaded)
    tiles: RwLock<Option<TileList>>,
    /// Sprite atlas (lazy loaded)
    sprites: RwLock<Option<SpriteAtlas>>,
    /// Enchantment list (lazy loaded)
    enchantments: RwLock<Option<EnchantmentList>>,
    /// Dungeon modifier table from mods.xml (lazy loaded)
    modifiers: RwLock<Option<ModifierTable>>,
    /// Dungeon difficulty + item-dungeon mapping (lazy loaded)
    dungeon_data: RwLock<Option<DungeonData>>,
    /// Forge/tooltip categories derived from `collectionIcon` (lazy, cached)
    dungeon_categories: RwLock<Option<std::sync::Arc<Vec<DungeonCategory>>>>,
    /// Whether assets have been initialized
    initialized: RwLock<bool>,
    /// Error from last load attempt
    last_error: RwLock<Option<String>>,
}

impl AssetManager {
    /// Create a new asset manager.
    pub fn new() -> Self {
        Self {
            assets_dir: RwLock::new(None),
            objects: RwLock::new(None),
            tiles: RwLock::new(None),
            sprites: RwLock::new(None),
            enchantments: RwLock::new(None),
            modifiers: RwLock::new(None),
            dungeon_data: RwLock::new(None),
            dungeon_categories: RwLock::new(None),
            initialized: RwLock::new(false),
            last_error: RwLock::new(None),
        }
    }

    /// Set the assets directory path.
    ///
    /// This should be called once at startup with the path to the assets folder.
    /// Expected structure:
    /// - assets/ObjectID.list
    /// - assets/TileID.list
    /// - assets/flatbuffer/spritesheetf
    /// - assets/sprites/*.png
    /// - assets/xml/enchantments.xml
    pub fn set_assets_dir<P: AsRef<Path>>(&self, path: P) {
        let mut dir = self.assets_dir.write().unwrap();
        *dir = Some(path.as_ref().to_path_buf());

        // Reset loaded state
        *self.initialized.write().unwrap() = false;
        *self.objects.write().unwrap() = None;
        *self.tiles.write().unwrap() = None;
        *self.sprites.write().unwrap() = None;
        *self.enchantments.write().unwrap() = None;
        *self.modifiers.write().unwrap() = None;
        *self.dungeon_data.write().unwrap() = None;
        *self.dungeon_categories.write().unwrap() = None;
        *self.last_error.write().unwrap() = None;
    }

    /// Get the assets directory path.
    pub fn assets_dir(&self) -> Option<PathBuf> {
        self.assets_dir.read().unwrap().clone()
    }

    /// Check if assets have been loaded.
    pub fn is_loaded(&self) -> bool {
        *self.initialized.read().unwrap()
    }

    /// Get the last error from loading.
    pub fn last_error(&self) -> Option<String> {
        self.last_error.read().unwrap().clone()
    }

    /// Try to load all assets from the configured directory.
    ///
    /// Returns true if all assets were loaded successfully.
    pub fn try_load(&self) -> bool {
        let assets_dir = match self.assets_dir.read().unwrap().clone() {
            Some(dir) => dir,
            None => {
                *self.last_error.write().unwrap() = Some("Assets directory not set".to_string());
                return false;
            }
        };

        // Already loaded
        if *self.initialized.read().unwrap() {
            return true;
        }

        let mut success = true;
        let mut errors = Vec::new();

        // Load object list
        let object_path = assets_dir.join("ObjectID.list");
        match ObjectList::load_from_file(&object_path) {
            Ok(mut list) => {
                tracing::info!("Loaded {} objects from {:?}", list.len(), object_path);

                // Enrich Summon-class objects with projectile display metadata
                // (displayId, ignoreOnTooltip) before equipment XML merge, so
                // SpawnCreep resolution can filter to visible variants.
                let allies_xml_path = assets_dir.join("xml").join("allies.xml");
                if allies_xml_path.exists() {
                    match list.merge_allies_xml(&allies_xml_path) {
                        Ok(count) => {
                            tracing::info!(
                                "Enriched {} summon projectiles from {:?}",
                                count,
                                allies_xml_path
                            );
                        }
                        Err(e) => {
                            tracing::warn!("Failed to merge allies XML: {}", e);
                        }
                    }
                }

                // Merge equipment XML data (tier, feed_power, fame_bonus, bag_type)
                let equip_xml_path = assets_dir.join("xml").join("equip.xml");
                if equip_xml_path.exists() {
                    match list.merge_equipment_xml(&equip_xml_path) {
                        Ok(count) => {
                            tracing::info!(
                                "Merged equipment data for {} items from {:?}",
                                count,
                                equip_xml_path
                            );
                        }
                        Err(e) => {
                            tracing::warn!("Failed to merge equipment XML: {}", e);
                        }
                    }
                }

                *self.objects.write().unwrap() = Some(list);
            }
            Err(e) => {
                errors.push(format!("ObjectID.list: {}", e));
                success = false;
            }
        }

        // Load tile list
        let tile_path = assets_dir.join("TileID.list");
        match TileList::load_from_file(&tile_path) {
            Ok(list) => {
                tracing::info!("Loaded {} tiles from {:?}", list.len(), tile_path);
                *self.tiles.write().unwrap() = Some(list);
            }
            Err(e) => {
                errors.push(format!("TileID.list: {}", e));
                // Tiles are optional, don't fail
            }
        }

        // Load sprite atlas
        let sprite_path = assets_dir.join("flatbuffer").join("spritesheetf");
        match SpriteAtlas::load_from_file(&sprite_path) {
            Ok(atlas) => {
                tracing::info!(
                    "Loaded sprite atlas: {} sheets, {} sprites",
                    atlas.sheet_count(),
                    atlas.sprite_count()
                );
                *self.sprites.write().unwrap() = Some(atlas);
            }
            Err(e) => {
                errors.push(format!("spritesheetf: {}", e));
                // Sprites are optional for name lookups
            }
        }

        // Load enchantment list
        let enchant_path = assets_dir.join("xml").join("enchantments.xml");
        match EnchantmentList::load_from_file(&enchant_path) {
            Ok(list) => {
                tracing::info!("Loaded {} enchantments from {:?}", list.len(), enchant_path);
                *self.enchantments.write().unwrap() = Some(list);
            }
            Err(e) => {
                tracing::warn!("Failed to load enchantments: {}", e);
                // Enchantments are optional, don't fail
            }
        }

        // Load dungeon modifier table from mods.xml
        let mods_path = assets_dir.join("xml").join("mods.xml");
        match ModifierTable::load_from_file(&mods_path) {
            Ok(table) => {
                tracing::info!(
                    "Loaded {} dungeon modifiers from {:?}",
                    table.len(),
                    mods_path
                );
                *self.modifiers.write().unwrap() = Some(table);
            }
            Err(e) => {
                tracing::warn!("Failed to load dungeon modifiers: {}", e);
                // The app can continue with raw modifier tokens.
            }
        }

        // Build dungeon data (item mappings from ORG_ labels)
        let objects_guard = self.objects.read().unwrap();
        let dungeon_data = DungeonData::build(objects_guard.as_ref());
        drop(objects_guard);
        *self.dungeon_data.write().unwrap() = Some(dungeon_data);
        tracing::info!("[ASSETS] Built dungeon data: item-dungeon mappings loaded");

        if !errors.is_empty() {
            *self.last_error.write().unwrap() = Some(errors.join("; "));
        }

        *self.initialized.write().unwrap() = success;
        success
    }

    /// Get the display name for an object ID.
    /// Humanizes `id_name` fallbacks by replacing underscores with spaces
    /// (e.g., `Small_Gemsbok_Cloth` -> `"Small Gemsbok Cloth"`).
    ///
    /// For stackable items (tarot cards, schematics, etc.) where the
    /// `id_name` has a " xN" suffix but `display_name` does not, the
    /// `id_name` is used so the stack count is visible.
    pub fn object_name(&self, id: i32) -> Option<String> {
        self.try_load();
        self.objects.read().unwrap().as_ref().and_then(|list| {
            let obj = list.get(id)?;
            let name = obj.name();

            // If id_name has a stack suffix (" xN") that name() doesn't,
            // prefer id_name so callers can extract the stack count.
            if Self::parse_stack_suffix(&obj.id_name).is_some()
                && Self::parse_stack_suffix(name).is_none()
            {
                return Some(obj.id_name.replace('_', " "));
            }

            // If we fell back to id_name, humanize underscores
            if name == obj.id_name {
                Some(name.replace('_', " "))
            } else {
                Some(name.to_string())
            }
        })
    }

    /// Parse a " xN" stack-count suffix from an item name.
    ///
    /// Returns `Some((base_name, count))` when the name ends with " x<digits>",
    /// or `None` for non-stackable names.
    pub fn parse_stack_suffix(name: &str) -> Option<(&str, u32)> {
        if let Some(idx) = name.rfind(" x") {
            let suffix = &name[idx + 2..];
            if let Ok(count) = suffix.parse::<u32>() {
                return Some((&name[..idx], count));
            }
        }
        None
    }

    /// Get stack info for an item: returns `(canonical_base_id, stack_count)`.
    ///
    /// For stackable items (id_name ending with " xN"), the base_id is the
    /// x1 variant's ID and stack_count is N. For non-stackable items,
    /// returns `(id, 1)`.
    pub fn get_stack_info(&self, id: i32) -> (i32, u32) {
        self.try_load();
        if let Some(list) = self.objects.read().unwrap().as_ref() {
            if let Some(obj) = list.get(id) {
                if let Some((base_name, count)) = Self::parse_stack_suffix(&obj.id_name) {
                    // Find the x1 variant as the canonical representative
                    let x1_name = format!("{} x1", base_name);
                    if let Some(x1_id) = list.id_for_name(&x1_name) {
                        return (x1_id, count);
                    }
                    // Fallback: try base name without suffix
                    if let Some(base_id) = list.id_for_name(base_name) {
                        return (base_id, count);
                    }
                    // Last resort: use this id itself
                    return (id, count);
                }
            }
        }
        (id, 1)
    }

    /// Check if two item IDs refer to the same logical item, accounting for
    /// stackable variants. Returns true if `item_id == filter_id` or both
    /// map to the same stackable base item.
    pub fn items_match(&self, item_id: i32, filter_id: i32) -> bool {
        if item_id == filter_id {
            return true;
        }
        let (base, _) = self.get_stack_info(item_id);
        base == filter_id
    }

    /// Check if an item ID is a shiny variant.
    pub fn is_shiny(&self, id: i32) -> bool {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .map(|list| list.is_shiny(id))
            .unwrap_or(false)
    }

    /// Check if an item ID is a Legendary rarity item.
    pub fn is_legendary(&self, id: i32) -> bool {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .map(|list| list.is_legendary(id))
            .unwrap_or(false)
    }

    /// Check if an item ID is a Divine rarity item.
    pub fn is_divine(&self, id: i32) -> bool {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .map(|list| list.is_divine(id))
            .unwrap_or(false)
    }

    /// Check if an item ID is a Legendary+ rarity item (Legendary or Divine).
    pub fn is_legendary_plus(&self, id: i32) -> bool {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .map(|list| list.is_legendary_plus(id))
            .unwrap_or(false)
    }

    /// Check if an item ID is a UT (Untiered) item.
    pub fn is_ut(&self, id: i32) -> bool {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .map(|list| list.is_ut(id))
            .unwrap_or(false)
    }

    /// Get the forge/tooltip category icon index for an item, normalizing
    /// stackable variants to their canonical base first. Returns `None` for
    /// items that are neither UT nor a Blueprint, or that have no
    /// `collectionIcon`, so the value is consistent with
    /// [`AssetManager::dungeon_categories`] membership.
    pub fn item_category_icon(&self, id: i32) -> Option<i32> {
        let (base, _) = self.get_stack_info(id);
        self.try_load();
        let list = self.objects.read().unwrap();
        let list = list.as_ref()?;
        let obj = list.get(base).or_else(|| list.get(id))?;
        if !obj.is_ut() && !obj.is_blueprint() {
            return None;
        }
        obj.collection_icon
    }

    /// Get the cached list of forge/tooltip categories (UT items grouped by
    /// `collectionIcon`). Built lazily on first access and cached until assets
    /// reload. Returns an empty list if assets aren't loaded.
    pub fn dungeon_categories(&self) -> std::sync::Arc<Vec<DungeonCategory>> {
        self.try_load();
        if let Some(cats) = self.dungeon_categories.read().unwrap().as_ref() {
            return cats.clone();
        }
        let built = self
            .objects
            .read()
            .unwrap()
            .as_ref()
            .map(build_categories)
            .unwrap_or_default();
        let arc = std::sync::Arc::new(built);
        *self.dungeon_categories.write().unwrap() = Some(arc.clone());
        arc
    }

    /// Resolve the item a forge Blueprint unlocks, from the authoritative
    /// `UnlockForgeBlueprint` activate target parsed from equip.xml. Returns
    /// `(item_id, item_name)`, or `None` when `id` isn't a mapped blueprint or
    /// the unlocked item can't be resolved. Cheap enough to call per-frame for
    /// every item slot: a couple of HashMap lookups, no `ObjectAsset` clone.
    pub fn blueprint_unlocked_item(&self, id: i32) -> Option<(i32, String)> {
        self.try_load();
        let guard = self.objects.read().unwrap();
        let list = guard.as_ref()?;
        let item_id = list.blueprint_unlocked_id(id)?;
        let name = list
            .name(item_id)
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("Item 0x{item_id:04X}"));
        Some((item_id, name))
    }

    /// Human-readable dungeon/collection name for a `collectionIcon` frame
    /// index, sourced from the same forge-category data used by the Treasury
    /// dungeon filters. Returns `None` when no category carries that icon or it
    /// has no resolvable dungeon name.
    pub fn dungeon_name_for_collection_icon(&self, icon: i32) -> Option<String> {
        self.dungeon_categories()
            .iter()
            .find(|c| c.icon_index == icon)
            .map(|c| c.display_name.clone())
            .filter(|n| !n.is_empty())
    }

    /// Get the object class for an object ID (e.g., "Character", "Equipment", "Portal").
    pub fn object_class(&self, id: i32) -> Option<String> {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .and_then(|list| list.get(id).map(|a| a.class.clone()))
    }

    /// Get the object group for an object ID (e.g., "Weapon", "Armor", "Enemy").
    pub fn object_group(&self, id: i32) -> Option<String> {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .and_then(|list| list.get(id).map(|a| a.group.clone()))
    }

    /// Get the object labels for an object ID (e.g., "UT", "ST", "LEGENDARY").
    pub fn object_labels(&self, id: i32) -> Option<String> {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .and_then(|list| list.get(id).map(|a| a.labels.clone()))
    }

    /// All boss-like `Character` objects carrying `label` that have a renderable
    /// sprite, as `(object_id, display_name)`. Spawned minions / clones / adds
    /// are excluded. Deterministically deduplicated by display name -- entries
    /// are sorted by `(name, id)` first, so a name shared by several variants
    /// always collapses to the lowest id -- then sorted by name. Used to
    /// populate the boss-group filter tooltips for the label-defined realm groups
    /// (beacon guardians, minibosses, adept / veteran encounters).
    pub fn objects_with_label(&self, label: &str) -> Vec<(i32, String)> {
        // The label itself is specific to real encounter bosses, so only spawned
        // minions need excluding (no strict boss-like check, which would drop
        // encounters whose only notable tag is the group label).
        self.collect_boss_objects(false, |asset| asset.labels.split(',').any(|l| l == label))
    }

    /// All boss-like `Character` objects with a renderable sprite whose display
    /// name contains any of `fragments` (case-insensitive), excluding spawned
    /// minions / adds. Deterministically deduplicated and sorted like
    /// [`Self::objects_with_label`]. Used for the seasonal boss-group tooltip,
    /// whose members are identified by curated name fragments rather than a
    /// catalog label.
    pub fn objects_with_name_fragments(&self, fragments: &[&str]) -> Vec<(i32, String)> {
        // Name fragments are broad (they match egg / present / lawnmower adds too),
        // so require a boss-like tag to keep only the seasonal bosses themselves.
        self.collect_boss_objects(true, |asset| {
            let lower = asset.name().to_lowercase();
            fragments.iter().any(|f| lower.contains(f))
        })
    }

    /// Shared helper for the boss-group tooltip catalogs: gather every
    /// sprite-bearing `Character` object that is not a spawned minion (and,
    /// when `require_boss_like`, carries a boss-like tag) and passes `pred`, then
    /// deterministically dedupe by name (lowest id wins) and sort by name.
    fn collect_boss_objects(
        &self,
        require_boss_like: bool,
        pred: impl Fn(&super::object_list::ObjectAsset) -> bool,
    ) -> Vec<(i32, String)> {
        self.try_load();
        let guard = self.objects.read().unwrap();
        let Some(list) = guard.as_ref() else {
            return Vec::new();
        };
        let mut matches: Vec<(i32, String)> = list
            .iter()
            .filter(|asset| {
                asset.class == "Character"
                    && asset.primary_texture().is_some()
                    && !labels_are_minion(&asset.labels)
                    && UNFIGHTABLE_CATALOG_TYPES.binary_search(&asset.id).is_err()
                    && (!require_boss_like || labels_are_boss_like(&asset.labels, &asset.group))
                    && pred(asset)
            })
            .map(|asset| (asset.id, asset.name().to_string()))
            .collect();
        // Sort by (name, id) so dedup_by_key keeps the lowest id per name,
        // making the choice deterministic regardless of HashMap iteration order.
        matches.sort_by(|a, b| {
            a.1.to_lowercase()
                .cmp(&b.1.to_lowercase())
                .then(a.0.cmp(&b.0))
        });
        matches.dedup_by(|a, b| a.1.eq_ignore_ascii_case(&b.1));
        matches
    }

    /// Find a portal object ID for a dungeon name.
    /// Searches loaded Portal objects for one matching the dungeon name.
    /// Returns None if no matching portal found.
    pub fn find_portal_for_dungeon(&self, dungeon_name: &str) -> Option<i32> {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .and_then(|list| list.find_portal_for_dungeon(dungeon_name))
    }

    /// Check if an object is a realm encounter (boss/event).
    ///
    /// This checks:
    /// - Labels containing "ENCOUNTER" (e.g., "ENCOUNTER,Skull Shrine")
    /// - Groups ending with "Encounter" (e.g., "Hermit God Encounter")
    /// - Specific entity names that should be tracked (e.g., "Mysterious Crystal")
    pub fn is_encounter(&self, id: i32) -> bool {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .map(|list| {
                if let Some(asset) = list.get(id) {
                    // Check labels for "ENCOUNTER" (exact match in comma-separated list)
                    let has_encounter_label = asset.labels.split(',').any(|l| l == "ENCOUNTER");

                    // Check if group ends with "Encounter" (e.g., "Hermit God Encounter")
                    let has_encounter_group = asset.group.ends_with("Encounter");

                    // Check for specific entity names that should be tracked
                    let name = asset.name();
                    let is_special_entity =
                        name == "Mysterious Crystal" || name == "Killer Bee Nest";

                    has_encounter_label || has_encounter_group || is_special_entity
                } else {
                    false
                }
            })
            .unwrap_or(false)
    }

    /// Whether an object type is a boss, mini-boss, Hero of Oryx, or other notable
    /// combat encounter - i.e. something a Combat History fight should be
    /// reconstructed for.
    ///
    /// Detection is tag-based (independent of HP, so low-HP mini-bosses still
    /// qualify) using the labels/group from the object catalog. It deliberately
    /// excludes two over-broad mechanical labels:
    /// - `GOD` (stasis-immunity flag carried by many mid-tier minions, e.g. Sprite
    ///   Forest sprites are `MINION_MID,GOD`).
    /// - `BOSSFIGHT` (carried by weak adds spawned during a fight, e.g. Small
    ///   Worker Bees are `BOSS_SPAWNED,BOSSFIGHT,CRITTER,MINION_WEAK`).
    ///
    /// Recognized: labels `BOSS`, `MINIBOSS`, `HERO`, `QUEST` (the in-game quest-
    /// arrow / notable-enemy marker), `ENCOUNTER`; or a group ending in
    /// "Encounter". Real bosses carry at least one of these; trash minions do not.
    pub fn is_boss_like(&self, id: i32) -> bool {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .map(|list| {
                if let Some(asset) = list.get(id) {
                    labels_are_boss_like(&asset.labels, &asset.group)
                } else {
                    false
                }
            })
            .unwrap_or(false)
    }

    /// Like [`Self::is_boss_like`] but omitting the weak `QUEST` marker (see
    /// [`labels_are_strong_boss`]). Used to decide whether a minion-tagged object
    /// is a genuine boss: a mere quest-arrow tag never qualifies a spawned add.
    pub fn is_strong_boss_like(&self, id: i32) -> bool {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .map(|list| {
                if let Some(asset) = list.get(id) {
                    labels_are_strong_boss(&asset.labels, &asset.group)
                } else {
                    false
                }
            })
            .unwrap_or(false)
    }

    /// Whether an object is a spawned minion / clone / stage-add (see
    /// [`labels_are_minion`]). Such entities never drop soulbound loot and must
    /// not be recorded as their own boss fight, regardless of max HP.
    pub fn is_minion(&self, id: i32) -> bool {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .map(|list| {
                list.get(id)
                    .map(|asset| labels_are_minion(&asset.labels))
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    }

    /// Whether `id` is in the curated boss allow-list ([`CURATED_BOSS_TYPES`]):
    /// a known standalone (mini)boss the catalog mislabels as `MINION`.
    pub fn is_curated_boss(&self, id: i32) -> bool {
        is_curated_boss_type(id)
    }

    /// Whether `id` is in the curated deny-list ([`CURATED_NON_BOSS_TYPES`]): a
    /// known boss summon / add that carries boss-tier HP but no catalog labels
    /// and must never be recorded as its own fight.
    pub fn is_curated_non_boss(&self, id: i32) -> bool {
        is_curated_non_boss_type(id)
    }

    /// Whether `id` is a curated wandering boss ([`WANDERING_BOSS_TYPES`]): a
    /// boss that roams in and out of view within a single map instance, so its
    /// fight must be suspended on despawn and resumed on re-entry rather than
    /// recorded as several duplicate fights.
    pub fn is_wandering_boss(&self, id: i32) -> bool {
        is_wandering_boss_type(id)
    }

    /// Whether `id` is a curated self-destruct boss ([`SELF_DESTRUCT_BOSS_TYPES`]):
    /// a boss that explodes on death and is removed at high HP, so the HP-based
    /// kill heuristic misses it. Such a fight is scored as a kill on removal only
    /// when the party dealt ~all of the boss's remaining HP.
    pub fn is_self_destruct_boss(&self, id: i32) -> bool {
        is_self_destruct_boss_type(id)
    }

    /// Whether `id` is a curated treasure crate ([`TREASURE_CRATE_TYPES`]): a
    /// lootable crate/chest grouped under the "Treasure crates" Combat History
    /// category.
    pub fn is_treasure_crate(&self, id: i32) -> bool {
        is_treasure_crate_type(id)
    }

    /// Whether an object is eligible to be a loot drop source (an enemy, boss,
    /// or lootable crate/chest) rather than an environmental structure.
    ///
    /// In the game catalog, enemies AND crates/chests (including Vault quest
    /// chests) are `Character`-class objects, while walls, gates, pillars,
    /// room-check markers, portals, beams, and the pickup-item twins are
    /// `GameObject`/`Equipment`/etc. Only valid sources should ever be recorded
    /// or displayed as the origin of a drop; everything else falls back to "?".
    pub fn is_valid_drop_source(&self, id: i32) -> bool {
        if is_curated_boss_type(id) || is_treasure_crate_type(id) {
            return true;
        }
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .and_then(|list| list.get(id).map(|a| a.class == "Character"))
            .unwrap_or(false)
    }

    /// The boss object types a Moonlight Village completion marker signals as
    /// cleared, or `None` when `id` is not a completion marker.
    /// Used by the combat engine to mark invulnerable-finish bosses as completed.
    pub fn completion_marker_targets(&self, id: i32) -> Option<&'static [i32]> {
        completion_marker_targets_for(id)
    }

    /// Whether `id` is a curated seasonal/special boss ([`SPECIAL_BOSS_TYPES`]):
    /// classified into the "Seasonal and Special bosses" category by object type.
    pub fn is_special_boss(&self, id: i32) -> bool {
        is_special_boss_type(id)
    }

    /// Whether `id` is a player gravestone (class `Gravestone`). Such objects
    /// spawn at a player's death tile, so seeing one lets Combat History mark a
    /// departed participant as dead and render the matching tier grave sprite.
    pub fn is_gravestone(&self, id: i32) -> bool {
        self.object_class(id).as_deref() == Some("Gravestone")
    }

    /// Parse the character tier a gravestone object represents (level bucket and
    /// optional fame star rank), derived from the object's in-game name (e.g.
    /// "Level 20 Default Gravestone", "1/8 Default Gravestone"). Returns `None`
    /// for non-gravestone ids or names that don't match the known tier prefixes.
    pub fn grave_tier(&self, id: i32) -> Option<GraveTier> {
        parse_grave_tier(&self.object_name(id)?)
    }

    /// Get encounter display info, mapping invisible helper entities to their visible counterparts.
    /// Returns (display_name, sprite_type_id) for the encounter.
    ///
    /// Some encounters use invisible "helper" entities with ENCOUNTER label, but the actual
    /// visible boss has a different type ID. This method handles those mappings.
    pub fn encounter_display_info(&self, id: i32) -> Option<(String, i32)> {
        self.try_load();
        self.objects.read().unwrap().as_ref().and_then(|list| {
            let asset = list.get(id)?;
            let name = asset.name();

            // Map invisible helper entities to their visible counterparts
            // "New Train Helper" (invisible, 0x5654) -> "Kogbold Expedition Engine" (0x565D)
            if name == "New Train Helper" || name == "Train Helper" {
                // Return the actual train boss type ID for sprite lookup
                // 0x565D = 22109 = "New Train Encounter" (Kogbold Expedition Engine)
                return Some(("Kogbold Expedition Engine".to_string(), 0x565D));
            }

            // Default: use the entity's own name and type ID
            Some((name.to_string(), id))
        })
    }

    /// Get the display name for an enchantment type ID.
    pub fn enchant_name(&self, type_id: u16) -> Option<String> {
        self.try_load();
        self.enchantments
            .read()
            .unwrap()
            .as_ref()
            .and_then(|list| list.name(type_id).map(|s| s.to_string()))
    }

    /// Owned snapshot of every loaded enchantment definition, for the settings
    /// picker and enchantment-sound matching.
    pub fn enchant_catalog(&self) -> Vec<super::enchantments::EnchantCatalogEntry> {
        self.try_load();
        self.enchantments
            .read()
            .unwrap()
            .as_ref()
            .map(|list| list.catalog())
            .unwrap_or_default()
    }

    /// Damage-reduction percentage of a Knight shield ability item, or `None`
    /// if the item is not a shield. Shields carry the `SHIELD` label; the
    /// Kogbold Cower Shield mitigates 30%, all other shields 25%.
    pub fn shield_reduction_pct(&self, item_id: i32) -> Option<u8> {
        self.try_load();
        self.objects.read().unwrap().as_ref().and_then(|list| {
            let asset = list.get(item_id)?;
            if !asset.labels.split(',').any(|l| l.trim() == "SHIELD") {
                return None;
            }
            if asset.name().to_lowercase().contains("cower") {
                Some(30)
            } else {
                Some(25)
            }
        })
    }

    /// Look up a dungeon modifier by wire token / id from the `mods.xml`
    /// extracted from the installed game.
    pub fn modifier_def(&self, token: &str) -> Option<ModifierDef> {
        self.try_load();
        self.modifiers
            .read()
            .unwrap()
            .as_ref()
            .and_then(|table| table.get(token).cloned())
    }

    /// Get the description for an enchantment type ID.
    pub fn enchant_description(&self, type_id: u16) -> Option<String> {
        self.try_load();
        self.enchantments
            .read()
            .unwrap()
            .as_ref()
            .and_then(|list| list.description(type_id).map(|s| s.to_string()))
    }

    /// Get the tier of an enchantment by type ID.
    pub fn enchant_tier(&self, type_id: u16) -> Option<super::enchantments::EnchantmentTier> {
        self.try_load();
        self.enchantments
            .read()
            .unwrap()
            .as_ref()
            .and_then(|list| list.tier(type_id))
    }

    /// Numeric tier (1-4) of an enchantment from its `TIERn` label, if any.
    pub fn enchant_tier_num(&self, type_id: u16) -> Option<u8> {
        self.try_load();
        self.enchantments
            .read()
            .unwrap()
            .as_ref()
            .and_then(|list| list.tier_num(type_id))
    }

    /// Check if an enchantment is a special tier (Unique or Awakened).
    pub fn enchant_is_special(&self, type_id: u16) -> bool {
        self.try_load();
        self.enchantments
            .read()
            .unwrap()
            .as_ref()
            .map(|list| list.is_special(type_id))
            .unwrap_or(false)
    }

    /// Check if an enchantment is Unique tier.
    pub fn enchant_is_unique(&self, type_id: u16) -> bool {
        self.try_load();
        self.enchantments
            .read()
            .unwrap()
            .as_ref()
            .map(|list| list.is_unique(type_id))
            .unwrap_or(false)
    }

    /// Check if an enchantment is Awakened tier.
    pub fn enchant_is_awakened(&self, type_id: u16) -> bool {
        self.try_load();
        self.enchantments
            .read()
            .unwrap()
            .as_ref()
            .map(|list| list.is_awakened(type_id))
            .unwrap_or(false)
    }

    /// Check if an enchantment is Loot Bonus III or IV (high-tier loot bonus).
    pub fn enchant_is_high_loot_bonus(&self, type_id: u16) -> bool {
        self.try_load();
        self.enchantments
            .read()
            .unwrap()
            .as_ref()
            .map(|list| list.is_high_loot_bonus(type_id))
            .unwrap_or(false)
    }

    /// Check if an enchantment is "valuable" (Unique, Awakened, or HighLootBonus).
    pub fn enchant_is_valuable(&self, type_id: u16) -> bool {
        self.try_load();
        self.enchantments
            .read()
            .unwrap()
            .as_ref()
            .map(|list| list.is_valuable(type_id))
            .unwrap_or(false)
    }

    /// Check if any of the given enchant IDs are valuable (Unique, Awakened, or HighLootBonus).
    pub fn has_any_valuable_enchant(&self, enchant_ids: &[u16]) -> bool {
        self.try_load();
        self.enchantments
            .read()
            .unwrap()
            .as_ref()
            .map(|list| list.has_any_valuable(enchant_ids))
            .unwrap_or(false)
    }

    /// Get full asset data for an object ID.
    pub fn get_object(&self, id: i32) -> Option<ObjectAsset> {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .and_then(|list| list.get(id).cloned())
    }

    /// Look up a weapon's projectile damage data for self-damage simulation.
    ///
    /// Returns `(min_damage, max_damage, armor_piercing, slot_type)` for the
    /// weapon `id`'s projectile at `projectile_id` (clamped into range), or
    /// `None` if the weapon or that projectile is unknown. Cheap: no allocation.
    pub fn weapon_projectile(
        &self,
        id: i32,
        projectile_id: usize,
    ) -> Option<(i32, i32, bool, i32)> {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .and_then(|list| list.get(id))
            .and_then(|asset| {
                asset.projectiles.get(projectile_id).map(|p| {
                    (
                        p.min_damage,
                        p.max_damage,
                        p.armor_piercing,
                        asset.slot_type,
                    )
                })
            })
    }

    /// Whether the object `id`'s projectile at `projectile_id` is armor-piercing
    /// (bypasses defense). Returns `None` when the object or that projectile is
    /// unknown. Used to value the local player's damage taken from enemy shots.
    pub fn projectile_armor_piercing(&self, id: i32, projectile_id: usize) -> Option<bool> {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .and_then(|list| list.get(id))
            .and_then(|asset| {
                asset
                    .projectiles
                    .get(projectile_id)
                    .map(|p| p.armor_piercing)
            })
    }

    /// Number of distinct projectile types defined on a weapon (e.g. 6 for
    /// Fractal Blades). Returns 1 when the object is unknown or has one type.
    pub fn projectile_type_count(&self, id: i32) -> i32 {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .and_then(|list| list.get(id))
            .map(|asset| asset.projectiles.len().max(1) as i32)
            .unwrap_or(1)
    }

    /// Returns the object's base defense (enemy DEF from game data), or `None`
    /// if the object is unknown. Used to reduce our self-computed hit damage for
    /// bosses, whose DEF is never sent in packets (only players broadcast DEF).
    pub fn object_defense(&self, id: i32) -> Option<i32> {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .and_then(|list| list.get(id))
            .map(|asset| asset.defense)
    }

    /// Enemy collision hitbox scale (`CustomHitbox scale`, default 1.0 when the
    /// object is unknown or has no custom hitbox). Used to size the target
    /// circle when testing whether Lethal Strike side procs actually land.
    pub fn enemy_hitbox_scale(&self, id: i32) -> f32 {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .and_then(|list| list.get(id))
            .map(|asset| asset.hitbox_scale)
            .unwrap_or(1.0)
    }

    /// Lethal Strike proc params for an item type (rogue cloaks only).
    /// Returns `None` for items without the buff (all non-cloak items).
    pub fn lethal_strike_params(&self, id: i32) -> Option<LethalStrikeParams> {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .and_then(|list| list.get(id))
            .and_then(|asset| asset.lethal_strike)
    }

    /// Locally-computable damaging ability effects (DetonateHex / PoisonGrenade)
    /// for an ability item, or `None` when the item deals no self-computable
    /// ability damage. Cloned so the assets lock isn't held by the caller.
    pub fn ability_effects(&self, id: i32) -> Option<AbilityEffects> {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .and_then(|list| list.ability_effects(id).cloned())
    }

    /// Native damaging procs (OnPlayerShootActivate/OnEnemyHitActivate) for a
    /// weapon item, cloned so the assets lock isn't held by the caller. Empty
    /// vec when the item is unknown or has no such procs.
    pub fn weapon_procs(&self, id: i32) -> Vec<super::object_list::WeaponProc> {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .map(|list| list.weapon_procs(id).to_vec())
            .unwrap_or_default()
    }

    /// Primary-attack fire attributes `(rate_of_fire, num_projectiles)` for a
    /// weapon item, from equip.xml. `None` if the item is unknown.
    pub fn weapon_fire(&self, id: i32) -> Option<(f32, i32)> {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .and_then(|list| list.weapon_fire(id))
    }

    /// Flat stat bonuses granted while the item is equipped (equip.xml
    /// `ActivateOnEquip` IncrementStat). `None` if the item is unknown.
    pub fn item_stat_bonuses(&self, id: i32) -> Option<super::StatBonuses> {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .and_then(|list| list.item_stat_bonuses(id))
    }

    /// Base XP-gain bonus percentage for an equipped item (equip.xml
    /// `<XPBonus>`; shown on tooltips as "XP Bonus: N%"). 0 when the item is
    /// unknown or has no XP bonus.
    pub fn item_fame_bonus(&self, id: i32) -> i32 {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .map(|list| list.item_fame_bonus(id))
            .unwrap_or(0)
    }

    /// `IncrementStatRelative` equip effects for an item, each
    /// `(stat, percent, stat_relative_to)`. Empty when the item is unknown or
    /// has no relative-stat effects.
    pub fn item_stat_relatives(&self, id: i32) -> Vec<(super::StatKind, f32, super::StatKind)> {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .map(|list| list.item_stat_relatives(id).to_vec())
            .unwrap_or_default()
    }

    /// Parsed `<Mutators>` effects for an enchantment type ID. Empty when the
    /// enchant is unknown or carries no modeled effects.
    pub fn enchant_effects(&self, type_id: u16) -> Vec<super::EnchantEffect> {
        self.try_load();
        self.enchantments
            .read()
            .unwrap()
            .as_ref()
            .map(|list| list.effects(type_id).to_vec())
            .unwrap_or_default()
    }

    /// Combined weapon-damage multiplier from a set of equipped enchant type IDs,
    /// as `(min_mult, max_mult)`. Each `MultiplyMinDamage` / `MultiplyMaxDamage`
    /// mutator whose `projectileId` is `-1` (all) or matches `proj_index` is
    /// multiplied into the running product; enchants with no damage mutators
    /// contribute nothing (1.0). Returns `(1.0, 1.0)` when nothing applies, so a
    /// weapon with no damage enchants rolls exactly as before.
    pub fn weapon_damage_enchant_mult(&self, enchant_ids: &[u16], proj_index: usize) -> (f32, f32) {
        use super::enchantments::EnchantEffectKind;
        self.try_load();
        let guard = self.enchantments.read().unwrap();
        let Some(list) = guard.as_ref() else {
            return (1.0, 1.0);
        };
        let (mut min_mult, mut max_mult) = (1.0f32, 1.0f32);
        for &id in enchant_ids {
            for eff in list.effects(id) {
                let applies = eff.projectile_id < 0 || eff.projectile_id as usize == proj_index;
                if !applies {
                    continue;
                }
                match eff.kind {
                    EnchantEffectKind::MultiplyMinDamage => min_mult *= eff.amount,
                    EnchantEffectKind::MultiplyMaxDamage => max_mult *= eff.amount,
                    _ => {}
                }
            }
        }
        (min_mult, max_mult)
    }

    /// Whether an object ID is a Portal-class object (cheap, no clone).
    pub fn is_portal_object(&self, id: i32) -> bool {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .map(|list| list.is_portal(id))
            .unwrap_or(false)
    }

    /// The equipment `SlotType` of an item (e.g. 21 for an Orb), or `0` when the
    /// id is unknown or not slotted equipment. Used to evaluate mission
    /// `wornRestriction` requirements against a character's equipped items.
    pub fn item_slot_type(&self, id: i32) -> i32 {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .and_then(|list| list.get(id))
            .map(|asset| asset.slot_type)
            .unwrap_or(0)
    }

    /// Build an `ItemCategorizer` from the loaded object list.
    ///
    /// Returns `None` if the object list has not been loaded yet.
    pub fn build_categorizer(&self) -> Option<ItemCategorizer> {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .map(ItemCategorizer::from_object_list)
    }

    /// Get object ID by id_name (internal name like "Sigma_Werewolf").
    pub fn object_id_for_name(&self, id_name: &str) -> Option<i32> {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .and_then(|list| list.id_for_name(id_name))
    }

    /// Get object ID by display name (the in-game item name, e.g. "Crown").
    pub fn object_id_for_display_name(&self, display_name: &str) -> Option<i32> {
        self.try_load();
        self.objects
            .read()
            .unwrap()
            .as_ref()
            .and_then(|list| list.id_for_display_name(display_name))
    }

    /// Resolve the base dungeon-key object id for a dungeon display name, for
    /// key-pop iconography. Keys are named `"<Dungeon> Key"` but the dungeon may
    /// carry a leading "The " that the key drops, so both spellings are tried.
    /// Returns `None` when no matching key exists.
    pub fn key_id_for_dungeon(&self, dungeon_name: &str) -> Option<i32> {
        let d = dungeon_name.trim();
        let stripped = d.strip_prefix("The ").unwrap_or(d);
        let candidates = [format!("{d} Key"), format!("{stripped} Key")];
        for name in candidates {
            if let Some(id) = self.object_id_for_display_name(&name) {
                return Some(id);
            }
        }
        None
    }

    /// Every object ID that shares `id`'s display name (including `id` itself),
    /// used to group biome / re-skinned variants of the same encounter so they
    /// all trigger the same realm-event notification. Returns just `[id]` if the
    /// object is unknown or has no display name.
    pub fn ids_sharing_display_name(&self, id: i32) -> Vec<i32> {
        self.try_load();
        let guard = self.objects.read().unwrap();
        let Some(list) = guard.as_ref() else {
            return vec![id];
        };
        match list.name(id) {
            Some(name) if !name.is_empty() => {
                let ids = list.ids_for_display_name(name);
                if ids.is_empty() {
                    vec![id]
                } else {
                    ids
                }
            }
            _ => vec![id],
        }
    }

    /// Resolve a killer name (from a Death packet) to a sprite object id.
    ///
    /// Prefers the object whose internal `id_name` matches (the canonical enemy /
    /// crate), then a case-insensitive display-name match. Candidates that are
    /// not valid drop sources (environmental structures, portals, beams, pickup
    /// item twins) are skipped so the sprite matches the real enemy. Returns
    /// `None` for names with no known enemy sprite (e.g. player/environmental
    /// causes), which callers render as a fallback marker.
    pub fn killer_sprite_id(&self, killer: &str) -> Option<i32> {
        let killer = killer.trim();
        if killer.is_empty() {
            return None;
        }
        self.try_load();
        let guard = self.objects.read().unwrap();
        let list = guard.as_ref()?;

        // Some distinct objects share a display name (e.g. two "Sunken Treasure"
        // objects, or lore aliases like "King Azamoth" for The Forgotten King).
        // An explicit id override resolves those authoritatively.
        let override_id = KILLER_SPRITE_ID_OVERRIDES
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(killer))
            .map(|(_, id)| *id);

        // A valid drop source is an enemy, boss, or lootable crate/chest -- all
        // `Character`-class in the catalog -- never an environmental structure,
        // portal, or beam that may share the display name.
        let is_valid_source = |id: i32| -> bool {
            is_curated_boss_type(id)
                || is_treasure_crate_type(id)
                || list
                    .get(id)
                    .map(|a| a.class == "Character")
                    .unwrap_or(false)
        };

        let candidates = [
            override_id,
            // Prefer the exact internal name: the canonical enemy/crate object
            // (real bosses often have an empty display name, with the shared
            // display name carried by a portal/beam/marker object instead).
            list.id_for_name(killer),
            list.id_for_display_name(killer),
            // Strip parenthetical suffix e.g. "Inspector Stromwell (Expert)"
            killer.rsplit_once(" (").and_then(|(base, _)| {
                let base = base.trim();
                list.id_for_name(base)
                    .or_else(|| list.id_for_display_name(base))
            }),
        ];

        let sprites = self.sprites.read().unwrap();
        let atlas = sprites.as_ref()?;

        for (idx, candidate) in candidates.into_iter().flatten().enumerate() {
            // The explicit override (idx 0) is authoritative; every other
            // candidate must be a valid drop source to be eligible.
            if idx != 0 && !is_valid_source(candidate) {
                continue;
            }
            if let Some(obj) = list.get(candidate) {
                if let Some(tex) = obj.primary_texture() {
                    if tex.name != "invisible" && atlas.get_sprite(&tex.name, tex.index).is_some() {
                        return Some(candidate);
                    }
                }
            }
        }
        None
    }

    /// Get the name for a tile ID.
    pub fn tile_name(&self, id: i32) -> Option<String> {
        self.try_load();
        self.tiles
            .read()
            .unwrap()
            .as_ref()
            .and_then(|list| list.name(id).map(|s| s.to_string()))
    }

    /// Get full asset data for a tile ID.
    pub fn get_tile(&self, id: i32) -> Option<TileAsset> {
        self.try_load();
        self.tiles
            .read()
            .unwrap()
            .as_ref()
            .and_then(|list| list.get(id).cloned())
    }

    /// Get sprite data for an object ID.
    ///
    /// This looks up the texture reference for the object, then finds
    /// the corresponding sprite coordinates in the atlas.
    pub fn get_object_sprite(&self, id: i32) -> Option<SpriteData> {
        self.try_load();

        // A few objects list their intended character sprite after grave/spawner
        // art, so render an explicit override sheet+index when present.
        if let Some((sheet, index)) = super::object_list::sprite_texture_override(id) {
            let sprites = self.sprites.read().unwrap();
            return sprites.as_ref()?.get_sprite(sheet, index).copied();
        }

        // Get the object's texture reference
        let objects = self.objects.read().unwrap();
        let obj = match objects.as_ref()?.get(id) {
            Some(o) => o,
            None => {
                tracing::trace!(
                    "get_object_sprite: no object found for id={} (0x{:04X})",
                    id,
                    id
                );
                return None;
            }
        };
        let texture = match obj.primary_texture() {
            Some(t) => t,
            None => {
                return None;
            }
        };

        // Look up sprite in atlas
        let sprites = self.sprites.read().unwrap();
        sprites
            .as_ref()?
            .get_sprite(&texture.name, texture.index)
            .copied()
    }

    /// Get sprite data for an object ID with a specific direction.
    ///
    /// Direction values (RotMG convention):
    /// - 0: Down (facing screen)
    /// - 1: Down-Left
    /// - 2: Left
    /// - 3: Up-Left
    /// - 4: Up
    /// - 5: Up-Right
    /// - 6: Right
    /// - 7: Down-Right
    ///
    /// Falls back to direction 0 if the requested direction isn't available.
    pub fn get_object_sprite_with_direction(&self, id: i32, direction: i32) -> Option<SpriteData> {
        self.try_load();

        // Get the object's texture reference
        let objects = self.objects.read().unwrap();
        let obj = objects.as_ref()?.get(id)?;
        let texture = obj.primary_texture()?;

        // Look up sprite with direction in atlas
        let sprites = self.sprites.read().unwrap();
        sprites
            .as_ref()?
            .get_sprite_with_direction(&texture.name, texture.index, direction)
            .copied()
    }

    /// Get sprite data by sheet name and index directly.
    pub fn get_sprite(&self, sheet_name: &str, index: i32) -> Option<SpriteData> {
        self.try_load();
        self.sprites
            .read()
            .unwrap()
            .as_ref()
            .and_then(|atlas| atlas.get_sprite(sheet_name, index).copied())
    }

    /// Get dye rendering info for an item (mask sprite + fill style).
    ///
    /// Returns `Some(DyeInfo)` if the item has a `<Mask>` texture and a
    /// non-zero `Tex1` or `Tex2` value, meaning it should be rendered with
    /// mask-based compositing (solid-color dyes or tiled textile patterns).
    pub fn get_item_dye_info(&self, id: i32) -> Option<DyeInfo> {
        self.try_load();

        let objects = self.objects.read().unwrap();
        let obj = objects.as_ref()?.get(id)?;

        if !obj.has_dye_data() {
            return None;
        }

        let mask_tex = obj.mask.as_ref()?;

        // Resolve fill style from Tex1/Tex2
        use super::object_list::TexInfo;
        let style = match obj.tex_info() {
            TexInfo::Color(r, g, b) => DyeStyle::SolidColor(r, g, b),
            TexInfo::Textile { sheet, index } => {
                let sprites = self.sprites.read().unwrap();
                let tex_sprite = sprites.as_ref()?.get_sprite(sheet, index)?.clone();
                DyeStyle::TextilePattern(tex_sprite)
            }
            TexInfo::None => return None,
        };

        // Look up the mask sprite in the atlas
        let sprites = self.sprites.read().unwrap();
        let mask_sprite = sprites
            .as_ref()?
            .get_sprite(&mask_tex.name, mask_tex.index)?
            .clone();

        Some(DyeInfo { mask_sprite, style })
    }

    /// Get character dye rendering info from dynamic tex1/tex2 values.
    ///
    /// Unlike items (which have tex1/tex2 baked in XML), character dye
    /// values come from live `StatType::Texture1` / `Texture2` packets.
    /// The mask coordinates come from the sprite's FlatBuffer `maskPosition` field.
    ///
    /// Returns `None` if the character sprite has no mask or both tex values are zero.
    pub fn get_character_dye_info(
        &self,
        sprite_id: i32,
        direction: i32,
        tex1: u32,
        tex2: u32,
    ) -> Option<CharacterDyeInfo> {
        if tex1 == 0 && tex2 == 0 {
            return None;
        }

        self.try_load();

        // Get the character's sprite data (which includes mask position)
        let sprite = self.get_object_sprite_with_direction(sprite_id, direction)?;
        let mask_pos = sprite.mask.as_ref()?;

        // Build a SpriteData for the mask region on the characters_masks atlas (ID 3)
        let mask_sprite = SpriteData {
            atlas_id: 3, // characters_masks atlas
            x: mask_pos.x as i32,
            y: mask_pos.y as i32,
            width: mask_pos.width as i32,
            height: mask_pos.height as i32,
            color: [255, 255, 255, 255],
            mask: None,
        };

        // Decode tex1/tex2 into dye styles.
        // API values can be either direct encodings (0x01RRGGBB, 0x04/05/09/0A textiles)
        // or dye item type IDs (small values like 4644) that need ObjectList lookup.
        use super::object_list::TexInfo;
        let resolve_tex = |tex: u32| -> Option<DyeStyle> {
            let info = match super::object_list::decode_tex(tex) {
                TexInfo::None if tex != 0 => {
                    // Not a direct encoding - try as dye item type ID
                    let objects = self.objects.read().unwrap();
                    let obj = objects.as_ref()?.get(tex as i32)?;
                    obj.tex_info()
                }
                other => other,
            };
            match info {
                TexInfo::Color(r, g, b) => Some(DyeStyle::SolidColor(r, g, b)),
                TexInfo::Textile { sheet, index } => {
                    let sprites = self.sprites.read().unwrap();
                    let tex_sprite = sprites.as_ref()?.get_sprite(sheet, index)?.clone();
                    Some(DyeStyle::TextilePattern(tex_sprite))
                }
                TexInfo::None => None,
            }
        };

        let clothing = resolve_tex(tex1);
        let accessory = resolve_tex(tex2);

        if clothing.is_none() && accessory.is_none() {
            return None;
        }

        Some(CharacterDyeInfo {
            mask_sprite,
            clothing,
            accessory,
        })
    }

    /// Get sprite data for an enchantment by its type ID.
    pub fn get_enchant_sprite(&self, type_id: u16) -> Option<SpriteData> {
        self.try_load();
        let enchantments = self.enchantments.read().unwrap();
        let (sheet, index) = enchantments.as_ref()?.texture(type_id)?;
        self.get_sprite(sheet, index)
    }

    /// Get the path to a sprite atlas PNG file.
    ///
    /// Atlas names: "groundTiles", "characters", "characters_masks", "mapObjects"
    pub fn sprite_atlas_path(&self, atlas_name: &str) -> Option<PathBuf> {
        self.assets_dir
            .read()
            .unwrap()
            .as_ref()
            .map(|dir| dir.join("sprites").join(format!("{}.png", atlas_name)))
    }

    /// Check if assets need to be extracted.
    ///
    /// Returns true if the assets directory doesn't exist, is incomplete,
    /// or the ObjectID.list uses an outdated format (fewer than 14 fields).
    pub fn needs_extraction(&self) -> bool {
        let assets_dir = match self.assets_dir.read().unwrap().clone() {
            Some(dir) => dir,
            None => {
                // Use default location
                default_assets_dir()
            }
        };

        let path = assets_dir.join("ObjectID.list");
        if !path.exists() {
            return true;
        }

        // Require the streamed CollectionIcon sprite sheet (added for the
        // Treasury forge-category filter). Missing it on an otherwise-valid
        // install means we must re-extract to generate it.
        if Self::required_assets_missing(&assets_dir) {
            return true;
        }

        // Check if the format is outdated (needs dye fields)
        Self::object_list_needs_format_update(&path)
    }

    /// Whether required extracted files are missing from `assets_dir`.
    ///
    /// Used to trigger re-extraction on installs that predate newly required
    /// files.
    fn required_assets_missing(assets_dir: &Path) -> bool {
        let collection_icon = assets_dir.join("sprites").join("CollectionIcon.png");
        if !collection_icon.exists() {
            tracing::info!(
                "[ASSETS] CollectionIcon.png missing - re-extraction needed for category icons"
            );
            return true;
        }
        let modifiers = assets_dir.join("xml").join("mods.xml");
        if !modifiers.exists() {
            tracing::info!(
                "[ASSETS] mods.xml missing - re-extraction needed for dungeon modifiers"
            );
            return true;
        }
        false
    }

    /// Check whether an ObjectID.list file uses the old format (< 14 fields).
    ///
    /// Field history:
    /// - 11 fields: added tex1/tex2 (dye data)
    /// - 12 fields: added slot_type (item categorization)
    /// - 13 fields: added defense (enemy DEF for self-computed damage)
    /// - 14 fields: added hitbox scale (enemy CustomHitbox scale for LS procs)
    fn object_list_needs_format_update(path: &Path) -> bool {
        if let Ok(content) = std::fs::read_to_string(path) {
            if let Some(line) = content
                .lines()
                .find(|l| !l.is_empty() && !l.starts_with('#'))
            {
                if line.split(';').count() < 14 {
                    tracing::info!(
                        "[ASSETS] ObjectID.list uses old format - re-extraction needed for enemy hitbox data"
                    );
                    return true;
                }
            }
        }
        false
    }

    /// Extract assets from the game's resources.assets file.
    ///
    /// Returns Ok(()) if extraction succeeded, or an error message.
    pub fn extract_assets(&self) -> Result<(), String> {
        // Find resources.assets
        let resources_path = find_resources_assets()
            .ok_or_else(|| "Could not find resources.assets - is RotMG installed?".to_string())?;

        tracing::info!("Found resources.assets at: {:?}", resources_path);

        // Determine output directory
        let output_dir = self
            .assets_dir
            .read()
            .unwrap()
            .clone()
            .unwrap_or_else(default_assets_dir);

        tracing::info!("Extracting assets to: {:?}", output_dir);

        // Create output directory
        std::fs::create_dir_all(&output_dir)
            .map_err(|e| format!("Failed to create assets directory: {}", e))?;

        // Extract
        let mut extractor = UnityExtractor::new();
        let result = extractor
            .extract(&resources_path, &output_dir)
            .map_err(|e| format!("Extraction failed: {}", e))?;

        tracing::info!(
            "Extraction complete: {} XML files, {} sprites, {} objects, {} tiles",
            result.xml_files,
            result.sprites,
            result.objects,
            result.tiles
        );

        // Set the assets directory if not already set
        if self.assets_dir.read().unwrap().is_none() {
            self.set_assets_dir(&output_dir);
        }

        // Save the resources.assets stamp so we can detect future game updates.
        // Note: live_settings will be patched in by initialize() after this returns.
        if let Some(stamp) = get_resources_assets_stamp() {
            Self::save_assets_stamp(stamp, None);
        }

        // Reload assets
        *self.initialized.write().unwrap() = false;
        // Invalidate the derived category cache so it rebuilds from the freshly
        // extracted object list rather than serving stale data.
        *self.dungeon_categories.write().unwrap() = None;
        self.try_load();

        if !result.errors.is_empty() {
            return Err(format!(
                "Extraction had {} errors: {:?}",
                result.errors.len(),
                result.errors
            ));
        }

        Ok(())
    }

    /// Initialize assets, extracting from game files if necessary.
    ///
    /// This is the recommended way to initialize assets on app startup.
    /// It checks whether extracted assets are stale (game was updated since
    /// last extraction) by comparing the `resources.assets` file modified
    /// time against the stamp stored in settings.
    pub fn initialize(
        &self,
        live_settings: Option<&std::sync::Arc<std::sync::RwLock<crate::settings::Settings>>>,
    ) -> Result<(), String> {
        // Try to find existing assets first
        if let Some(dir) = find_assets_dir() {
            self.set_assets_dir(&dir);

            // Check if assets are stale (game updated since last extraction)
            if self.assets_are_stale() {
                tracing::info!(
                    "[ASSETS] Game update detected - resources.assets changed, re-extracting"
                );
                self.extract_assets()?;
                // Update live settings so on_exit won't overwrite the stamp
                if let Some(stamp) = get_resources_assets_stamp() {
                    Self::save_assets_stamp(stamp, live_settings);
                }
                return Ok(());
            }

            if self.try_load() {
                // Check if ObjectID.list format is outdated (e.g., missing dye fields)
                // or a required sprite sheet (CollectionIcon) is missing.
                let needs_update = self
                    .assets_dir
                    .read()
                    .unwrap()
                    .as_ref()
                    .map(|d| {
                        Self::object_list_needs_format_update(&d.join("ObjectID.list"))
                            || Self::required_assets_missing(d)
                    })
                    .unwrap_or(false);
                if needs_update {
                    tracing::info!("[ASSETS] Re-extracting for ObjectID.list format update");
                    self.extract_assets()?;
                    if let Some(stamp) = get_resources_assets_stamp() {
                        Self::save_assets_stamp(stamp, live_settings);
                    }
                }
                return Ok(());
            }
        }

        // Need to extract
        if self.needs_extraction() {
            self.extract_assets()?;
            // Update live settings so on_exit won't overwrite the stamp
            if let Some(stamp) = get_resources_assets_stamp() {
                Self::save_assets_stamp(stamp, live_settings);
            }
        }

        Ok(())
    }

    /// Check if extracted assets are stale by comparing the current
    /// `resources.assets` modified time against the stamp saved in settings.
    ///
    /// Returns true when the game has been updated since assets were last
    /// extracted (i.e., a re-extraction is needed).
    pub fn assets_are_stale(&self) -> bool {
        let current_stamp = match get_resources_assets_stamp() {
            Some(s) => s,
            None => {
                tracing::debug!("[ASSETS] Cannot determine resources.assets stamp");
                return false; // Can't check, assume OK
            }
        };

        let saved_stamp = {
            use crate::settings::Settings;
            Settings::load().assets_stamp
        };

        match saved_stamp {
            Some(saved) if saved == current_stamp => {
                tracing::debug!(
                    "[ASSETS] Assets stamp matches ({}), no re-extraction needed",
                    current_stamp
                );
                false
            }
            Some(saved) => {
                tracing::info!(
                    "[ASSETS] Assets stamp mismatch: saved={}, current={}",
                    saved,
                    current_stamp
                );
                true
            }
            None => {
                // No saved stamp - we can't reliably determine if existing
                // assets match the current resources.assets, so re-extract.
                tracing::info!(
                    "[ASSETS] No saved assets stamp - re-extracting to ensure assets are current (resources.assets stamp: {})",
                    current_stamp
                );
                true
            }
        }
    }

    /// Save the current resources.assets stamp to settings.
    ///
    /// If `live_settings` is provided (the app's in-memory settings), it
    /// will also be updated so that `on_exit` doesn't overwrite the stamp.
    pub fn save_assets_stamp(
        stamp: u64,
        live_settings: Option<&std::sync::Arc<std::sync::RwLock<crate::settings::Settings>>>,
    ) {
        use crate::settings::Settings;

        // Always persist to disk
        let mut settings = Settings::load();
        settings.assets_stamp = Some(stamp);
        settings.save();

        // Also update the app's in-memory settings if available
        if let Some(live) = live_settings {
            if let Ok(mut s) = live.write() {
                s.assets_stamp = Some(stamp);
            }
        }

        tracing::info!("[ASSETS] Saved assets stamp: {}", stamp);
    }

    /// Get statistics about loaded assets.
    pub fn stats(&self) -> AssetStats {
        let objects = self.objects.read().unwrap();
        let tiles = self.tiles.read().unwrap();
        let sprites = self.sprites.read().unwrap();

        AssetStats {
            objects_loaded: objects.as_ref().map(|l| l.len()).unwrap_or(0),
            tiles_loaded: tiles.as_ref().map(|l| l.len()).unwrap_or(0),
            sprite_sheets: sprites.as_ref().map(|a| a.sheet_count()).unwrap_or(0),
            sprites_loaded: sprites.as_ref().map(|a| a.sprite_count()).unwrap_or(0),
        }
    }

    // ========== Dungeon Data (difficulty + item-dungeon mapping) ==========

    /// Get all item IDs that originate from a given dungeon.
    /// Derived from ORG_ labels on loaded items.
    pub fn dungeon_items(&self, dungeon_name: &str) -> Vec<i32> {
        self.try_load();
        let guard = self.dungeon_data.read().unwrap();
        match guard.as_ref() {
            Some(data) => data.items_for_dungeon(dungeon_name),
            None => Vec::new(),
        }
    }

    /// Get dungeon names that a given item originates from.
    /// Derived from ORG_ labels on the item.
    pub fn item_dungeons(&self, item_id: i32) -> Vec<&'static str> {
        self.try_load();
        let guard = self.dungeon_data.read().unwrap();
        match guard.as_ref() {
            Some(data) => data.dungeons_for_item(item_id),
            None => Vec::new(),
        }
    }

    /// Returns the set of object type IDs that have the BOSS label (excluding BOSS_SPAWNED).
    pub fn boss_type_ids(&self) -> HashSet<i32> {
        self.try_load();
        let guard = self.objects.read().unwrap();
        let Some(obj_list) = guard.as_ref() else {
            return HashSet::new();
        };
        obj_list
            .iter_with_ids()
            .filter(|(_, asset)| {
                let labels = &asset.labels;
                labels.split(',').any(|l| l == "BOSS")
                    && !labels.split(',').any(|l| l == "BOSS_SPAWNED")
            })
            .map(|(id, _)| *id)
            .collect::<HashSet<i32>>()
    }

    /// Get all item IDs that have the MARK label.
    pub fn mark_item_ids(&self) -> HashSet<i32> {
        self.try_load();
        let guard = self.objects.read().unwrap();
        let Some(obj_list) = guard.as_ref() else {
            return HashSet::new();
        };
        obj_list
            .iter_with_ids()
            .filter(|(_, asset)| asset.labels.split(',').any(|l| l == "MARK"))
            .map(|(id, _)| *id)
            .collect::<HashSet<i32>>()
    }

    /// Item ids of boss completion Marks (label `MARK` with an id name of the
    /// form "Mark of ..."). A Mark is a guaranteed, exclusive drop of its
    /// dungeon's main boss, so a bag holding one is definitively that boss's
    /// drop. Excludes non-boss `MARK`-labelled items (e.g. Essences, Vials).
    pub fn boss_mark_item_ids(&self) -> HashSet<i32> {
        self.try_load();
        let guard = self.objects.read().unwrap();
        let Some(obj_list) = guard.as_ref() else {
            return HashSet::new();
        };
        obj_list
            .iter_with_ids()
            .filter(|(_, asset)| {
                asset.labels.split(',').any(|l| l == "MARK") && asset.id_name.starts_with("Mark of")
            })
            .map(|(id, _)| *id)
            .collect::<HashSet<i32>>()
    }
}

impl Default for AssetManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics about loaded assets.
#[derive(Debug, Clone, Default)]
pub struct AssetStats {
    /// Number of objects loaded
    pub objects_loaded: usize,
    /// Number of tiles loaded
    pub tiles_loaded: usize,
    /// Number of sprite sheets
    pub sprite_sheets: usize,
    /// Total number of sprites
    pub sprites_loaded: usize,
}

/// Get the default assets directory.
pub fn default_assets_dir() -> PathBuf {
    // Prefer LocalAppData on Windows
    #[cfg(target_os = "windows")]
    {
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            return PathBuf::from(local_app_data)
                .join("RealmHound")
                .join("assets");
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(support) = dirs::data_local_dir() {
            return support.join("RealmHound").join("assets");
        }
    }

    // Fallback to current directory
    PathBuf::from("assets")
}

/// Default asset directory locations to try.
pub fn default_asset_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // Current working directory
    dirs.push(PathBuf::from("assets"));

    // Executable directory
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            dirs.push(exe_dir.join("assets"));
        }
    }

    // LocalAppData on Windows
    #[cfg(target_os = "windows")]
    {
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            dirs.push(
                PathBuf::from(local_app_data)
                    .join("RealmHound")
                    .join("assets"),
            );
        }
    }

    dirs
}

/// Get the modified-time stamp of the game's `resources.assets` file.
///
/// Returns the last modified time as seconds since Unix epoch,
/// or None if the file can't be found or its metadata can't be read.
pub fn get_resources_assets_stamp() -> Option<u64> {
    let path = find_resources_assets()?;
    let metadata = std::fs::metadata(&path).ok()?;
    let modified = metadata.modified().ok()?;
    let stamp = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(stamp)
}

/// Find the first valid assets directory from default locations.
pub fn find_assets_dir() -> Option<PathBuf> {
    for dir in default_asset_dirs() {
        let object_list = dir.join("ObjectID.list");
        if object_list.exists() {
            return Some(dir);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_asset_manager_new() {
        let manager = AssetManager::new();
        assert!(!manager.is_loaded());
        assert!(manager.assets_dir().is_none());
    }

    #[test]
    fn valid_drop_source_accepts_curated_crates_without_assets() {
        // Curated crates/bosses short-circuit before the catalog lookup, so a
        // known crate is a valid source even when no assets are loaded.
        let manager = AssetManager::new();
        assert!(!manager.is_loaded());
        assert!(manager.is_valid_drop_source(2078)); // Infested Chest (curated crate)
        assert!(manager.is_valid_drop_source(3372)); // Treasure Sarcophagus (curated crate)
                                                     // An arbitrary uncatalogued id is not a valid source (fail-closed here;
                                                     // the loot recording guard fails open on unknown-to-catalog types).
        assert!(!manager.is_valid_drop_source(999_999));
    }

    #[test]
    fn killer_sprite_overrides_include_lore_aliases() {
        // "King Azamoth" (RealmEye lore name) aliases to The Forgotten King, and
        // "Sunken Treasure" resolves to the Davy Jones' Locker chest.
        let find = |n: &str| {
            KILLER_SPRITE_ID_OVERRIDES
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(n))
                .map(|(_, id)| *id)
        };
        assert_eq!(find("King Azamoth"), Some(29039));
        assert_eq!(find("Sunken Treasure"), Some(28065));
    }

    #[test]
    fn test_parse_grave_tier() {
        assert_eq!(
            parse_grave_tier("Level 1 Default Gravestone"),
            Some(GraveTier {
                level: "1",
                fame: None
            })
        );
        assert_eq!(
            parse_grave_tier("Level 2-19 Voodoo Gravestone"),
            Some(GraveTier {
                level: "2-19",
                fame: None
            })
        );
        assert_eq!(
            parse_grave_tier("Level 20 Default Gravestone"),
            Some(GraveTier {
                level: "20",
                fame: None
            })
        );
        assert_eq!(
            parse_grave_tier("1/8 Default Gravestone"),
            Some(GraveTier {
                level: "20",
                fame: Some(1)
            })
        );
        assert_eq!(
            parse_grave_tier("8/8 Unity Supporter Gravestone"),
            Some(GraveTier {
                level: "20",
                fame: Some(8)
            })
        );
        // Unrecognized prefixes yield no tier.
        assert_eq!(parse_grave_tier("Gravestone"), None);
        assert_eq!(parse_grave_tier("9/8 Bogus Gravestone"), None);
        assert_eq!(parse_grave_tier("Level 99 Gravestone"), None);
    }

    #[test]
    fn boss_like_labels_match_real_bosses() {
        // Real bosses / mini-bosses / heroes / encounters (labels from ObjectID.list).
        assert!(labels_are_boss_like(
            "ENEMY,BOSS,BOSSFIGHT,QUEST,GOD",
            "Ocean Trench"
        )); // Thessal
        assert!(labels_are_boss_like(
            "ENEMY,MINION,UNDEAD,HERO,GOD,QUEST",
            "Heros"
        )); // Ghost King
        assert!(labels_are_boss_like("ENEMY,MINIBOSS,BOSSFIGHT,GOD", "")); // Pentaract Tower
        assert!(labels_are_boss_like("ENCOUNTER,Skull Shrine", "")); // Skull Shrine
        assert!(labels_are_boss_like("", "Hermit God Encounter")); // Hermit God (group)
                                                                   // Beacon Guardians (Heroes of Oryx) carry a dedicated BEACON_GUARDIAN label.
        assert!(labels_are_boss_like(
            "ENEMY,STASISIMMUNE,BEACON_GUARDIAN",
            ""
        )); // Legion Desert Guard
            // A real miniboss can also carry MINION_STRONG; the boss-like labels win.
        assert!(labels_are_boss_like(
            "ENEMY,MINION,GOD,HERO,QUEST,MINION_STRONG",
            ""
        )); // Cosmic Sprite
            // Tier-prefixed encounter markers alone still identify a boss: the newer
            // catalog gives some realm-event bosses only ADEPT_ENCOUNTER (Man-eating
            // Barnacle 47930, Artificial Slop 47973) with no plain ENCOUNTER token.
        assert!(labels_are_boss_like("ADEPT_ENCOUNTER", "")); // Man-eating Barnacle / Artificial Slop
        assert!(labels_are_boss_like(
            "ENEMY,ENCOUNTER,QUEST,VETERAN_ENCOUNTER",
            ""
        )); // Sentient Monolith
    }

    #[test]
    fn boss_like_labels_reject_trash_and_adds() {
        // GOD is a mechanical flag on mid-tier minions, not a boss marker.
        assert!(!labels_are_boss_like(
            "ENEMY,MINION,SPRITEFOREST,GOD,MINION_MID,ADEPT",
            ""
        )); // Noxious Sprite
            // BOSSFIGHT is carried by weak adds spawned during a fight.
        assert!(!labels_are_boss_like(
            "ENEMY,MINION,BOSS_SPAWNED,BOSSFIGHT,CRITTER,MINION_WEAK",
            ""
        )); // Small Worker Bee
        assert!(!labels_are_boss_like("ENEMY,MINION", "")); // plain trash
                                                            // Beacon Guardian adds/decoys/orbs lack the BEACON_GUARDIAN label.
        assert!(!labels_are_boss_like(
            "",
            "Beacon Guardian Dead Church Minion"
        )); // Reanimated Legion Minion
        assert!(!labels_are_boss_like("ENEMY", "")); // Legion Missionary beam
        assert!(!labels_are_boss_like("", "")); // untagged (e.g. Elder Sprite Tree - relies on HP fallback)
    }

    #[test]
    fn strong_boss_labels_reject_quest_only_minions() {
        // QUEST alone (the quest-arrow marker) does NOT promote a minion: Urgle
        // the Traptosser is realm trash despite carrying QUEST.
        assert!(!labels_are_strong_boss(
            "ENEMY,MINION,GROTESQUE,QUEST,MINION_MID",
            ""
        )); // Urgle

        // Genuine bosses still qualify without relying on QUEST.
        assert!(labels_are_strong_boss(
            "ENEMY,BOSS,BOSSFIGHT,QUEST,GOD",
            "Ocean Trench"
        )); // Thessal
        assert!(labels_are_strong_boss(
            "ENEMY,MINION,GROTESQUE,HERO,GOD,QUEST,MINION_STRONG",
            ""
        )); // Red Demon (Hero)
        assert!(labels_are_strong_boss("ENEMY,MINIBOSS,BOSSFIGHT,GOD", "")); // Pentaract Tower
        assert!(labels_are_strong_boss("ENCOUNTER,Skull Shrine", "")); // Skull Shrine
        assert!(labels_are_strong_boss("", "Hermit God Encounter")); // group
        assert!(labels_are_strong_boss(
            "ENEMY,STASISIMMUNE,BEACON_GUARDIAN",
            ""
        )); // Beacon Guardian

        // A tier-prefixed encounter label must NOT promote a minion add: the
        // Kogbold "EH" loot orbs are ENEMY,MINION,TRACKLOOT,VETERAN_ENCOUNTER and
        // stay non-boss (is_boss gates them through is_minion -> strong_boss).
        assert!(!labels_are_strong_boss(
            "ENEMY,MINION,TRACKLOOT,VETERAN_ENCOUNTER",
            ""
        ));
    }

    #[test]
    fn minion_labels_match_clones_and_stage_adds() {
        // Spawned adds / clones / prior stages: excluded from boss-fight tracking
        // even at boss-tier HP (they never drop soulbound loot).
        assert!(labels_are_minion(
            "ENEMY,MINION,ABANDONED,GOD,MINION_MID,ADEPT"
        )); // Mysterious Crystal
        assert!(labels_are_minion("ENEMY,MINION,HUMANOID,MINION_STRONG")); // Crystal Prisoner Clone
        assert!(labels_are_minion("ENEMY,MINION")); // plain trash
        assert!(labels_are_minion("ENEMY,SOMETHING,MINION_WEAK")); // tier label only
                                                                   // Cosmic Sprite is minion-tagged but is_boss() keeps it via boss-like
                                                                   // precedence (it also has HERO/QUEST); see TrackedObject::is_boss.
        assert!(labels_are_minion(
            "ENEMY,MINION,GOD,HERO,QUEST,MINION_STRONG"
        )); // Cosmic Sprite

        // Real bosses are NOT minions (MINIBOSS must not match MINION).
        assert!(!labels_are_minion(
            "ENEMY,BOSSFIGHT,MINIBOSS,HUMANOID,GOD,STASISIMMUNE,TRACKLOOT"
        )); // Crystal Prisoner
        assert!(!labels_are_minion("ENEMY,BOSS,BOSSFIGHT,QUEST,GOD")); // Thessal
        assert!(!labels_are_minion("")); // untagged setpiece (Sprite God / pumpkin)
        assert!(!labels_are_minion("ENEMY,STASISIMMUNE,BEACON_GUARDIAN")); // Legion Desert Guard
    }

    #[test]
    fn curated_boss_lists_are_sorted_and_scoped() {
        // The lists are binary-searched, so they must stay sorted.
        assert!(CURATED_BOSS_TYPES.windows(2).all(|w| w[0] < w[1]));
        assert!(CURATED_NON_BOSS_TYPES.windows(2).all(|w| w[0] < w[1]));
        // Sea Dragon is catalog-tagged MINION but is a real 50k-HP miniboss.
        assert!(is_curated_boss_type(1033)); // Sea Dragon
        assert!(is_curated_boss_type(22109)); // Kogbold Expedition Engine (realm)
        assert!(is_curated_boss_type(28796)); // Bartholomew the Massive Parrot (Deadwater Docks gate boss)
        assert!(is_curated_boss_type(33280)); // Shatters Stone Idol (secret boss)
                                              // Adept Hero of Oryx with no boss label and sub-threshold HP.
        assert!(is_curated_boss_type(21943)); // New Kage Kami (Adept Hero, 3000 HP, MINION-labeled)
                                              // Treasure-room loot chests promoted so they show on the dungeon card.
        assert!(is_curated_boss_type(620)); // Bilgewater's Booty A (Deadwater Docks)
        assert!(is_curated_boss_type(621)); // Bilgewater's Booty B
        assert!(is_curated_boss_type(622)); // Bilgewater's Booty C
        assert!(is_curated_boss_type(28065)); // Sunken Treasure (Davy Jones' Locker)
        assert!(is_curated_boss_type(28671)); // Old Chest (Mountain Temple)
                                              // Round 5: Tomb sarcophagi, Woodland secret chest, Crawling Depths egg sacs.
        assert!(is_curated_boss_type(3369)); // Active Sarcophagus (Tomb miniboss)
        assert!(is_curated_boss_type(3372)); // Treasure Sarcophagus (Tomb chest)
        assert!(is_curated_boss_type(9110)); // Woodland Skysplitter Stone (secret chest)
        for egg in [13208, 13209, 13210, 13211, 28797, 28798, 28799, 28810] {
            assert!(is_curated_boss_type(egg)); // Son of Arachna Giant Egg Sacs (4 colors x2 variants)
        }
        // Its small add and the ~2.5k-HP Deep Sea swarm gods are NOT bosses.
        assert!(!is_curated_boss_type(1031)); // Sea Dragon Carp (add)
        assert!(!is_curated_boss_type(29221)); // Abyssal Kraken (swarm add)
        assert!(!is_curated_boss_type(1214)); // Oryx Judge (O3 add)
                                              // Legion General summons are on the deny-list (label-less, boss-tier HP).
        assert!(is_curated_non_boss_type(16907)); // Galleon Parrot (Deadwater Docks add)
        assert!(is_curated_non_boss_type(24006)); // Legion Footsoldier
        assert!(is_curated_non_boss_type(24007)); // Legion Major
        assert!(is_curated_non_boss_type(24061)); // SpecPen Administration Turret
        for tutorial in [24454, 24455, 24456, 24457, 24458, 24459] {
            assert!(is_curated_non_boss_type(tutorial)); // SpecPen tutorial objects
        }
        // SpecPen interactive props (switches / gravestones): Character class,
        // boss-tier HP, no labels -- never real fights.
        for prop in [
            23708, 23747, 23840, 23932, 24412, 44354, 44355, 44356, 44357, 44358, 44359, 44360,
            44361,
        ] {
            assert!(is_curated_non_boss_type(prop)); // SpecPen switch / gravestone props
        }
        assert!(is_curated_non_boss_type(34547)); // World's Oyster Coral (add)
        assert!(is_curated_non_boss_type(34583)); // White Blood Cell (Bloodroot Heart add)
        assert!(is_curated_non_boss_type(51080)); // Bramblethorn Bud (Corrupted Bramblethorn add)
        assert!(is_curated_non_boss_type(45408)); // LH Spawn Pillar (Lost Halls spawner)
        assert!(is_curated_non_boss_type(16933)); // Flying Behemoth Tornado (add)
        assert!(is_curated_non_boss_type(16971)); // Sentient Monolith Protector (add)
        assert!(is_curated_non_boss_type(34462)); // Well of Souls Skeleton / Possessed Skeleton (add)
        assert!(is_curated_non_boss_type(34465)); // Galleon Admiral (Bilgewater's Galleon add)
        assert!(is_curated_non_boss_type(34587)); // Lich King Grave (add)
        assert!(is_curated_non_boss_type(51077)); // Aerial Warship Crew (add)
                                                  // Cube Deity respawnable swarming adds are hidden; only Cube Deity
                                                  // (47927) stays tracked.
        assert!(is_curated_non_boss_type(47928)); // Deity Overseer (add)
        assert!(is_curated_non_boss_type(47929)); // Deity Defender (add)
        assert!(!is_curated_non_boss_type(47927)); // Cube Deity (real boss)
                                                   // Ravenous Rot's last-phase tentacles are hidden; only Ravenous Rot
                                                   // (16990) stays tracked.
        assert!(is_curated_non_boss_type(16993)); // Ravenous Rot Overgrowth (tentacle)
        assert!(!is_curated_non_boss_type(16990)); // Ravenous Rot (real boss)
                                                   // Rat Extermination minigame minions are hidden; only Mammoth City Rat
                                                   // (18007, 75k HP) stays tracked.
        for rat in [18000, 18001, 18002, 18003, 18047, 18048, 18049, 18050] {
            assert!(is_curated_non_boss_type(rat)); // City Rat minions
        }
        assert!(!is_curated_non_boss_type(18007)); // Mammoth City Rat (real boss)
        assert!(is_curated_non_boss_type(34457)); // Baneserpent Head 1 (segment)
        assert!(is_curated_non_boss_type(34469)); // Baneserpent Neck 3 (segment)
        assert!(is_curated_non_boss_type(34474)); // Baneserpent Impact Telegraph (segment)
        assert!(!is_curated_non_boss_type(34456)); // Adult Baneserpent (real boss body)
        assert!(!is_curated_non_boss_type(24005)); // Legion General (real boss)
        assert!(!is_curated_non_boss_type(34538)); // World's Oyster (real boss)
        assert!(!is_curated_non_boss_type(34581)); // Bloodroot Heart (real boss)
        assert!(!is_curated_non_boss_type(51078)); // Corrupted Bramblethorn (real boss)
        assert!(!is_curated_non_boss_type(20744)); // Flying Behemoth (real boss)
        assert!(!is_curated_non_boss_type(16935)); // Sentient Monolith (real boss)
        assert!(!is_curated_non_boss_type(34460)); // Well of Souls (real boss)
        assert!(!is_curated_non_boss_type(34584)); // The Lich King (real boss)
        assert!(!is_curated_non_boss_type(51075)); // Aerial Warship (real boss)
                                                   // The two lists never overlap.
        assert!(!CURATED_BOSS_TYPES
            .iter()
            .any(|id| is_curated_non_boss_type(*id)));
        // The wandering-boss list is binary-searched too, so it must stay sorted.
        assert!(WANDERING_BOSS_TYPES.windows(2).all(|w| w[0] < w[1]));
        // Treasure-crate and special-boss lists are binary-searched.
        assert!(TREASURE_CRATE_TYPES.windows(2).all(|w| w[0] < w[1]));
        assert!(SPECIAL_BOSS_TYPES.windows(2).all(|w| w[0] < w[1]));
        assert!(is_treasure_crate_type(2078)); // Infested Chest
        assert!(is_treasure_crate_type(3372)); // Treasure Sarcophagus (Tomb of the Ancients)
        assert!(is_special_boss_type(44020)); // The Glitch
        assert!(!is_special_boss_type(1033)); // Sea Dragon is not a special boss
        assert!(is_wandering_boss_type(47387)); // Calamity Crab (Deadwater Docks)
        assert!(!is_wandering_boss_type(2352)); // Jon Bilgewater (stationary boss)
                                                // Commotion Crabs (modifier variant) must stay per-instance, not merged.
        assert!(!is_wandering_boss_type(17410)); // Commotion Crab (4-6 roaming instances)
                                                 // The self-destruct list is binary-searched too, so it must stay sorted.
        assert!(SELF_DESTRUCT_BOSS_TYPES.windows(2).all(|w| w[0] < w[1]));
        assert!(is_self_destruct_boss_type(22109)); // Kogbold Expedition Engine (realm train)
        assert!(!is_self_destruct_boss_type(50349)); // Kogbold Flying Machine (Steamworks, dies at 0 HP)
    }

    #[test]
    fn wanderer_event_spawns_are_post_boss_bonus() {
        // event Wanderers spawn on the dungeon boss's death, so they must
        // be demoted below the real boss (post-boss bonus), like the Prismimic.
        for wanderer in [14474, 14477, 17391, 18346] {
            assert!(
                is_post_boss_bonus_type(wanderer),
                "Wanderer {wanderer} is a post-boss bonus"
            );
        }
        // Prismimic halves remain post-boss bonuses.
        assert!(is_post_boss_bonus_type(19247));
        assert!(is_post_boss_bonus_type(19249));
        // The MOTMG realm / Hidden Interregnum main Wanderer stays a real boss.
        assert!(!is_post_boss_bonus_type(49755));
        assert!(!is_post_boss_bonus_type(45811)); // Tarul (Untaris main boss)
    }

    #[test]
    fn optional_secondary_bosses_are_demoted() {
        // Each listed optional side boss must be classified as an optional
        // secondary so anchor selection keeps the real main boss headlining.
        for opt in [
            2227, 3613, 8200, 14887, 15940, 16889, 24092, 25594, 29764, 43924, 46385, 47387,
        ] {
            assert!(
                is_optional_secondary_boss_type(opt),
                "{opt} is an optional secondary boss"
            );
        }
        // Real main bosses must NOT be demoted.
        for main in [4243, 2314, 1351, 25598, 3448, 2352, 43923] {
            assert!(
                !is_optional_secondary_boss_type(main),
                "{main} is a real main boss"
            );
        }
        // Event reskins are excluded so their standalone cards keep their main.
        assert!(!is_optional_secondary_boss_type(3894)); // MOTMG Beekeeper
        assert!(!is_optional_secondary_boss_type(13189)); // mgm2 Cursed Phantom
    }

    #[test]
    fn cultist_hideout_encounter_registry() {
        // Every phase-boss maps to the Cultist Hideout encounter.
        for member in [
            45215, 45217, 45219, 45221, 45223, 45225, 45227, 45228, 45229, 45230, 45231,
        ] {
            let enc = encounter_for_boss_type(member).expect("member maps to encounter");
            assert_eq!(enc.id, "cultist_hideout");
            assert_eq!(enc.display_name, "Malus");
            assert_eq!(enc.anchor_type, 45231);
        }
        // The anchor is a listed member.
        let enc = encounter_by_id("cultist_hideout").expect("encounter by id");
        assert!(enc.member_types.contains(&enc.anchor_type));
        // Cultist Followers (minion adds) and unrelated bosses are NOT members.
        assert!(encounter_for_boss_type(45141).is_none()); // Follower of Malus (add)
        assert!(encounter_for_boss_type(1033).is_none()); // Sea Dragon (unrelated)
        assert!(encounter_by_id("nope").is_none());
    }

    #[test]
    fn nest_encounter_registry_and_aux_targets() {
        // The Queen anchors the Nest card and is a listed member.
        let enc = encounter_by_id("nest").expect("nest encounter");
        assert_eq!(enc.display_name, "Killer Bee Queen");
        assert_eq!(enc.anchor_type, 4243);
        assert!(enc.member_types.contains(&enc.anchor_type));

        // Each per-color Beehemoth / Nest aggregates under its own row and groups
        // into the Queen card.
        let cases = [
            (4285, "nest_beehemoth_yellow", "Adolescent Yellow Beehemoth"),
            (4286, "nest_beehemoth_red", "Adolescent Red Beehemoth"),
            (4287, "nest_beehemoth_blue", "Adolescent Blue Beehemoth"),
            (17627, "nest_beehemoth_green", "Adolescent Green Beehemoth"),
            (4280, "nest_hive_yellow", "Yellow Killer Bee Nest"),
            (4279, "nest_hive_red", "Red Killer Bee Nest"),
            (4278, "nest_hive_blue", "Blue Killer Bee Nest"),
            (17503, "nest_hive_green", "Green Killer Bee Nest"),
        ];
        for (otype, category, name) in cases {
            let aux = aux_target_for_type(otype).expect("aux target for nest type");
            assert_eq!(aux.category, category);
            assert_eq!(aux.display_name, name);
            assert_eq!(aux.repr_type, otype);
            assert_eq!(
                encounter_for_boss_type(aux.repr_type).map(|e| e.id),
                Some("nest"),
                "aux repr {otype} must group into the nest card",
            );
        }
    }

    #[test]
    fn hornets_nest_encounter_and_angry_hornet_aux() {
        // The Nest anchors its card and is a listed member.
        let enc = encounter_by_id("hornets_nest").expect("hornets_nest encounter");
        assert_eq!(enc.display_name, "Hornet's Nest");
        assert_eq!(enc.anchor_type, 34603);
        assert!(enc.member_types.contains(&enc.anchor_type));

        // Angry Hornet is a curated non-boss add that aggregates into one row and
        // groups into the Nest card.
        assert!(is_curated_non_boss_type(34604));
        let aux = aux_target_for_type(34604).expect("angry hornet aux");
        assert_eq!(aux.category, "angry_hornet");
        assert_eq!(aux.display_name, "Angry Hornet");
        assert_eq!(aux.repr_type, 34604);
        assert_eq!(
            encounter_for_boss_type(34604).map(|e| e.id),
            Some("hornets_nest"),
        );
        // The Nest folds even in the Realm (Floral Escape set-piece).
        assert!(encounter_realm_grouped("hornets_nest"));
    }

    #[test]
    fn legion_missionary_encounter_and_orb_aux() {
        // The Missionary anchors its card and is a listed member.
        let enc = encounter_by_id("legion_missionary").expect("legion_missionary encounter");
        assert_eq!(enc.display_name, "Legion Missionary");
        assert_eq!(enc.anchor_type, 53013);
        assert!(enc.member_types.contains(&enc.anchor_type));

        // Each orb is a curated non-boss add that aggregates into its own row
        // (Light / Chaos) and groups into the Missionary card.
        for (orb, cat, name) in [
            (53017, "legion_orb_holy", "Orb of Light"),
            (53018, "legion_orb_chaos", "Orb of Chaos"),
        ] {
            assert!(
                is_curated_non_boss_type(orb),
                "orb {orb} must be curated non-boss"
            );
            let aux = aux_target_for_type(orb).unwrap_or_else(|| panic!("orb {orb} aux"));
            assert_eq!(aux.category, cat);
            assert_eq!(aux.display_name, name);
            assert_eq!(aux.repr_type, orb);
            assert_eq!(
                encounter_for_boss_type(orb).map(|e| e.id),
                Some("legion_missionary"),
                "orb {orb} must group into the Missionary card",
            );
        }
        // The Missionary folds even in the Realm (Beacon Guardian set-piece).
        assert!(encounter_realm_grouped("legion_missionary"));
    }

    #[test]
    fn every_aux_repr_type_belongs_to_an_encounter() {
        // Invariant: an aggregated summary can only surface if its representative
        // type is a member of some encounter (otherwise it would never group).
        for (_types, aux) in AUX_TARGETS {
            assert!(
                encounter_for_boss_type(aux.repr_type).is_some(),
                "aux '{}' repr_type {} has no encounter",
                aux.category,
                aux.repr_type,
            );
        }
    }

    #[test]
    fn kogbold_steamworks_encounter_and_shield_generator() {
        let enc = encounter_by_id("kogbold_steamworks").expect("ksw encounter");
        assert_eq!(enc.display_name, "Factory Control Core");
        assert_eq!(enc.anchor_type, 50391);
        assert!(enc.member_types.contains(&enc.anchor_type));
        // The Control Core groups into its own card.
        assert_eq!(
            encounter_for_boss_type(50391).map(|e| e.id),
            Some("kogbold_steamworks")
        );
        // Shield Generators sum into one row grouped under the Core.
        let aux = aux_target_for_type(50388).expect("shield generator aux");
        assert_eq!(aux.category, "shield_generator");
        assert_eq!(aux.display_name, "Shield Generator");
        assert_eq!(
            encounter_for_boss_type(aux.repr_type).map(|e| e.id),
            Some("kogbold_steamworks")
        );
    }

    #[test]
    fn spectral_penitentiary_encounters_and_aux() {
        // Lobotomik's separate transformation forms all fold into his card.
        let lobo = encounter_by_id("spec_pen_lobotomik").expect("lobotomik encounter");
        assert_eq!(lobo.anchor_type, 23920);
        for form in [23920, 23958, 23959, 23960, 23961, 23934, 23935] {
            assert_eq!(
                encounter_for_boss_type(form).map(|e| e.id),
                Some("spec_pen_lobotomik")
            );
        }
        // Oculon: Eyesmall minions sum into one row grouped under Oculon.
        let eyes = aux_target_for_type(23509).expect("eyesmall aux");
        assert_eq!(eyes.category, "overseer_eyesmall");
        assert_eq!(
            aux_target_for_type(44197).map(|a| a.category),
            Some("overseer_eyesmall")
        );
        assert_eq!(
            encounter_for_boss_type(eyes.repr_type).map(|e| e.id),
            Some("spec_pen_oculon")
        );
        // Murcian: Spectral Keys sum into one row grouped under Murcian.
        let key = aux_target_for_type(23804).expect("spectral key aux");
        assert_eq!(key.category, "spectral_key");
        assert_eq!(key.display_name, "Spectral Key");
        assert_eq!(
            encounter_for_boss_type(key.repr_type).map(|e| e.id),
            Some("spec_pen_murcian")
        );
        // Gretch and Zole are deliberately NOT curated (plain auto-detected fights).
        assert!(encounter_for_boss_type(23819).is_none()); // Gretch
        assert!(encounter_for_boss_type(23659).is_none()); // Zole
    }

    #[test]
    fn ice_citadel_groups_esben_forms() {
        let enc = encounter_by_id("ice_citadel").expect("ice citadel encounter");
        assert_eq!(enc.display_name, "Esben the Neurotic");
        assert_eq!(enc.anchor_type, 40166);
        // Esben and both summoned forms fold into one card.
        for form in [40166, 40177, 40191] {
            assert_eq!(
                encounter_for_boss_type(form).map(|e| e.id),
                Some("ice_citadel")
            );
        }
        // Esben the Unwilling (the separate Ice Cave boss) is NOT part of it.
        assert!(encounter_for_boss_type(1887).is_none());
    }

    #[test]
    fn shatters_twilight_archmage_and_stone_idol() {
        let enc = encounter_by_id("twilight_archmage").expect("archmage encounter");
        assert_eq!(enc.anchor_type, 29021);
        assert_eq!(
            encounter_for_boss_type(29021).map(|e| e.id),
            Some("twilight_archmage")
        );

        // Three birds (each its own row) + all generators summed into one row.
        let cases: &[(i32, &str, &str)] = &[
            (29341, "archmage_inferno", "Inferno"),
            (29342, "archmage_blizzard", "Blizzard"),
            (17490, "archmage_tempest", "Tempest"),
            (33054, "archmage_generators", "Twilight Archmage Generators"),
            (33072, "archmage_generators", "Twilight Archmage Generators"),
        ];
        for (otype, category, name) in cases {
            let aux = aux_target_for_type(*otype).expect("aux target for shatters type");
            assert_eq!(aux.category, *category);
            assert_eq!(aux.display_name, *name);
            assert_eq!(
                encounter_for_boss_type(aux.repr_type).map(|e| e.id),
                Some("twilight_archmage"),
                "aux repr for {otype} must group into the Twilight Archmage card",
            );
        }

        // The Stone Idol is a curated standalone boss (no encounter grouping).
        assert!(is_curated_boss_type(33280));
        assert!(encounter_for_boss_type(33280).is_none());
    }

    #[test]
    fn legacy_shatters_only_tracks_three_bosses() {
        // The three real bosses are all curated allow-listed: Forgotten Sentinel,
        // Twilight Archmage and the King (Retro shtrs The Forgotten King 52560).
        assert!(is_curated_boss_type(52556)); // The Forgotten Sentinel
        assert!(is_curated_boss_type(52557)); // Twilight Archmage
        assert!(is_curated_boss_type(52560)); // The Forgotten King (final)
        assert!(!is_curated_non_boss_type(52556));
        assert!(!is_curated_non_boss_type(52557));
        assert!(!is_curated_non_boss_type(52560));

        // Blizzard/Inferno fold into the Twilight Archmage card as aux rows.
        let cases: &[(i32, &str, &str)] = &[
            (52563, "legacy_archmage_inferno", "Inferno"),
            (52564, "legacy_archmage_blizzard", "Blizzard"),
        ];
        for (otype, category, name) in cases {
            let aux = aux_target_for_type(*otype).expect("aux target for legacy shatters bird");
            assert_eq!(aux.category, *category);
            assert_eq!(aux.display_name, *name);
            assert_eq!(
                encounter_for_boss_type(aux.repr_type).map(|e| e.id),
                Some("legacy_twilight_archmage"),
            );
        }
        assert_eq!(
            encounter_for_boss_type(52557).map(|e| e.id),
            Some("legacy_twilight_archmage"),
        );

        // Boss 1 gate: the four Titanums fold into one row, the two Paladin
        // Obelisks into another; both resolve to the Forgotten Sentinel card.
        for (otype, category) in [
            (52532, "legacy_bridge_titanum"),
            (52533, "legacy_bridge_titanum"),
            (52535, "legacy_bridge_titanum"),
            (52536, "legacy_bridge_titanum"),
            (52534, "legacy_paladin_obelisk"),
            (52537, "legacy_paladin_obelisk"),
        ] {
            let aux = aux_target_for_type(otype).expect("gate aux target");
            assert_eq!(aux.category, category);
            assert_eq!(
                encounter_for_boss_type(aux.repr_type).map(|e| e.id),
                Some("legacy_forgotten_sentinel"),
            );
        }
        assert_eq!(
            encounter_for_boss_type(52556).map(|e| e.id),
            Some("legacy_forgotten_sentinel"),
        );

        // Boss 3 gate: Royal Guardian L folds into one row and each crystal into
        // its own row; all resolve to the Forgotten King card. Royal Guardian J
        // stays hidden (spawns after the King is already vulnerable).
        for (otype, category) in [
            (52558, "legacy_royal_guardian"),
            (52572, "legacy_green_crystal"),
            (52573, "legacy_yellow_crystal"),
            (52574, "legacy_red_crystal"),
            (52575, "legacy_blue_crystal"),
        ] {
            let aux = aux_target_for_type(otype).expect("gate aux target");
            assert_eq!(aux.category, category);
            assert_eq!(
                encounter_for_boss_type(aux.repr_type).map(|e| e.id),
                Some("legacy_forgotten_king"),
            );
        }
        assert_eq!(
            encounter_for_boss_type(52560).map(|e| e.id),
            Some("legacy_forgotten_king"),
        );
        assert!(is_curated_non_boss_type(52559)); // Royal Guardian J hidden
        assert!(aux_target_for_type(52559).is_none());

        // Every other Retro shtrs arena entity is curated out (the three real
        // bosses and the gate aux entities above are excluded from this list).
        for trash in [
            52513, 52522, 52531, 52538, 52539, 52541, 52542, 52545, 52548, 52551, 52552, 52555,
            52559, 52585, 52590, 52592,
        ] {
            assert!(
                is_curated_non_boss_type(trash),
                "{trash} must be curated non-boss"
            );
            assert!(!is_curated_boss_type(trash));
        }
    }

    #[test]
    fn legacy_woodland_murderous_megamoth_groups_three_forms() {
        let enc = encounter_by_id("legacy_murderous_megamoth").expect("megamoth encounter");
        assert_eq!(enc.display_name, "Murderous Megamoth");
        assert_eq!(enc.anchor_type, 52665);
        // All three self-destructing forms fold under the final Murderous form.
        for form in [52662, 52663, 52665] {
            assert_eq!(
                encounter_for_boss_type(form).map(|e| e.id),
                Some("legacy_murderous_megamoth"),
            );
            assert!(
                aux_target_for_type(form).is_none(),
                "{form} is a real phase, not aux"
            );
        }
        // The Larva/Mammoth are allow-listed so they always classify as bosses.
        assert!(is_curated_boss_type(52662));
        assert!(is_curated_boss_type(52663));
        // Woodland trash (Mecha Squirrel, turrets, goblins) is curated out so
        // the Murderous Megamoth anchors the grouped card.
        for trash in [52653, 52654, 52655, 52657, 52658, 52659, 52660] {
            assert!(
                is_curated_non_boss_type(trash),
                "{trash} must be curated non-boss"
            );
        }
    }

    #[test]
    fn forax_waste_splits_group_into_one_card() {
        for (id, name, anchor, members) in [
            ("forax_waste", "Waste", 45917, [45917, 56070, 56071]),
            ("neo_forax_waste", "Neo Waste", 56390, [56390, 56391, 56392]),
        ] {
            let enc = encounter_by_id(id).expect("forax encounter");
            assert_eq!(enc.display_name, name);
            assert_eq!(enc.anchor_type, anchor);
            for form in members {
                assert_eq!(
                    encounter_for_boss_type(form).map(|e| e.id),
                    Some(id),
                    "Waste form {form} must fold into {id}",
                );
            }
        }
    }

    #[test]
    fn oryx_sanctuary_leucoryx_orbs_and_oryx_messengers() {
        let leu = encounter_by_id("o3_leucoryx").expect("leucoryx encounter");
        assert_eq!(leu.anchor_type, 6622);
        let oryx = encounter_by_id("o3_oryx").expect("oryx encounter");
        assert_eq!(oryx.anchor_type, 45363);

        let cases: &[(i32, &str, &str, &str)] = &[
            (6617, "o3_orb_light", "Orb of Light", "o3_leucoryx"),
            (6618, "o3_orb_chaos", "Orb of Chaos", "o3_leucoryx"),
            (45365, "o3_messengers", "Messengers", "o3_oryx"),
            (45431, "o3_messengers", "Messengers", "o3_oryx"),
            (45445, "o3_messengers", "Messengers", "o3_oryx"),
        ];
        for (otype, category, name, enc_id) in cases {
            let aux = aux_target_for_type(*otype).expect("aux target for o3 type");
            assert_eq!(aux.category, *category);
            assert_eq!(aux.display_name, *name);
            assert_eq!(
                encounter_for_boss_type(aux.repr_type).map(|e| e.id),
                Some(*enc_id),
                "aux repr for {otype} must group into {enc_id}",
            );
        }
    }

    #[test]
    fn shaitan_hands_aggregate_into_one_row() {
        // Every real Left/Right Hand of Shaitan (both md2 and md1 variants) maps
        // to a single "Hand of Shaitan" summary row.
        for otype in [28059, 28060, 28061, 29724, 29725, 29739, 29740] {
            let aux = aux_target_for_type(otype).expect("aux target for shaitan hand");
            assert_eq!(aux.category, "shaitan_hands");
            assert_eq!(aux.display_name, "Hand of Shaitan");
        }
        // The invisible spawner/playerlock and fake decoy hands are excluded.
        for otype in [28167, 28177, 29729, 29730] {
            assert!(aux_target_for_type(otype).is_none());
        }
        // The Hand summary groups into the Lair of Shaitan card.
        let aux = aux_target_for_type(28059).expect("shaitan hand aux");
        assert_eq!(
            encounter_for_boss_type(aux.repr_type).map(|e| e.id),
            Some("lair_of_shaitan"),
        );
        // Both Head variants belong to the same card but are real bosses, never
        // aux rows themselves.
        for head in [28058, 29723] {
            assert_eq!(
                encounter_for_boss_type(head).map(|e| e.id),
                Some("lair_of_shaitan"),
            );
            assert!(aux_target_for_type(head).is_none());
        }
    }

    #[test]
    fn astral_rift_spawns_aggregate_and_pumpkin_is_non_boss() {
        // Astral Guard and Astral Beast are summed into one "Astral Rift Spawns"
        // row, and are curated non-boss so they route to the aggregate instead of
        // spawning a card each (their event-scaled HP otherwise passes the
        // HP-only boss fallback).
        for otype in [51062, 51063] {
            let aux = aux_target_for_type(otype).expect("aux target for astral spawn");
            assert_eq!(aux.category, "astral_rift_spawns");
            assert_eq!(aux.display_name, "Astral Rift Spawns");
            assert!(is_curated_non_boss_type(otype));
        }
        // The spawns row belongs to the Astral Rift encounter; the Rift itself is
        // a real boss in that encounter, never an aux row.
        let aux = aux_target_for_type(51062).expect("astral spawn aux");
        assert_eq!(
            encounter_for_boss_type(aux.repr_type).map(|e| e.id),
            Some("astral_rift")
        );
        assert_eq!(
            encounter_for_boss_type(51061).map(|e| e.id),
            Some("astral_rift")
        );
        assert!(aux_target_for_type(51061).is_none());
        // Possessed Small Pumpkin is removed from tracking entirely.
        assert!(is_curated_non_boss_type(51034));
        assert!(aux_target_for_type(51034).is_none());
    }

    #[test]
    fn hermit_god_only_tracks_boss_and_tentacle() {
        // Both variants of the Hermit God are real bosses in one encounter.
        for boss in [3425, 22023] {
            assert_eq!(
                encounter_for_boss_type(boss).map(|e| e.id),
                Some("hermit_god")
            );
            assert!(!is_curated_non_boss_type(boss));
        }
        // Every tentacle (legacy + New) sums into one "Hermit God Tentacle" row.
        for t in [3428, 22026] {
            let aux = aux_target_for_type(t).expect("tentacle aux");
            assert_eq!(aux.category, "hermit_tentacle");
            assert_eq!(aux.display_name, "Hermit God Tentacle");
            assert!(is_curated_non_boss_type(t));
        }
        // Minions, whirlpools and invisible tentacle spawners are hidden.
        for hidden in [3426, 3427, 3429, 22024, 22025, 22027] {
            assert!(is_curated_non_boss_type(hidden), "{hidden} must be hidden");
            assert!(aux_target_for_type(hidden).is_none());
        }
        // Hermit God groups in the Realm.
        assert!(encounter_realm_grouped("hermit_god"));
    }

    #[test]
    fn plague_doctor_groups_lethal_cure() {
        assert_eq!(
            encounter_for_boss_type(34489).map(|e| e.id),
            Some("the_plague_doctor")
        );
        // All Lethal Cure orbs (A/B/C plus effect forms) sum into one row.
        for cure in [34490, 34491, 34492, 34504, 34505, 34506] {
            let aux = aux_target_for_type(cure).expect("cure aux");
            assert_eq!(aux.category, "lethal_cure");
            assert_eq!(aux.display_name, "Lethal Cure");
            assert!(is_curated_non_boss_type(cure));
        }
        assert_eq!(
            encounter_for_boss_type(34490).map(|e| e.id),
            Some("the_plague_doctor")
        );
        assert!(encounter_realm_grouped("the_plague_doctor"));
    }

    #[test]
    fn mountain_temple_groups_daichi_and_elementals() {
        let enc = encounter_by_id("mountain_temple").expect("mountain temple encounter");
        assert_eq!(enc.display_name, "Daichi the Fallen");
        assert_eq!(enc.anchor_type, 43681);
        // Daichi plus his four summoned elementals fold into one card.
        for form in [43681, 43696, 43698, 43700, 43701] {
            assert_eq!(
                encounter_for_boss_type(form).map(|e| e.id),
                Some("mountain_temple"),
                "form {form} must fold into mountain_temple",
            );
        }
    }

    #[test]
    fn realm_event_encounters_registry_209() {
        // Every realm event is registered, realm-grouped, and its
        // combined-add repr types resolve back into the right encounter.
        let cases: &[(&str, &str, i32)] = &[
            ("worlds_oyster", "World's Oyster", 34538),
            ("jade_garnet_statues", "Jade and Garnet Statues", 22149),
            ("flying_behemoth", "Flying Behemoth", 20744),
            ("crab_sovereign", "Crab Sovereign", 51072),
            ("killer_bee_nest", "Killer Bee Nest", 4312),
            ("sentient_monolith", "Sentient Monolith", 16935),
            ("eye_of_the_storm", "Eye of the Storm", 51031),
            ("pentaract", "Pentaract", 22019),
            ("assembled_giant", "Assembled Giant", 34485),
        ];
        for (id, name, anchor) in cases {
            let enc = encounter_by_id(id).unwrap_or_else(|| panic!("{id} missing"));
            assert_eq!(enc.display_name, *name);
            assert_eq!(enc.anchor_type, *anchor);
            assert!(encounter_realm_grouped(id), "{id} must be realm-grouped");
        }

        // Aggregated add rows route into their encounter and are curated non-boss.
        let aux_cases: &[(i32, &str, &str, &str)] = &[
            (34546, "worlds_pearl", "World's Pearl", "worlds_oyster"),
            (51073, "royal_crab", "Royal Crab", "crab_sovereign"),
            (
                4238,
                "mini_bee_nest",
                "Mini Killer Bee Nest",
                "killer_bee_nest",
            ),
            (
                21967,
                "killer_pillars",
                "Killer Pillar",
                "avatar_forgotten_king",
            ),
            (
                21963,
                "shades_of_avatar",
                "Shades of the Avatar",
                "avatar_forgotten_king",
            ),
            (
                21976,
                "eye_of_avatar",
                "Eye of the Avatar",
                "avatar_forgotten_king",
            ),
            (
                16936,
                "monolith_siphon",
                "Monolith Siphon",
                "sentient_monolith",
            ),
            (51033, "eye_storm_tornado", "Tornado", "eye_of_the_storm"),
        ];
        for (otype, cat, name, enc_id) in aux_cases {
            let aux = aux_target_for_type(*otype).unwrap_or_else(|| panic!("aux {otype}"));
            assert_eq!(aux.category, *cat);
            assert_eq!(aux.display_name, *name);
            assert_eq!(
                encounter_for_boss_type(aux.repr_type).map(|e| e.id),
                Some(*enc_id)
            );
        }
        // Mini Killer Bee Nest is a MINION add (already non-boss), the rest of the
        // above adds pass the HP fallback so must be curated non-boss.
        for otype in [34546, 51073, 21967, 21963, 21976, 16936, 51033] {
            assert!(
                is_curated_non_boss_type(otype),
                "{otype} must be curated non-boss"
            );
        }
    }

    #[test]
    fn avatar_forgotten_king_realm_groups_all_adds_286() {
        // The Avatar spawns as a realm event, so its card must group in the
        // Realm (not only inside The Shatters dungeon).
        assert!(encounter_realm_grouped("avatar_forgotten_king"));

        let enc = encounter_by_id("avatar_forgotten_king").expect("avatar encounter");
        assert_eq!(enc.display_name, "Avatar of the Forgotten King");
        assert_eq!(enc.anchor_type, 21959);
        // The Avatar itself is a real boss (never an aux add), so it anchors the
        // run and the card's Completed/Escaped status follows it -- not a minion.
        assert!(aux_target_for_type(21959).is_none());
        assert!(aux_target_for_type(29517).is_none());
        assert!(encounter_completion_any("avatar_forgotten_king").is_none());
        assert!(encounter_completion_all("avatar_forgotten_king").is_none());

        // Every Avatar add -- shades, eyes and the newly grouped ShadowBombs
        // (new + legacy) -- is a curated non-boss that folds into the Avatar
        // card, so no minion earns its own fight card.
        let shadowbombs = [21964, 21965, 34448, 34449];
        for otype in shadowbombs {
            assert!(
                is_curated_non_boss_type(otype),
                "{otype} must be curated non-boss"
            );
            let aux = aux_target_for_type(otype).unwrap_or_else(|| panic!("aux {otype}"));
            assert_eq!(aux.category, "shadowbombs");
            assert_eq!(aux.display_name, "ShadowBombs");
            assert_eq!(
                encounter_for_boss_type(aux.repr_type).map(|e| e.id),
                Some("avatar_forgotten_king"),
            );
        }
        // Shades and Eyes still route into the Avatar card.
        for (otype, cat) in [(21963, "shades_of_avatar"), (21976, "eye_of_avatar")] {
            let aux = aux_target_for_type(otype).unwrap_or_else(|| panic!("aux {otype}"));
            assert_eq!(aux.category, cat);
            assert_eq!(
                encounter_for_boss_type(aux.repr_type).map(|e| e.id),
                Some("avatar_forgotten_king"),
            );
        }
    }

    #[test]
    fn statues_and_behemoth_are_real_boss_rows_not_aux() {
        // Both statues (new + legacy) and the Behemoth's Egg are real bosses shown
        // as their own phase rows, not aggregated summary rows.
        for otype in [22149, 22150, 28619, 28618] {
            assert_eq!(
                encounter_for_boss_type(otype).map(|e| e.id),
                Some("jade_garnet_statues")
            );
            assert!(aux_target_for_type(otype).is_none());
        }
        for otype in [20744, 20799] {
            assert_eq!(
                encounter_for_boss_type(otype).map(|e| e.id),
                Some("flying_behemoth")
            );
            assert!(aux_target_for_type(otype).is_none());
        }
        // The three event Beehemoths are real boss rows inside the Killer Bee Nest.
        for otype in [4324, 4325, 4326] {
            assert_eq!(
                encounter_for_boss_type(otype).map(|e| e.id),
                Some("killer_bee_nest")
            );
            assert!(aux_target_for_type(otype).is_none());
        }
    }

    #[test]
    fn pentaract_and_assembled_giant_completion() {
        // Pentaract towers are real boss rows; the card completes on any tower.
        for tower in [1337, 3422, 22018] {
            assert_eq!(
                encounter_for_boss_type(tower).map(|e| e.id),
                Some("pentaract")
            );
            assert!(aux_target_for_type(tower).is_none());
        }
        assert_eq!(
            encounter_completion_any("pentaract"),
            Some(&[1337, 3422, 22018][..])
        );
        // Killer Bee Nest completes on any of the three Beehemoths.
        assert_eq!(
            encounter_completion_any("killer_bee_nest"),
            Some(&[4324, 4325, 4326][..])
        );
        // Assembled Giant parts are real boss rows; the card completes if any
        // part (Head / Body / Arm) was killed.
        for part in [34480, 34485, 34486] {
            assert_eq!(
                encounter_for_boss_type(part).map(|e| e.id),
                Some("assembled_giant")
            );
            assert!(aux_target_for_type(part).is_none());
        }
        assert_eq!(
            encounter_completion_any("assembled_giant"),
            Some(&[34480, 34485, 34486][..])
        );
        assert_eq!(
            encounter_by_id("assembled_giant").unwrap().anchor_type,
            34485
        );
    }

    #[test]
    fn tomb_of_the_ancients_requires_all_three_bosses() {
        // all three main bosses (Nut, Geb, Bes) map to the encounter and
        // gate completion; the two Sarcophagi are members so a Sarcophagus-only
        // card still resolves to the encounter (and is thus Escaped, not killed).
        for boss in [3366, 3367, 3368, 3369, 3372] {
            assert_eq!(
                encounter_for_boss_type(boss).map(|e| e.id),
                Some("tomb_of_the_ancients"),
            );
        }
        assert_eq!(
            encounter_completion_all("tomb_of_the_ancients"),
            Some(&[3366, 3367, 3368][..]),
        );
        // It uses the all-of rule, never the any-of rule.
        assert_eq!(encounter_completion_any("tomb_of_the_ancients"), None);
        // The Treasure Sarcophagus is categorized as a loot crate.
        assert!(is_treasure_crate_type(3372));
    }

    #[test]
    fn manor_coffin_is_curated_crate() {
        // the Coffin ships as a MINION_STRONG, so it must be curated to be
        // tracked, and classified as a treasure crate.
        assert!(is_curated_boss_type(5943));
        assert!(is_treasure_crate_type(5943));
    }

    #[test]
    fn reward_crates_classify_as_treasure_crates() {
        // Ruins Lamp (Genie reward crate) and DS Golden Rat (wandering loot
        // piñata) ship as minions, so they must be curated to be tracked and
        // classified as crates.
        for otype in [28867, 29023] {
            assert!(is_curated_boss_type(otype), "{otype} must be curated");
            assert!(is_treasure_crate_type(otype), "{otype} must be a crate");
        }
        // DS Master Rat Box ships as a MINIBOSS, so it is already tracked and
        // only needs the crate classification to move out of the miniboss band.
        assert!(is_treasure_crate_type(46404));
    }

    #[test]
    fn towering_perfection_realm_encounter() {
        // The core (47909), both Imperfection segments (Lower 47916 / Upper
        // 47917) and the four Toppled Cubes (44894 / 44895 / 47914 / 47915) fold
        // into one realm-grouped card anchored on the core; each is a real phase
        // row, not an aggregated aux summary.
        for otype in [47909, 47916, 47917, 44894, 44895, 47914, 47915] {
            assert_eq!(
                encounter_for_boss_type(otype).map(|e| e.id),
                Some("towering_perfection"),
            );
            assert!(aux_target_for_type(otype).is_none());
            // Every member is dedup-prone so respawn waves collapse to one row.
            assert!(is_dedup_prone_boss(otype), "{otype} must be dedup-prone");
        }
        let enc = encounter_by_id("towering_perfection").unwrap();
        assert_eq!(enc.anchor_type, 47909);
        assert_eq!(enc.display_name, "Towering Perfection");
        assert!(encounter_realm_grouped("towering_perfection"));
        // Headline-only: the card's name and picture are always the core's, even
        // when only the escaping segments took damage.
        assert!(encounter_headline_only("towering_perfection"));
        // A loot bag from the core latches the card to Completed; the segments do
        // not (they are damage sponges that routinely escape).
        assert_eq!(encounter_loot_completes(47909), Some("towering_perfection"));
        assert_eq!(encounter_loot_completes(47916), None);
        assert_eq!(encounter_loot_completes(47917), None);
    }

    #[test]
    fn legacy_lod_dragons_complete_on_chest_and_alias_loot() {
        // (dragon, chest) pairs: Nikao, Feargus, Limoz, Pyyr.
        let pairs = [
            (29849, 30009),
            (29978, 30049),
            (30017, 30035),
            (30019, 30034),
        ];
        assert_eq!(lod_dragon_chest_pairs(), &pairs);
        for (dragon, chest) in pairs {
            // Each chest spawn is a completion marker for exactly its dragon, so
            // the invulnerable-finish dragon fight scores Completed.
            assert_eq!(completion_marker_targets_for(chest), Some(&[dragon][..]));
            assert!(is_invuln_finish_boss(dragon), "{dragon} is invuln-finish");
            // The dragon <-> chest loot alias round-trips both ways.
            assert_eq!(loot_emitter_for_boss(dragon), Some(chest));
            assert_eq!(boss_for_loot_emitter(chest), Some(dragon));
        }
        // A boss that emits its own loot has no chest alias.
        assert_eq!(loot_emitter_for_boss(47927), None);
        assert_eq!(boss_for_loot_emitter(47927), None);
    }

    #[test]
    fn animal_merchant_realm_encounter() {
        // the Merchant and both spawns are curated bosses that fold into
        // one realm-grouped card anchored on the Merchant.
        for otype in [24023, 24020, 24021] {
            assert!(is_curated_boss_type(otype));
            assert_eq!(
                encounter_for_boss_type(otype).map(|e| e.id),
                Some("animal_merchant"),
            );
            assert!(aux_target_for_type(otype).is_none());
        }
        let enc = encounter_by_id("animal_merchant").unwrap();
        assert_eq!(enc.anchor_type, 24023);
        assert!(enc.member_types.contains(&enc.anchor_type));
        assert!(encounter_realm_grouped("animal_merchant"));
        // Anchor-based completion (not COMPLETION_ANY): the Merchant is a real
        // killable boss, so a dead spawn must not mark the card complete.
        assert_eq!(encounter_completion_any("animal_merchant"), None);
    }

    #[test]
    fn curated_non_boss_list_stays_sorted() {
        assert!(
            CURATED_NON_BOSS_TYPES.windows(2).all(|w| w[0] < w[1]),
            "CURATED_NON_BOSS_TYPES must be sorted and unique for binary_search",
        );
    }

    #[test]
    fn test_set_assets_dir() {
        let manager = AssetManager::new();
        manager.set_assets_dir("/test/path");
        assert_eq!(manager.assets_dir(), Some(PathBuf::from("/test/path")));
    }

    #[test]
    fn required_assets_include_mods_xml() {
        let dir = tempfile::tempdir().unwrap();
        let sprites_dir = dir.path().join("sprites");
        let xml_dir = dir.path().join("xml");
        std::fs::create_dir_all(&sprites_dir).unwrap();
        std::fs::create_dir_all(&xml_dir).unwrap();
        std::fs::write(sprites_dir.join("CollectionIcon.png"), []).unwrap();

        assert!(AssetManager::required_assets_missing(dir.path()));

        std::fs::write(xml_dir.join("mods.xml"), []).unwrap();
        assert!(!AssetManager::required_assets_missing(dir.path()));
    }

    #[test]
    fn test_stats_empty() {
        let manager = AssetManager::new();
        let stats = manager.stats();
        assert_eq!(stats.objects_loaded, 0);
        assert_eq!(stats.tiles_loaded, 0);
        assert_eq!(stats.sprites_loaded, 0);
    }
}

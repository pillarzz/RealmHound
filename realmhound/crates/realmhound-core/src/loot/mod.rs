//! Loot tracking module.
//!
//! Tracks loot bag drops, attributes them to mobs, and stores historical data.

mod attribution;
mod bag_types;
mod database;
mod detection;
mod entity;
mod player_state;
mod tracker;

pub use attribution::{boss_ids, AttributionTrigger, LootAttributionManager, VariantSuffix};
pub use bag_types::{is_any_loot_bag, is_loot_drop_bag, loot_bag_name, LootBagType};
pub use database::{
    DungeonItemStat, LootDatabase, LootDropRecord, LootItemRecord, LootStatistics, MobItemStat,
    NewLootDrop, NewLootItem, SourceFilters, REQUIRED_TABLES, SUPPORTED_SCHEMA_VERSION,
};
pub use detection::{KilledEntity, LootDetector, PendingBag};
pub use entity::{LootEntity, TrackedPlayer};
pub use player_state::{
    class_ids, exalt_tiers, ClassExaltData, PlayerSnapshot, PlayerStateTracker,
};
pub use tracker::{LootTracker, ProcessedLootDrop, ProcessedLootItem, RecentBossKill};

/// Object type of Janus the Doorwarden (the Enemy the player fights).
pub const JANUS_OBJECT_TYPE: i32 = 8200;

/// Display name for Janus the Doorwarden, matching Combat History's boss name.
pub const JANUS_NAME: &str = "Janus the Doorwarden";

/// Item id of Mark of Janus, a guaranteed soulbound drop from Janus. Its
/// presence in a bag is a 100% reliable signal the bag came from Janus (whose
/// loot is emitted anonymously by an invisible generator, so proximity-based
/// attribution stores it as Unknown).
pub const MARK_OF_JANUS_ITEM_ID: i32 = 7731;

/// Object type of Bradley the Barkeep, The Tavern's final boss.
pub const TAVERN_BARKEEP_OBJECT_TYPE: i32 = 0x4552;

/// Display name for Bradley the Barkeep, matching Combat History's boss name.
pub const TAVERN_BARKEEP_NAME: &str = "Bradley the Barkeep";

/// Item id of Mark of the Barkeep, a guaranteed soulbound drop from Bradley the
/// Barkeep. He never drops loot directly (his bags are emitted anonymously and
/// arrive Unknown via proximity), so a bag holding the Mark is a 100% reliable
/// signal it came from him.
pub const MARK_OF_THE_BARKEEP_ITEM_ID: i32 = 0x44af;

// --- Killer Bee Nest realm event ---

/// Dungeon label used for realm fights/loot (the Godlands and its events).
pub const REALM_NAME: &str = "Realm";

/// True when a loot-stored dungeon string is the open realm (the Godlands),
/// where the Killer Bee Nest event spawns. The loot layer records the raw map
/// token (e.g. `{s.rotmg}`) rather than the combat-normalized `"Realm"`, and an
/// empty token is also the realm, so match both forms.
pub fn is_realm_dungeon(dungeon: &str) -> bool {
    let trimmed = dungeon.trim();
    trimmed.is_empty() || trimmed.starts_with('{') || trimmed == REALM_NAME
}

/// EH Event Taunt Controller: the invisible entity that emits every Killer Bee
/// Nest event loot bag (and its portals), so proximity attributes all event
/// bags to it. It is exclusive to this event (its objects are all prefixed
/// "EH ..."), so a bag tagged with it is always a Killer Bee Nest drop.
pub const EH_EVENT_TAUNT_CONTROLLER_OBJECT_TYPE: i32 = 2220;

/// EH Event Hive: the Killer Bee Nest miniboss the three Beehemoths guard.
pub const EH_EVENT_HIVE_OBJECT_TYPE: i32 = 4312;

/// Object type of the Blue Beehemoth (the Killer Bee Nest guardian bee).
pub const BLUE_BEEHEMOTH_OBJECT_TYPE: i32 = 4326;
/// Object type of the Red Beehemoth.
pub const RED_BEEHEMOTH_OBJECT_TYPE: i32 = 4325;
/// Object type of the Yellow Beehemoth.
pub const YELLOW_BEEHEMOTH_OBJECT_TYPE: i32 = 4324;

/// Object type of Corrupted Bramblethorn, a realm encounter that drops the same
/// (multi-colour) Beehemoth loot the Killer Bee Nest does.
pub const CORRUPTED_BRAMBLETHORN_OBJECT_TYPE: i32 = 51078;
/// Display name for Corrupted Bramblethorn, matching Combat History's name.
pub const CORRUPTED_BRAMBLETHORN_NAME: &str = "Corrupted Bramblethorn";

/// Object type of The Keyper, which also drops the multi-colour Beehemoth loot.
pub const KEYPER_OBJECT_TYPE: i32 = 19246;

/// A distinct Beehemoth colour, identified from the colour-specific loot items a
/// bag holds (Quiver / Quiver Shiny / Armor). The event drops soulbound loot in
/// a single colour matching the last Beehemoth the player damaged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BeehemothColor {
    /// Blue Beehemoth.
    Blue,
    /// Red Beehemoth.
    Red,
    /// Yellow Beehemoth.
    Yellow,
}

impl BeehemothColor {
    /// The Beehemoth object type for this colour.
    pub fn object_type(self) -> i32 {
        match self {
            Self::Blue => BLUE_BEEHEMOTH_OBJECT_TYPE,
            Self::Red => RED_BEEHEMOTH_OBJECT_TYPE,
            Self::Yellow => YELLOW_BEEHEMOTH_OBJECT_TYPE,
        }
    }

    /// The Beehemoth display name for this colour, matching Combat History.
    pub fn name(self) -> &'static str {
        match self {
            Self::Blue => "Blue Beehemoth",
            Self::Red => "Red Beehemoth",
            Self::Yellow => "Yellow Beehemoth",
        }
    }
}

/// The Beehemoth colour a Killer Bee Nest loot item belongs to, or `None` for
/// items that are not a colour-specific Beehemoth Quiver / Quiver Shiny / Armor.
/// The Green (Killer Bee Queen) variants are deliberately excluded: they belong
/// to the Queen fight, not this realm event.
fn beehemoth_color_of_item(item_id: i32) -> Option<BeehemothColor> {
    match item_id {
        4338 | 5345 | 4310 => Some(BeehemothColor::Blue), // quiver / shiny / armor
        4339 | 5344 | 4309 => Some(BeehemothColor::Red),
        4333 | 5343 | 4308 => Some(BeehemothColor::Yellow),
        _ => None,
    }
}

/// The distinct Beehemoth colours whose loot appears in `item_ids`, in a stable
/// order (Blue, Red, Yellow -- alphabetical). A single-colour bag is one
/// Beehemoth's soulbound drop; a multi-colour bag comes from Corrupted
/// Bramblethorn / The Keyper (both drop the same items across colours). Repeated
/// items of the same colour count once.
pub fn beehemoth_colors(item_ids: &[i32]) -> Vec<BeehemothColor> {
    let mut out = Vec::new();
    for c in [
        BeehemothColor::Blue,
        BeehemothColor::Red,
        BeehemothColor::Yellow,
    ] {
        if item_ids
            .iter()
            .any(|&id| beehemoth_color_of_item(id) == Some(c))
        {
            out.push(c);
        }
    }
    out
}

/// True when `object_type` positively marks a bag as a Killer Bee Nest event
/// drop before curated re-attribution: the invisible taunt controller or the
/// event hive. (Unknown / 0 is handled separately, since a generic Unknown Realm
/// bag is not necessarily a Beehemoth drop.)
pub fn is_killer_bee_nest_event_source(object_type: i32) -> bool {
    matches!(
        object_type,
        EH_EVENT_TAUNT_CONTROLLER_OBJECT_TYPE | EH_EVENT_HIVE_OBJECT_TYPE
    )
}

/// Dungeon name for Lair of Draconis, matching the stored dungeon label.
pub const LAIR_OF_DRACONIS_NAME: &str = "Lair of Draconis";

// Object types of the four Lair of Draconis elemental dragons. Each transforms
// on death (its loot is emitted anonymously), so proximity attribution stores
// their bags as Unknown. The final boss (Ivory Wyvern) attributes normally.
pub const LOD_FEARGUS_OBJECT_TYPE: i32 = 45428;
pub const LOD_NIKAO_OBJECT_TYPE: i32 = 45429;
pub const LOD_PYYR_OBJECT_TYPE: i32 = 45430;
pub const LOD_LIMOZ_OBJECT_TYPE: i32 = 45667;

fn lod_potion_dragon(item_id: i32) -> Option<(i32, &'static str)> {
    Some(match item_id {
        9064 => (LOD_FEARGUS_OBJECT_TYPE, "Feargus the Obsidian Dragon"),
        9068 => (LOD_NIKAO_OBJECT_TYPE, "Nikao the Azure Dragon"),
        9067 => (LOD_PYYR_OBJECT_TYPE, "Pyyr the Crimson Dragon"),
        9066 => (LOD_LIMOZ_OBJECT_TYPE, "Limoz the Viridian Dragon"),
        _ => return None,
    })
}

fn lod_utst_dragon(item_id: i32) -> Option<(i32, &'static str)> {
    Some(match item_id {
        3927 | 14353 | 14354 | 14355 | 14356 | 30053 => {
            (LOD_FEARGUS_OBJECT_TYPE, "Feargus the Obsidian Dragon")
        }
        14218 | 14219 | 14220 | 14221 | 30050 | 38546 => {
            (LOD_LIMOZ_OBJECT_TYPE, "Limoz the Viridian Dragon")
        }
        14866 | 14867 | 14868 | 14869 | 17846 | 30051 => {
            (LOD_NIKAO_OBJECT_TYPE, "Nikao the Azure Dragon")
        }
        3928 | 30052 | 32697 | 32698 | 32699 | 32700 => {
            (LOD_PYYR_OBJECT_TYPE, "Pyyr the Crimson Dragon")
        }
        _ => return None,
    })
}

/// Attribute a Lair of Draconis loot bag to a specific elemental dragon from
/// its item signature. Each elemental dragon guarantees a unique color greater
/// potion (the primary, near-universal signal); rarer per-dragon UT/ST drops
/// serve as a fallback. Bags with no dragon signal (public/player bags) or a
/// conflicting signal return None so they stay Unknown.
pub fn lair_of_draconis_dragon(item_ids: &[i32]) -> Option<(i32, &'static str)> {
    let mut dragon: Option<(i32, &'static str)> = None;
    for &id in item_ids {
        if let Some(d) = lod_potion_dragon(id) {
            match dragon {
                None => dragon = Some(d),
                Some(prev) if prev.0 != d.0 => return None,
                _ => {}
            }
        }
    }
    if dragon.is_some() {
        return dragon;
    }
    for &id in item_ids {
        if let Some(d) = lod_utst_dragon(id) {
            match dragon {
                None => dragon = Some(d),
                Some(prev) if prev.0 != d.0 => return None,
                _ => {}
            }
        }
    }
    dragon
}

/// Dungeon name for Spectral Penitentiary, matching the stored dungeon label.
pub const SPECTRAL_PENITENTIARY_NAME: &str = "Spectral Penitentiary";

/// Dungeon name for Moonlight Village, matching the stored dungeon label.
pub const MOONLIGHT_VILLAGE_NAME: &str = "Moonlight Village";

/// Dungeon name for Oryx's Sanctuary, matching the stored dungeon label.
pub const ORYX_SANCTUARY_NAME: &str = "Oryx's Sanctuary";

/// Object type of Chief Beisa, the Oryx's Sanctuary miniboss whose oversized
/// room means he routinely dies offscreen. With no killed entity near the bag
/// and (for older runs) no recorded fight, his high-tier bags arrive Unknown
/// while the other O3 minibosses -- who die onscreen -- are attributed at death.
pub const CHIEF_BEISA_OBJECT_TYPE: i32 = 45613;

/// Display name for Chief Beisa, matching Combat History's boss name.
pub const CHIEF_BEISA_NAME: &str = "Chief Beisa";

/// Greater Potion of Life / Mana item ids. Beisa reliably drops these, but they
/// are common enough that RealmEye omits them from his per-boss drop list, so we
/// whitelist them explicitly as part of his bag profile.
pub const GREATER_POTION_OF_LIFE_ITEM_ID: i32 = 9070;
pub const GREATER_POTION_OF_MANA_ITEM_ID: i32 = 9071;

/// True if an Unknown Oryx's Sanctuary bag fits Chief Beisa's drop profile. A
/// bag qualifies if it holds a Greater Potion of Life/Mana, is a red bag holding
/// a T13+ item, or holds any item on Beisa's Oryx's Sanctuary drop table. Brown
/// (public) bags are the caller's responsibility to exclude. This is the signal
/// that recovers Beisa's offscreen bags, which proximity attribution misses.
pub fn bag_matches_beisa_profile(bag_type: LootBagType, item_ids: &[i32]) -> bool {
    use crate::assets::DropLocationType;

    if item_ids
        .iter()
        .any(|&i| i == GREATER_POTION_OF_LIFE_ITEM_ID || i == GREATER_POTION_OF_MANA_ITEM_ID)
    {
        return true;
    }

    let assets = crate::assets::get_asset_manager();
    if matches!(bag_type, LootBagType::Red | LootBagType::BoostedRed)
        && item_ids.iter().any(|&i| {
            assets
                .get_object(i)
                .and_then(|o| o.get_tier())
                .is_some_and(|t| t >= 13)
        })
    {
        return true;
    }

    let drops = crate::assets::get_realmeye_drops();
    for &item in item_ids {
        for src in drops.sources_for_item(item) {
            if src.location_type == DropLocationType::Dungeon
                && src.location_name.eq_ignore_ascii_case(ORYX_SANCTUARY_NAME)
                && src.source_name.eq_ignore_ascii_case(CHIEF_BEISA_NAME)
            {
                return true;
            }
        }
    }

    false
}

/// True if Chief Beisa was recorded fighting in the given instance (`map_seed`),
/// the live-attribution engagement gate for his offscreen bags.
pub fn beisa_engaged(recent_boss_kills: &[RecentBossKill], map_seed: i32) -> bool {
    map_seed != 0
        && recent_boss_kills
            .iter()
            .any(|k| k.object_type == CHIEF_BEISA_OBJECT_TYPE && k.map_seed == map_seed)
}

// Object types of the Spectral Penitentiary bosses.
pub const SPEN_MURCIAN_OBJECT_TYPE: i32 = 23681;
pub const SPEN_LOBOTOMIK_OBJECT_TYPE: i32 = 23920;
pub const SPEN_ZOLE_OBJECT_TYPE: i32 = 23659;
pub const SPEN_GRETCH_OBJECT_TYPE: i32 = 23819;
pub const SPEN_OCULON_OBJECT_TYPE: i32 = 24071;

/// Item id of Mark of the Soulwarden, a guaranteed, exclusive drop from the
/// final boss Soulwarden Murcian. Its presence marks a bag as Murcian's; its
/// absence from a bag holding a dungeon white means that white came from the
/// miniboss that drops it, not from Murcian.
pub const MARK_OF_THE_SOULWARDEN_ITEM_ID: i32 = 23968;

/// Mana potion item ids (normal and greater). Spectral Penitentiary minibosses
/// drop a mana potion alongside their stat potion, so a stat potion paired with
/// a mana potion is a miniboss drop; a lone stat potion is a clearing-phase
/// minion drop (minions also drop stat potions) and must not be attributed.
const SPEN_MANA_POTION_IDS: [i32; 2] = [2794, 9071];

/// Schematic item id. Every Spectral Penitentiary miniboss drops a Schematic, so
/// it confirms a bag came from a miniboss (never a minion or player bag) without
/// identifying which one. Vial of Soul Extract is deliberately NOT used here: it
/// also drops from ordinary dungeon minions.
const SPEN_SCHEMATIC_ITEM_ID: i32 = 34634;

/// Helmet Rune item id. Runes drop only from bosses, and within Spectral
/// Penitentiary only the Admin/Garden pair (Oculon, Gretch) drop this one, so it
/// both confirms a miniboss and narrows identity to that pair.
const SPEN_HELMET_RUNE_ITEM_ID: i32 = 10024;

/// Set-shard id ranges (quantities x1..x15). The two Spectral Penitentiary sets
/// each drop from a fixed pair of minibosses, so a shard confirms a miniboss and
/// narrows identity to that branch pair:
/// Plague Doctor Priest -> Lobotomik or Zole; Alchemist Assassin -> Gretch or
/// Oculon.
const SPEN_PLAGUE_DOCTOR_SHARD_IDS: std::ops::RangeInclusive<i32> = 53297..=53311;
const SPEN_ALCHEMIST_ASSASSIN_SHARD_IDS: std::ops::RangeInclusive<i32> = 53361..=53375;

/// The Alchemist Assassin branch pair (Oculon, Gretch), listed Oculon-first: he
/// is the miniboss most prone to being recorded as Unknown, so he is the default
/// pick when a bag can only be narrowed to this pair.
const SPEN_ALCHEMIST_PAIR: [(i32, &str); 2] = [
    (SPEN_OCULON_OBJECT_TYPE, "Overseer Oculon"),
    (SPEN_GRETCH_OBJECT_TYPE, "Groundskeeper Gretch"),
];

/// The Plague Doctor Priest branch pair (Lobotomik, Zole). Neither is Oculon, so
/// a bag narrowed only to this pair with no other signal stays Unknown.
const SPEN_PLAGUE_PAIR: [(i32, &str); 2] = [
    (SPEN_LOBOTOMIK_OBJECT_TYPE, "Doctor Lobotomik"),
    (SPEN_ZOLE_OBJECT_TYPE, "Griefkeeper Zole"),
];

/// True if the bag holds an item only a Spectral Penitentiary miniboss can drop:
/// a Schematic, a set shard, a Helmet Rune, or any Tier 13+ weapon or armor.
/// Minions and player-dropped public bags never contain these, so this confirms
/// the bag is a genuine miniboss drop even when no potion identifies which one.
pub fn spectral_penitentiary_boss_confirmed(item_ids: &[i32]) -> bool {
    if item_ids.iter().any(|&id| {
        id == SPEN_SCHEMATIC_ITEM_ID
            || id == SPEN_HELMET_RUNE_ITEM_ID
            || SPEN_PLAGUE_DOCTOR_SHARD_IDS.contains(&id)
            || SPEN_ALCHEMIST_ASSASSIN_SHARD_IDS.contains(&id)
    }) {
        return true;
    }
    let assets = crate::assets::get_asset_manager();
    item_ids.iter().any(|&id| {
        assets
            .get_object(id)
            .and_then(|o| o.get_tier())
            .map(|t| t >= 13)
            .unwrap_or(false)
    })
}

/// If the bag holds a set shard or Helmet Rune, return the branch pair of
/// minibosses it could belong to (Oculon-first for the Alchemist pair). Returns
/// None when the bag carries no pair-narrowing signal.
pub fn spectral_penitentiary_shard_pair(
    item_ids: &[i32],
) -> Option<&'static [(i32, &'static str)]> {
    if item_ids.iter().any(|&id| {
        id == SPEN_HELMET_RUNE_ITEM_ID || SPEN_ALCHEMIST_ASSASSIN_SHARD_IDS.contains(&id)
    }) {
        return Some(&SPEN_ALCHEMIST_PAIR);
    }
    if item_ids
        .iter()
        .any(|&id| SPEN_PLAGUE_DOCTOR_SHARD_IDS.contains(&id))
    {
        return Some(&SPEN_PLAGUE_PAIR);
    }
    None
}

/// Murcian, the Spectral Penitentiary final boss, always drops the Mark of the
/// Soulwarden and nothing else drops it. A bag holding the Mark is therefore a
/// Murcian drop with certainty; a bag without it is never Murcian.
pub fn spectral_penitentiary_murcian(item_ids: &[i32]) -> Option<(i32, &'static str)> {
    if item_ids.contains(&MARK_OF_THE_SOULWARDEN_ITEM_ID) {
        Some((SPEN_MURCIAN_OBJECT_TYPE, "Soulwarden Murcian"))
    } else {
        None
    }
}

/// Reliable Spectral Penitentiary attribution from guaranteed boss-only items:
/// the Mark of the Soulwarden (Murcian) and per-miniboss whites, shinies and
/// forge blueprints (minions never drop these). Returns None if the signal is
/// absent or conflicting.
pub fn spectral_penitentiary_reliable_boss(item_ids: &[i32]) -> Option<(i32, &'static str)> {
    if let Some(b) = spectral_penitentiary_murcian(item_ids) {
        return Some(b);
    }
    let mut boss: Option<(i32, &'static str)> = None;
    for &id in item_ids {
        let m = match id {
            // Sinister Syringes, Cackling Straitjacket (+shiny, +blueprints)
            7475 | 23745 | 23919 | 44412 | 44413 => {
                (SPEN_LOBOTOMIK_OBJECT_TYPE, "Doctor Lobotomik")
            }
            // Overwhelming Axehead, Motivational Megaphone (+shiny, +blueprints)
            23915 | 23916 | 41482 | 44410 | 44411 => (SPEN_ZOLE_OBJECT_TYPE, "Griefkeeper Zole"),
            // Tools of the Tarnished, Wretched Rags (+shiny, +blueprints)
            7295 | 7408 | 23898 | 24414 | 44408 | 44409 => {
                (SPEN_GRETCH_OBJECT_TYPE, "Groundskeeper Gretch")
            }
            // Command Cornea, Ocular Entrapment (+shiny, +blueprints)
            3989 | 23512 | 23742 | 44414 | 44415 => (SPEN_OCULON_OBJECT_TYPE, "Overseer Oculon"),
            _ => continue,
        };
        match boss {
            None => boss = Some(m),
            Some(prev) if prev.0 != m.0 => return None,
            _ => {}
        }
    }
    boss
}

/// Spectral Penitentiary miniboss identified by its stat potion (Wisdom ->
/// Oculon, Attack -> Zole, Speed -> Gretch, Vitality -> Lobotomik). Minions also
/// drop stat potions, so a stat potion only counts when the bag also holds a
/// mana potion (minibosses drop the pair) or `boss_confirmed` is set (a boss-only
/// item rules out a minion). Returns None if absent, unconfirmed, or conflicting.
pub fn spectral_penitentiary_potion_miniboss(
    item_ids: &[i32],
    boss_confirmed: bool,
) -> Option<(i32, &'static str)> {
    let mana_paired = item_ids.iter().any(|i| SPEN_MANA_POTION_IDS.contains(i));
    if !mana_paired && !boss_confirmed {
        return None;
    }
    let mut boss: Option<(i32, &'static str)> = None;
    for &id in item_ids {
        let m = match id {
            2612 | 9067 => (SPEN_LOBOTOMIK_OBJECT_TYPE, "Doctor Lobotomik"),
            2591 | 9064 => (SPEN_ZOLE_OBJECT_TYPE, "Griefkeeper Zole"),
            2593 | 9066 => (SPEN_GRETCH_OBJECT_TYPE, "Groundskeeper Gretch"),
            2613 | 9068 => (SPEN_OCULON_OBJECT_TYPE, "Overseer Oculon"),
            _ => continue,
        };
        match boss {
            None => boss = Some(m),
            Some(prev) if prev.0 != m.0 => return None,
            _ => {}
        }
    }
    boss
}

/// True if `object_type` is one of the four Spectral Penitentiary minibosses
/// (a run has at most two of them, plus the final boss Murcian).
pub fn is_spectral_penitentiary_miniboss(object_type: i32) -> bool {
    matches!(
        object_type,
        SPEN_LOBOTOMIK_OBJECT_TYPE
            | SPEN_ZOLE_OBJECT_TYPE
            | SPEN_GRETCH_OBJECT_TYPE
            | SPEN_OCULON_OBJECT_TYPE
    )
}

/// Attribute a Spectral Penitentiary loot bag to a boss from its item signature
/// for going-forward ingestion: a reliable white/mark, then a stat potion
/// (accepted when mana-paired or the bag holds boss-only loot). Returns None when
/// no confident identity is present; the caller falls back to fight correlation
/// and, as a last resort, an Oculon default (see the tracker). Returns None when
/// unattributable.
pub fn spectral_penitentiary_boss(item_ids: &[i32]) -> Option<(i32, &'static str)> {
    if let Some(b) = spectral_penitentiary_reliable_boss(item_ids) {
        return Some(b);
    }
    let confirmed = spectral_penitentiary_boss_confirmed(item_ids);
    spectral_penitentiary_potion_miniboss(item_ids, confirmed)
}

/// Last-resort Spectral Penitentiary attribution for a bag confirmed to be a
/// miniboss drop (boss-only loot) but carrying no identifying white or potion.
/// Defaults to Oculon, the miniboss overwhelmingly most likely to have gone
/// unrecorded, unless a set shard narrows the bag to the Plague Doctor pair
/// (Lobotomik/Zole), which excludes Oculon and stays Unknown.
pub fn spectral_penitentiary_boss_confirmed_default(
    item_ids: &[i32],
) -> Option<(i32, &'static str)> {
    if !spectral_penitentiary_boss_confirmed(item_ids) {
        return None;
    }
    match spectral_penitentiary_shard_pair(item_ids) {
        Some(pair) if !pair.iter().any(|(t, _)| *t == SPEN_OCULON_OBJECT_TYPE) => None,
        _ => Some((SPEN_OCULON_OBJECT_TYPE, "Overseer Oculon")),
    }
}

/// Bosses that could have dropped this bag, derived from the RealmEye drop
/// tables scoped to `dungeon`. Each item's known drop source is resolved to a
/// boss object type (minion, add and non-boss sources are excluded), then
/// deduplicated. An empty result is the "not boss-only loot" signal: the bag
/// holds nothing a boss is known to drop in this dungeon, so it must not be
/// attributed to a boss. This is the general, drop-table-driven complement to
/// the curated per-dungeon signals above.
pub fn boss_drop_candidates(dungeon: &str, item_ids: &[i32]) -> Vec<(i32, String)> {
    use crate::assets::DropLocationType;

    let drops = crate::assets::get_realmeye_drops();
    let assets = crate::assets::get_asset_manager();
    let mut out: Vec<(i32, String)> = Vec::new();
    for &item in item_ids {
        for src in drops.sources_for_item(item) {
            if src.location_type != DropLocationType::Dungeon
                || !src.location_name.eq_ignore_ascii_case(dungeon)
            {
                continue;
            }
            let Some(obj) = assets.object_id_for_display_name(&src.source_name) else {
                continue;
            };
            if !(assets.is_boss_like(obj) || assets.is_curated_boss(obj)) {
                continue;
            }
            if out.iter().any(|(t, _)| *t == obj) {
                continue;
            }
            let name = assets
                .object_name(obj)
                .unwrap_or_else(|| src.source_name.clone());
            out.push((obj, name));
        }
    }
    out
}

/// The single boss every boss-only drop in this bag points to, if the drop
/// tables make the attribution unambiguous. Returns None when the bag has no
/// boss-only drop or its drops could have come from more than one boss.
pub fn unique_boss_drop(dungeon: &str, item_ids: &[i32]) -> Option<(i32, String)> {
    let mut candidates = boss_drop_candidates(dungeon, item_ids);
    if candidates.len() == 1 {
        candidates.pop()
    } else {
        None
    }
}

/// True when the bag holds a boss completion Mark (a guaranteed, exclusive drop
/// of a dungeon's main boss). A Mark is a definitive signal that the bag is that
/// dungeon's main-boss drop, even if proximity left it Unknown or misattributed
/// it to a nearby minion. The specific boss is resolved from the instance's
/// recorded kill (live) or per-dungeon consensus (legacy migration).
pub fn bag_has_boss_mark(item_ids: &[i32]) -> bool {
    let marks = crate::assets::get_asset_manager().boss_mark_item_ids();
    !marks.is_empty() && item_ids.iter().any(|id| marks.contains(id))
}

/// The single realm-event source every rare item in this bag points to, if that
/// attribution is unambiguous. Used to reattribute Unknown open-realm bags
/// (e.g. Ethereal Shrine event loot) that proximity left on the invisible
/// dropper or `?`. Only realm **biome** sources are considered (dungeon sources
/// belong to dungeon instances, handled elsewhere), and **seasonal** sources
/// (limited-time reskin events) are excluded so a rare that normally drops from
/// one event but also from a seasonal reskin still resolves to its regular
/// source. Returns the source only when, after excluding seasonal ones, exactly
/// one distinct source name remains across all the bag's items AND it resolves
/// to a valid drop source; otherwise `None` (leave the bag Unknown).
pub fn resolve_realm_event_source(item_ids: &[i32]) -> Option<(i32, String)> {
    use crate::assets::DropLocationType;

    let drops = crate::assets::get_realmeye_drops();
    let assets = crate::assets::get_asset_manager();

    let mut source_names: Vec<String> = Vec::new();
    for &item in item_ids {
        for src in drops.sources_for_item(item) {
            if src.location_type != DropLocationType::Biome || src.seasonal {
                continue;
            }
            if !source_names.iter().any(|n| n == &src.source_name) {
                source_names.push(src.source_name.clone());
            }
        }
    }

    let [only] = source_names.as_slice() else {
        return None;
    };
    let obj = assets.object_id_for_display_name(only)?;
    if !assets.is_valid_drop_source(obj) {
        return None;
    }
    let name = assets.object_name(obj).unwrap_or_else(|| only.clone());
    Some((obj, name))
}

/// The generic drop-table path must not touch these: RealmEye's coarser "drops
/// of interest" data would otherwise override those precise decisions (e.g.
/// attributing a minion-dropped Vial of Soul Extract to the dungeon's boss).
pub fn has_curated_boss_attribution(dungeon: &str) -> bool {
    dungeon == LAIR_OF_DRACONIS_NAME
        || dungeon == SPECTRAL_PENITENTIARY_NAME
        || dungeon == MOONLIGHT_VILLAGE_NAME
        || dungeon == TOMB_OF_THE_ANCIENTS_NAME
        || dungeon == ICE_TOMB_NAME
}

/// Dungeon name for Tomb of the Ancients, matching the stored dungeon label.
pub const TOMB_OF_THE_ANCIENTS_NAME: &str = "Tomb of the Ancients";

// Object types of the Tomb of the Ancients bosses. The three main bosses
// (Bes/Geb/Nut) sit in separate rooms and frequently die offscreen, so their
// bags arrive Unknown; the Sarcophagus miniboss opens the boss gate.
pub const TOMB_NUT_OBJECT_TYPE: i32 = 3366;
pub const TOMB_GEB_OBJECT_TYPE: i32 = 3367;
pub const TOMB_BES_OBJECT_TYPE: i32 = 3368;
pub const TOMB_ACTIVE_SARCOPHAGUS_OBJECT_TYPE: i32 = 3369;
pub const TOMB_TREASURE_SARCOPHAGUS_OBJECT_TYPE: i32 = 3372;

/// The three Tomb main bosses in alphabetical order (Bes, Geb, Nut). Used as the
/// last-resort ordering when several bags drop offscreen with no identifying
/// loot: bags are handed out to the already-dead bosses alphabetically.
pub const TOMB_MAIN_BOSSES: [(i32, &str); 3] = [
    (TOMB_BES_OBJECT_TYPE, "Bes"),
    (TOMB_GEB_OBJECT_TYPE, "Geb"),
    (TOMB_NUT_OBJECT_TYPE, "Nut"),
];

/// True if `object_type` is one of the three Tomb main bosses (Bes/Geb/Nut).
pub fn is_tomb_main_boss(object_type: i32) -> bool {
    matches!(
        object_type,
        TOMB_NUT_OBJECT_TYPE | TOMB_GEB_OBJECT_TYPE | TOMB_BES_OBJECT_TYPE
    )
}

/// Map a Tomb boss rage-phase taunt to its boss object type. Each of the three
/// main bosses emits a unique, one-time message shortly before dying, so the
/// order of taunts approximates the order of deaths -- a far stronger offscreen
/// attribution signal than the alphabetical fallback. Matched on text alone (the
/// phrases are unique), independent of the chat author name.
pub fn tomb_rage_taunt_boss(text: &str) -> Option<i32> {
    match text.trim() {
        "The end of your path is here!" => Some(TOMB_BES_OBJECT_TYPE),
        "This cannot be! You shall not succeed!" => Some(TOMB_NUT_OBJECT_TYPE),
        "Argh! You shall pay for your crimes!" => Some(TOMB_GEB_OBJECT_TYPE),
        _ => None,
    }
}

/// Enemies that can drop a Potion of Life in the Tomb of the Ancients: the three
/// main bosses and the Sarcophagus miniboss (its Active and Treasure forms).
/// Artifacts and other minions never drop Life, so a Life bag proximity-
/// attributed to anything else was mis-attributed and must be re-resolved.
pub fn is_tomb_life_dropper(object_type: i32) -> bool {
    is_tomb_main_boss(object_type)
        || matches!(
            object_type,
            TOMB_ACTIVE_SARCOPHAGUS_OBJECT_TYPE | TOMB_TREASURE_SARCOPHAGUS_OBJECT_TYPE
        )
}

/// True if the item belongs to a Tomb "modifier spawner" category -- loot no
/// boss drops (pet food/cookies, pet eggs, pet/character skins, shards and skin
/// tokens, effusions/nildrops/ichors, and mystery-stat crates). Certain dungeon
/// modifiers spawn these from an invisible entity (like the Mark of Geb), so a
/// bag holding only such items is not one of a boss's own loot bags.
fn is_tomb_spawner_item(item_id: i32) -> bool {
    use crate::assets::item_category::{ItemCategorizer, ItemSubCategory};
    if item_id <= 0 {
        return false;
    }
    match crate::assets::get_asset_manager().get_object(item_id) {
        Some(asset) => matches!(
            ItemCategorizer::categorize(&asset).1,
            ItemSubCategory::PetFood
                | ItemSubCategory::PetEggs
                | ItemSubCategory::PetSkins
                | ItemSubCategory::Skins
                | ItemSubCategory::EventTokens
                | ItemSubCategory::HelpfulConsumables
                | ItemSubCategory::Crates
        ),
        None => false,
    }
}

/// True if a Tomb bag holds only "modifier spawner" loot (see
/// [`is_tomb_spawner_item`]) and nothing a boss actually drops. Such a bag comes
/// from an invisible spawner, not a boss's own loot, so -- like the Mark of Geb
/// -- it is credited to Geb (the conventional last main boss) without consuming
/// a boss's one-bag slot, leaving the real boss bags to be attributed correctly.
pub fn is_tomb_spawner_only_bag(item_ids: &[i32]) -> bool {
    !item_ids.is_empty() && item_ids.iter().all(|&id| is_tomb_spawner_item(id))
}

/// Item id of Potion of Life. Tomb of the Ancients drops the standard Potion of
/// Life (never the Greater/soulbound variants), so a single id suffices. Used to
/// detect Life bags for the "Life never drops from artifacts" rule.
pub const POTION_OF_LIFE_ITEM_ID: i32 = 2793;

/// Item id of Mark of Geb. Geb's Mark is emitted by an invisible spawner
/// (proximity leaves it Unknown), so a bag holding it is a certain Geb drop.
/// This must be the item id, not an object lookup: `object_id_for_display_name`
/// resolves the invisible *spawner* object (a Character sharing the name), not
/// the item, which silently broke the rule.
pub const MARK_OF_GEB_ITEM_ID: i32 = 7729;

/// Attribute a Tomb bag to a single boss from its unique drop signature using
/// the dungeon's RealmEye drop tables (each main boss's whites and UT shinies).
/// Returns None when the bag holds no boss-only Tomb loot or its loot could have
/// come from more than one boss.
pub fn tomb_unique_boss(item_ids: &[i32]) -> Option<(i32, String)> {
    unique_boss_drop(TOMB_OF_THE_ANCIENTS_NAME, item_ids)
}

// ---------------------------------------------------------------------------
// Ice Tomb
// ---------------------------------------------------------------------------

/// Dungeon name for Ice Tomb, matching the stored dungeon label.
pub const ICE_TOMB_NAME: &str = "Ice Tomb";

// Ice Tomb boss "soul" object types. On death each of the three bosses
// (Frimar/Polaris/Glacius) spawns a soul that lingers until all three bosses
// are dead, then every soul dies at once and drops that boss's loot. The souls
// -- not the bosses -- are the actual loot droppers, but the player never hits
// them, so they must be whitelisted as droppers for proximity attribution.
pub const ICE_TOMB_DEFENDER_SOUL_OBJECT_TYPE: i32 = 45638; // Frimar (Bes reskin)
pub const ICE_TOMB_SUPPORT_SOUL_OBJECT_TYPE: i32 = 45639; // Polaris (Nut reskin)
pub const ICE_TOMB_ATTACKER_SOUL_OBJECT_TYPE: i32 = 45640; // Glacius (Geb reskin)

// Ice Tomb boss object types (from iceTomb.xml). Stored as each bag's `mob_type`
// so the loot history renders the real boss sprite; the soul ids above are only
// used for live proximity detection.
pub const ICE_TOMB_FRIMAR_OBJECT_TYPE: i32 = 32694; // 0x7fb6 "Ice Tomb Defender"
pub const ICE_TOMB_POLARIS_OBJECT_TYPE: i32 = 32692; // 0x7fb4 "Ice Tomb Support"
pub const ICE_TOMB_GLACIUS_OBJECT_TYPE: i32 = 32693; // 0x7fb5 "Ice Tomb Attacker"

/// Map an Ice Tomb boss name to its boss object type -- the stable, nonzero id
/// stored as `mob_type` so the correct boss sprite is drawn.
pub fn ice_tomb_object_type_for_boss(boss: &str) -> i32 {
    match boss {
        "Frimar" => ICE_TOMB_FRIMAR_OBJECT_TYPE,
        "Polaris" => ICE_TOMB_POLARIS_OBJECT_TYPE,
        "Glacius" => ICE_TOMB_GLACIUS_OBJECT_TYPE,
        _ => 0,
    }
}

/// Map an Ice Tomb boss object type back to its boss name, so a stored bag from
/// a prior (fixed) run is recognised as already credited.
pub fn ice_tomb_boss_name_for_object(object_type: i32) -> Option<&'static str> {
    match object_type {
        ICE_TOMB_FRIMAR_OBJECT_TYPE => Some("Frimar"),
        ICE_TOMB_POLARIS_OBJECT_TYPE => Some("Polaris"),
        ICE_TOMB_GLACIUS_OBJECT_TYPE => Some("Glacius"),
        _ => None,
    }
}

/// Map an Ice Tomb boss to its soul object type. Used to remap legacy rows that
/// stored a soul id as `mob_type` onto the real boss id.
pub fn ice_tomb_soul_for_boss(boss: &str) -> i32 {
    match boss {
        "Frimar" => ICE_TOMB_DEFENDER_SOUL_OBJECT_TYPE,
        "Polaris" => ICE_TOMB_SUPPORT_SOUL_OBJECT_TYPE,
        "Glacius" => ICE_TOMB_ATTACKER_SOUL_OBJECT_TYPE,
        _ => 0,
    }
}

/// The three Ice Tomb bosses in alphabetical order, used as the last-resort
/// ordering when several soul bags are ambiguous: bags are handed to the
/// already-dead bosses in this order (refined by rage-taunt order when known).
pub const ICE_TOMB_BOSSES: [&str; 3] = ["Frimar", "Glacius", "Polaris"];

/// True if `object_type` is one of the three Ice Tomb boss souls.
pub fn is_ice_tomb_soul(object_type: i32) -> bool {
    matches!(
        object_type,
        ICE_TOMB_DEFENDER_SOUL_OBJECT_TYPE
            | ICE_TOMB_SUPPORT_SOUL_OBJECT_TYPE
            | ICE_TOMB_ATTACKER_SOUL_OBJECT_TYPE
    )
}

/// Object types that drop loot without ever being hit by the player, so the
/// detector must treat them as valid droppers regardless of the hit list. Ice
/// Tomb souls are the only such entities today.
pub fn is_forced_loot_dropper(object_type: i32) -> bool {
    is_ice_tomb_soul(object_type)
}

/// Map an Ice Tomb soul object type to the boss it belongs to.
pub fn ice_tomb_boss_name_for_soul(object_type: i32) -> Option<&'static str> {
    match object_type {
        ICE_TOMB_DEFENDER_SOUL_OBJECT_TYPE => Some("Frimar"),
        ICE_TOMB_SUPPORT_SOUL_OBJECT_TYPE => Some("Polaris"),
        ICE_TOMB_ATTACKER_SOUL_OBJECT_TYPE => Some("Glacius"),
        _ => None,
    }
}

/// Map an Ice Tomb boss rage-phase taunt to its boss. Each boss emits a unique,
/// one-time line shortly before dying, so the order of taunts approximates the
/// order of deaths -- refining the alphabetical fallback for ambiguous soul
/// bags. Matched on distinctive fragments (case-insensitive) to stay robust to
/// the game's fancy punctuation.
pub fn ice_tomb_rage_taunt_boss(text: &str) -> Option<&'static str> {
    let t = text.to_lowercase();
    if t.contains("encase") {
        Some("Frimar")
    } else if t.contains("meltdown") {
        Some("Polaris")
    } else if t.contains("failing") && t.contains("sorry") {
        Some("Glacius")
    } else {
        None
    }
}

/// Item ids that only a single Ice Tomb boss drops (each boss's unique ring).
/// Shared drops (potions, Freezing Quiver, Arctic Bow, etc.) are excluded, so a
/// bag holding one of these names its boss outright.
const ICE_TOMB_UNIQUE_DROPS: [(i32, &str); 3] = [
    (32724, "Frimar"),  // Frimarra
    (32722, "Polaris"), // Enchanted Ice Shard
    (32723, "Glacius"), // Ring of the Northern Light
];

/// Attribute an Ice Tomb bag to a single boss from its unique drop signature.
/// Returns None when the bag holds no boss-only Ice Tomb loot or its loot could
/// have come from more than one boss.
pub fn ice_tomb_unique_boss(item_ids: &[i32]) -> Option<&'static str> {
    let mut found: Option<&'static str> = None;
    for &id in item_ids {
        if let Some((_, boss)) = ICE_TOMB_UNIQUE_DROPS.iter().find(|(i, _)| *i == id) {
            match found {
                Some(prev) if prev != *boss => return None,
                _ => found = Some(*boss),
            }
        }
    }
    found
}

/// True if the bag holds any loot the Ice Tomb bosses are known to drop (per the
/// embedded RealmEye drop tables). Ice Tomb's drop table lists only its three
/// bosses' loot, so an item found there marks the bag as a soul (boss) bag --
/// used to gate ambiguous-bag distribution so stray non-boss bags stay Unknown.
pub fn ice_tomb_has_boss_loot(item_ids: &[i32]) -> bool {
    let drops = crate::assets::get_realmeye_drops();
    item_ids.iter().any(|&id| {
        drops
            .dungeons_for_item(id)
            .iter()
            .any(|d| d.eq_ignore_ascii_case(ICE_TOMB_NAME))
    })
}

#[cfg(test)]
mod beisa_tests {
    use super::*;

    #[test]
    fn greater_potions_match_beisa_profile() {
        assert!(bag_matches_beisa_profile(
            LootBagType::Blue,
            &[GREATER_POTION_OF_LIFE_ITEM_ID]
        ));
        assert!(bag_matches_beisa_profile(
            LootBagType::Purple,
            &[GREATER_POTION_OF_MANA_ITEM_ID]
        ));
    }

    #[test]
    fn beisa_drop_table_item_matches_profile() {
        // 5146 is on Chief Beisa's Oryx's Sanctuary drop table (RealmEye data).
        assert!(bag_matches_beisa_profile(LootBagType::Red, &[5146]));
    }

    #[test]
    fn unrelated_item_does_not_match_profile() {
        // Demon Blade (17840) drops in Abyss of Demons, never from Beisa; with no
        // potion and (in tests) no loaded tier data it must not match.
        assert!(!bag_matches_beisa_profile(LootBagType::Red, &[17840]));
    }

    #[test]
    fn engagement_gate_requires_matching_seed() {
        let kills = vec![RecentBossKill {
            map_seed: 42,
            object_type: CHIEF_BEISA_OBJECT_TYPE,
            name: CHIEF_BEISA_NAME.to_string(),
            started_at_ms: 0,
            ended_at_ms: 0,
        }];
        assert!(beisa_engaged(&kills, 42));
        assert!(!beisa_engaged(&kills, 99));
        assert!(!beisa_engaged(&kills, 0));
        assert!(!beisa_engaged(&[], 42));
    }
}

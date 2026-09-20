//! Dead-fame (fame-bonus) estimator based on RotMG's in-game Fame Overview.
//!
//! Estimates the total fame a *living* character would receive if it died now:
//! `total = base_fame + sum(flat bonuses) + sum(ceil(base_fame * percent))`.
//!
//! Each percentage bonus is rounded up (ceiling) and applied on its own line,
//! matching the in-game Fame Overview. Summing every percentage first and
//! rounding once undercounts by a few fame, so the per-line ceiling is used.
//!
//! Bonus tables come from <https://www.realmeye.com/wiki/fame-bonuses>. Tiered
//! categories are cumulative and their highest tier repeats: for a count `c`
//! with thresholds `t[0..5]` and bonuses `b[0..5]`, the character earns `b[i]`
//! once for each `i < 4` reached, plus `floor(c / t[4]) * b[4]` for the top tier.
//!
//! The result is an estimate: it excludes the account-level Ancestor bonus and
//! depends on the freshness of the cached base fame and PCStats.

use super::collections::DungeonCollection;
use crate::api::{BiomeStats, CharacterStats, DungeonStats};

/// Percent bonuses are tracked in tenths of a percent to keep integer math
/// (e.g. 2.5% -> 25, 7.5% -> 75). Applied as `base * tenths / 1000`.
type PercentTenths = i64;

/// Fame from a single percentage bonus, rounded up (ceiling) as the game does.
///
/// The in-game Fame Overview lists each percentage bonus separately and rounds
/// each one up, so e.g. `+7.5%` of 180,723 fame contributes
/// `ceil(13,554.225) = 13,555`, not `13,554`.
fn ceil_percent_bonus(base_fame: i64, percent_tenths: PercentTenths) -> i64 {
    let product = base_fame.saturating_mul(percent_tenths);
    if product > 0 {
        product.saturating_add(999) / 1000
    } else {
        // No negative percentage bonuses exist in practice; truncate toward zero.
        product / 1000
    }
}

/// A single contributing line of the fame-bonus breakdown.
#[derive(Debug, Clone)]
pub struct FameBonusLine {
    pub name: String,
    pub flat: i64,
    pub percent_tenths: PercentTenths,
}

/// Full fame-bonus breakdown for a character.
#[derive(Debug, Clone)]
pub struct FameBonusBreakdown {
    pub base_fame: i64,
    pub lines: Vec<FameBonusLine>,
}

impl FameBonusBreakdown {
    pub fn flat_total(&self) -> i64 {
        self.lines.iter().map(|l| l.flat).sum()
    }

    pub fn percent_tenths_total(&self) -> PercentTenths {
        self.lines.iter().map(|l| l.percent_tenths).sum()
    }

    /// Bonus fame added on death (flat + percent-of-base), excluding base fame.
    ///
    /// Each line's percentage is rounded up independently, matching the game.
    pub fn bonus_fame(&self) -> i64 {
        self.lines
            .iter()
            .map(|l| l.flat + ceil_percent_bonus(self.base_fame, l.percent_tenths))
            .sum()
    }

    /// Estimated total fame received on death.
    pub fn total_dead_fame(&self) -> i64 {
        self.base_fame + self.bonus_fame()
    }
}

/// Cumulative tiered bonus with a repeating top tier.
///
/// The repeating top tier is capped at [`MAX_TOP_TIER_REPS`] repetitions, which
/// matches the game (it stops awarding the top-tier bonus after 20 repeats).
fn tiered(count: i64, thresholds: [i64; 5], bonuses: [i64; 5]) -> i64 {
    if count <= 0 {
        return 0;
    }
    let mut total = 0;
    for i in 0..4 {
        if count >= thresholds[i] {
            total += bonuses[i];
        }
    }
    if thresholds[4] > 0 && count >= thresholds[4] {
        let reps = (count / thresholds[4]).min(MAX_TOP_TIER_REPS);
        total += reps * bonuses[4];
    }
    total
}

/// The game caps the repeating top tier at 20 repetitions for every fame-bonus
/// category, so `tiered` never awards more than `20 * bonuses[4]` for it.
const MAX_TOP_TIER_REPS: i64 = 20;

// --- Achievement threshold tables (thresholds, bonuses) --------------------

const CARTOGRAPHY: ([i64; 5], [i64; 5]) = (
    [200_000, 1_000_000, 4_000_000, 10_000_000, 25_000_000],
    [20, 100, 400, 1_000, 2_500],
);
const QUESTS: ([i64; 5], [i64; 5]) = ([25, 100, 400, 1_000, 2_000], [20, 200, 800, 2_000, 5_000]);
const COMBAT: ([i64; 5], [i64; 5]) = (
    [8_000, 40_000, 200_000, 800_000, 2_000_000],
    [20, 100, 400, 1_000, 2_500],
);
const PARTY: ([i64; 5], [i64; 5]) = ([25, 50, 100, 400, 1_000], [20, 50, 200, 400, 1_000]);
const POTIONS: ([i64; 5], [i64; 5]) = ([25, 100, 400, 1_000, 2_000], [20, 50, 200, 400, 1_000]);
const ABILITY: ([i64; 5], [i64; 5]) =
    ([100, 500, 2_500, 10_000, 25_000], [20, 50, 200, 400, 1_000]);
const TELEPORT: ([i64; 5], [i64; 5]) = ([50, 200, 500, 1_000, 2_500], [20, 50, 200, 400, 1_000]);

// --- Enemy kill tables -----------------------------------------------------

const MONSTER: ([i64; 5], [i64; 5]) = (
    [2_000, 5_000, 10_000, 50_000, 100_000],
    [20, 50, 200, 400, 1_000],
);
const GOD: ([i64; 5], [i64; 5]) = (
    [200, 1_000, 5_000, 10_000, 50_000],
    [20, 100, 400, 1_000, 2_500],
);
const LESSER_GOD: ([i64; 5], [i64; 5]) = (
    [100, 500, 2_000, 5_000, 25_000],
    [20, 100, 400, 1_000, 2_500],
);
const ORYX: ([i64; 5], [i64; 5]) = ([1, 20, 100, 250, 500], [200, 500, 1_000, 3_000, 8_000]);
const ENCOUNTER: ([i64; 5], [i64; 5]) = ([5, 50, 200, 500, 1_250], [20, 200, 800, 2_000, 5_000]);
const HERO: ([i64; 5], [i64; 5]) = ([5, 100, 500, 1_250, 2_500], [20, 100, 400, 1_000, 2_500]);
/// Shared table for cube kills and per-type kills.
const STANDARD_KILL: ([i64; 5], [i64; 5]) = (
    [200, 1_000, 5_000, 10_000, 50_000],
    [20, 100, 400, 1_000, 2_500],
);

// --- Biome kill table ------------------------------------------------------
//
// The official (correct) biome kill table is identical to `STANDARD_KILL`:
//
//   const BIOME_KILL_CORRECT: ([i64; 5], [i64; 5]) =
//       ([200, 1_000, 5_000, 10_000, 50_000], [20, 100, 400, 1_000, 2_500]);
//
// However, the live game currently has a bug where each *even* biome tier
// completes one threshold early -- at the threshold of the preceding tier:
//
//   * Tier 2 "Adversary" (+100, intended at 1,000 kills) completes at 200
//     (tier 1 "Foe"'s threshold), so any biome with >=200 kills pays +120.
//   * Tier 4 "Slaughterer" (+1,000, intended at 10,000 kills) completes at
//     5,000 (tier 3 "Slayer"'s threshold), so any biome with >=5,000 kills
//     pays the +400 and +1,000 tiers together.
//
// Verified against a test Necromancer (Wither 295 -> +120, Shipwreck Cove
// 915 -> +120, Dead Church 35,494 -> +1,520) and a Druid whose unfolded Beach
// tier (320 kills) shows Foe +20 and Adversary +100 both complete. The tier 4
// behaviour is confirmed by the Deca bug report ("when you hit 5000 biome
// kills it also counts the 10000 kill fame bonus").
//
// This is a known, reported Deca-side bug. Because RH's dead-fame estimate is
// meant to predict the fame the game will actually award on death, we mirror
// the game's real payout here. When Deca fixes the game, swap `BIOME_KILL` back
// to `BIOME_KILL_CORRECT` (thresholds [200, 1_000, 5_000, 10_000, 50_000]) and
// update the tests.
const BIOME_KILL: ([i64; 5], [i64; 5]) = (
    [200, 200, 5_000, 5_000, 50_000],
    [20, 100, 400, 1_000, 2_500],
);

/// Common threshold ladder for every dungeon-completion bonus.
const DUNGEON_THRESHOLDS: [i64; 5] = [1, 10, 20, 40, 100];

/// Maxing bonus names, in order [HP, MP, ATT, DEF, SPD, DEX, VIT, WIS].
/// HP/MP give +5%/+200, all others give +2.5%/+100.
const MAXED_STAT_NAMES: [&str; 8] = [
    "Maxed HP",
    "Maxed MP",
    "Maxed ATT",
    "Maxed DEF",
    "Maxed SPD",
    "Maxed DEX",
    "Maxed VIT",
    "Maxed WIS",
];

fn maxed_stat_bonus(index: usize) -> (i64, PercentTenths) {
    match index {
        0 | 1 => (200, 50), // HP, MP: +5%
        _ => (100, 25),     // others: +2.5%
    }
}

/// Per-dungeon completion tier bonuses, keyed by display name (matching the
/// names returned by [`super::collections::get_dungeon_list`]).
/// Values are the tier-1..5 fame rewards; thresholds are `DUNGEON_THRESHOLDS`.
/// `[0; 5]` marks dungeons with no known Fame Overview bonus (legacy/new).
fn dungeon_bonuses(name: &str) -> [i64; 5] {
    match name {
        "Pirate Cave" => [2, 5, 10, 20, 60],
        "Forest Maze" => [2, 5, 10, 20, 60],
        "Forbidden Jungle" => [4, 8, 15, 30, 80],
        "The Hive" => [4, 8, 15, 30, 80],
        "Spider Den" => [5, 10, 20, 40, 100],
        "Ancient Ruins" => [40, 80, 150, 300, 800],
        "Snake Pit" => [25, 50, 100, 200, 500],
        "Sprite World" => [25, 50, 100, 200, 500],
        "Magic Woods" => [35, 70, 140, 280, 700],
        "Undead Lair" => [35, 70, 140, 280, 700],
        "Abyss of Demons" => [40, 80, 150, 300, 800],
        "Cursed Library" => [50, 100, 200, 400, 1_000],
        "Toxic Sewers" => [40, 80, 150, 300, 800],
        "Mad Lab" => [50, 100, 200, 400, 1_000],
        "Puppet Master's Theatre" => [40, 80, 150, 300, 800],
        "Manor of the Immortals" => [40, 80, 150, 300, 800],
        "Haunted Cemetery" => [60, 120, 250, 500, 1_200],
        "Cave of a Thousand Treasures" => [60, 120, 250, 500, 1_200],
        "Candyland Hunting Grounds" => [60, 120, 250, 500, 1_200],
        "Parasite Chambers" => [150, 300, 600, 1_200, 3_000],
        "Deadwater Docks" => [100, 200, 400, 800, 2_000],
        "Woodland Labyrinth" => [100, 200, 400, 800, 2_000],
        "The Crawling Depths" => [100, 200, 400, 800, 2_000],
        "Sulfurous Wetlands" => [100, 200, 400, 800, 2_000],
        "Davy Jones' Locker" => [100, 200, 400, 800, 2_000],
        "Mountain Temple" => [150, 300, 600, 1_200, 3_000],
        "Lair of Draconis" => [175, 350, 700, 1_400, 3_500],
        "The Third Dimension" => [150, 300, 600, 1_200, 3_000],
        // Wiki lists Ice Citadel tier 1 as 120, but the game awards 125
        // (verified in-game: Scout +125, Explorer +250 -> 375 at 10 clears).
        "Ice Citadel" => [125, 250, 500, 1_000, 2_500],
        "Ocean Trench" => [175, 350, 700, 1_400, 3_500],
        "Tomb of the Ancients" => [200, 400, 800, 1_600, 4_000],
        "The Shatters" => [500, 1_000, 2_000, 4_000, 10_000],
        "The Nest" => [200, 400, 800, 1_600, 4_000],
        "Fungal Cavern" => [250, 500, 1_000, 2_000, 5_000],
        "Crystal Cavern" => [300, 600, 1_200, 2_400, 6_000],
        "Lost Halls" => [300, 600, 1_200, 2_400, 6_000],
        "Cultist Hideout" => [350, 700, 1_500, 3_000, 7_000],
        "The Void" => [400, 800, 1_600, 3_200, 8_000],
        "Kogbold Steamworks" => [400, 800, 1_600, 3_200, 8_000],
        "Advanced Kogbold Steamworks" => [400, 800, 1_600, 3_200, 8_000],
        "Moonlight Village" => [300, 600, 1_200, 2_400, 6_000],
        "Plagued Nest" => [200, 400, 800, 1_600, 4_000],
        "The Tavern" => [150, 300, 600, 1_200, 3_000],
        "Oryx's Castle" => [100, 200, 400, 800, 2_000],
        "Lair of Shaitan" => [100, 200, 400, 800, 2_000],
        "Puppet Master's Encore" => [100, 200, 400, 800, 2_000],
        "Cnidarian Reef" => [100, 200, 400, 800, 2_000],
        "Secluded Thicket" => [125, 250, 500, 1_000, 2_500],
        "High Tech Terror" => [125, 250, 500, 1_000, 2_500],
        "Oryx's Chamber" => [125, 250, 500, 1_000, 2_500],
        "Wine Cellar" => [200, 400, 800, 1_600, 4_000],
        "Oryx's Sanctuary" => [500, 1_000, 2_000, 4_000, 10_000],
        "Belladonna's Garden" => [150, 300, 600, 1_200, 3_000],
        "The Machine" => [100, 200, 400, 800, 2_000],
        "Rainbow Road" => [5, 10, 20, 40, 100],
        "Mad God Mayhem" => [150, 300, 600, 1_200, 3_000],
        "Malogia" => [125, 250, 500, 1_000, 2_500],
        "Untaris" => [125, 250, 500, 1_000, 2_500],
        "Forax" => [125, 250, 500, 1_000, 2_500],
        "Katalund" => [125, 250, 500, 1_000, 2_500],
        "Battle for the Nexus" => [150, 300, 600, 1_200, 3_000],
        "Ice Tomb" => [150, 300, 600, 1_200, 3_000],
        "Santa's Workshop" => [5, 10, 20, 40, 100],
        "Beachzone" => [5, 10, 20, 40, 100],
        "Hidden Interregnum" => [5, 10, 20, 40, 100],
        "Queen Bunny Chamber" => [5, 10, 20, 40, 100],
        "Spectral Penitentiary" => [300, 600, 1_200, 2_400, 6_000],
        "Heroic Undead Lair" => [150, 300, 600, 1_200, 3_000],
        "Infernal Abyss of Demons" => [150, 300, 600, 1_200, 3_000],
        "Neo Malogia" => [200, 400, 800, 1_600, 4_000],
        "Neo Untaris" => [200, 400, 800, 1_600, 4_000],
        "Neo Forax" => [200, 400, 800, 1_600, 4_000],
        "Neo Katalund" => [200, 400, 800, 1_600, 4_000],
        // No known Fame Overview bonus (legacy/unlisted): contribute nothing.
        _ => [0; 5],
    }
}

/// Fame earned for completing a single dungeon `count` times, by display name.
pub fn dungeon_completion_bonus(name: &str, count: i64) -> i64 {
    tiered(count, DUNGEON_THRESHOLDS, dungeon_bonuses(name))
}

/// Whether a dungeon awards any Fame Overview bonus at all. `false` for
/// legacy/unlisted dungeons (e.g. White Snake Invasion, Stromwell's Rift).
pub fn dungeon_awards_fame(name: &str) -> bool {
    dungeon_bonuses(name) != [0; 5]
}

/// The first-tier (tier 1) fame reward for a dungeon, i.e. what a single
/// completion grants. `0` for dungeons that award no fame.
pub fn dungeon_first_tier_bonus(name: &str) -> i64 {
    dungeon_bonuses(name)[0]
}

fn dungeon_completion_total(d: &DungeonStats) -> i64 {
    super::collections::get_dungeon_list(d)
        .into_iter()
        .map(|(name, count)| dungeon_completion_bonus(name, count as i64))
        .sum()
}

/// Fame earned for a statistics row (STATISTICS column), by its display label.
/// Returns 0 for rows that grant no fame (e.g. Shots Fired, Assists).
pub fn stat_line_bonus(label: &str, count: i64) -> i64 {
    match stat_table(label) {
        Some((thresholds, bonuses)) => tiered(count, thresholds, bonuses),
        None => 0,
    }
}

/// Threshold/bonus table for a STATISTICS row, or `None` for rows that grant no
/// fame. Shared by the earned-bonus and first-tier-prediction helpers.
fn stat_table(label: &str) -> Option<([i64; 5], [i64; 5])> {
    Some(match label {
        "Tiles Discovered" => CARTOGRAPHY,
        "Quests Completed" => QUESTS,
        "Hits" => COMBAT,
        "Party Level-ups" => PARTY,
        "Potions Drunk" => POTIONS,
        "Ability Uses" => ABILITY,
        "Teleports" => TELEPORT,
        "Kills" => MONSTER,
        "God Kills" => GOD,
        "Lesser Gods Kills" => LESSER_GOD,
        "Oryx Kills" => ORYX,
        "Encounter Kills" => ENCOUNTER,
        "Hero Kills" => HERO,
        "Cube Kills" | "Critter Kills" | "Beast Kills" | "Humanoid Kills" | "Undead Kills"
        | "Nature Kills" | "Construct Kills" | "Grotesque Kills" | "Structure Kills" => {
            STANDARD_KILL
        }
        _ => return None,
    })
}

/// The first-tier fame reward for a STATISTICS row, i.e. the bonus granted when
/// its first threshold is reached. `0` for rows that never award fame.
pub fn stat_first_tier_bonus(label: &str) -> i64 {
    stat_table(label)
        .map(|(_, bonuses)| bonuses[0])
        .unwrap_or(0)
}

/// Fame earned for a single biome's enemy kills (BIOME ENEMY KILLS column).
///
/// Uses [`BIOME_KILL`], which mirrors the game's current (buggy) payout. See
/// the `BIOME_KILL` definition for details.
pub fn biome_kill_bonus(count: i64) -> i64 {
    tiered(count, BIOME_KILL.0, BIOME_KILL.1)
}

/// The first-tier fame reward for a biome's enemy kills, i.e. the bonus granted
/// when its first threshold is reached.
pub fn biome_first_tier_bonus() -> i64 {
    BIOME_KILL.1[0]
}

/// All biome kill counts, in a fixed order, sharing `STANDARD_KILL`.
fn biome_counts(b: &BiomeStats) -> [i64; 20] {
    [
        b.ruins_enemy_kills as i64,
        b.beach_enemy_kills as i64,
        b.undead_forest_enemy_kills as i64,
        b.forest_enemy_kills as i64,
        b.plains_enemy_kills as i64,
        b.wither_enemy_kills as i64,
        b.dark_forest_enemy_kills as i64,
        b.desert_enemy_kills as i64,
        b.coral_reefs_enemy_kills as i64,
        b.sprite_forest_enemy_kills as i64,
        b.haunted_hallows_enemy_kills as i64,
        b.shipwreck_cove_enemy_kills as i64,
        b.dead_church_enemy_kills as i64,
        b.risen_hells_enemy_kills as i64,
        b.abandoned_city_enemy_kills as i64,
        b.deep_sea_abyss_enemy_kills as i64,
        b.carboniferous_enemy_kills as i64,
        b.floral_escape_enemy_kills as i64,
        b.sanguine_forest_enemy_kills as i64,
        b.runic_tundra_kills as i64,
    ]
}

/// Fame contribution a dungeon collection adds to dead fame when completed:
/// `flat + round(base_fame * percent / 100)`.
pub fn collection_bonus(base_fame: i64, collection: &DungeonCollection) -> i64 {
    let percent_tenths = (collection.fame_percent * 10.0).round() as i64;
    collection.fame_flat as i64 + ceil_percent_bonus(base_fame, percent_tenths)
}

fn add_flat<F: FnMut(&str, i64, PercentTenths)>(add: &mut F, name: &str, flat: i64) {
    if flat > 0 {
        add(name, flat, 0);
    }
}

fn add_bonus_lines<F: FnMut(&str, i64, PercentTenths)>(
    stats: &CharacterStats,
    maxed: [bool; 8],
    mut add: F,
) {
    for (i, &is_maxed) in maxed.iter().enumerate() {
        if is_maxed {
            let (flat, pct) = maxed_stat_bonus(i);
            add(MAXED_STAT_NAMES[i], flat, pct);
        }
    }

    add_flat(
        &mut add,
        "Cartography",
        tiered(stats.tiles_discovered as i64, CARTOGRAPHY.0, CARTOGRAPHY.1),
    );
    add_flat(
        &mut add,
        "Quests",
        tiered(stats.quests_completed as i64, QUESTS.0, QUESTS.1),
    );
    add_flat(
        &mut add,
        "Combat Proficiency",
        tiered(stats.hits as i64, COMBAT.0, COMBAT.1),
    );
    add_flat(
        &mut add,
        "Party Level Ups",
        tiered(stats.party_level_ups as i64, PARTY.0, PARTY.1),
    );
    add_flat(
        &mut add,
        "Potions",
        tiered(stats.potions_drunk as i64, POTIONS.0, POTIONS.1),
    );
    add_flat(
        &mut add,
        "Ability",
        tiered(stats.ability_used as i64, ABILITY.0, ABILITY.1),
    );
    add_flat(
        &mut add,
        "Teleport",
        tiered(stats.teleports as i64, TELEPORT.0, TELEPORT.1),
    );
    add_flat(
        &mut add,
        "Monster Kills",
        tiered(stats.kills as i64, MONSTER.0, MONSTER.1),
    );
    add_flat(
        &mut add,
        "God Kills",
        tiered(stats.god_kills as i64, GOD.0, GOD.1),
    );
    add_flat(
        &mut add,
        "Lesser God Kills",
        tiered(stats.lesser_gods_kills as i64, LESSER_GOD.0, LESSER_GOD.1),
    );
    add_flat(
        &mut add,
        "Oryx Kills",
        tiered(stats.oryx_kills as i64, ORYX.0, ORYX.1),
    );
    add_flat(
        &mut add,
        "Encounter Kills",
        tiered(stats.encounter_kills as i64, ENCOUNTER.0, ENCOUNTER.1),
    );
    add_flat(
        &mut add,
        "Hero Kills",
        tiered(stats.hero_kills as i64, HERO.0, HERO.1),
    );
    add_flat(
        &mut add,
        "Cube Kills",
        tiered(stats.cube_kills as i64, STANDARD_KILL.0, STANDARD_KILL.1),
    );

    let type_kills = [
        ("Critter Kills", stats.critter_kills as i64),
        ("Beast Kills", stats.beast_kills as i64),
        ("Humanoid Kills", stats.humanoid_kills as i64),
        ("Undead Kills", stats.undead_kills as i64),
        ("Nature Kills", stats.nature_kills as i64),
        ("Construct Kills", stats.construct_kills as i64),
        ("Grotesque Kills", stats.grotesque_kills as i64),
        ("Structure Kills", stats.structure_kills as i64),
    ];
    for (name, count) in type_kills {
        add_flat(
            &mut add,
            name,
            tiered(count, STANDARD_KILL.0, STANDARD_KILL.1),
        );
    }

    let biome_total: i64 = biome_counts(&stats.biomes)
        .into_iter()
        .map(biome_kill_bonus)
        .sum();
    add_flat(&mut add, "Biome Kills", biome_total);

    add_flat(
        &mut add,
        "Dungeon Completion",
        dungeon_completion_total(&stats.dungeons),
    );
}

/// Compute the full dead-fame breakdown for a living character.
pub fn dead_fame_breakdown(
    base_fame: i64,
    stats: &CharacterStats,
    maxed: [bool; 8],
    completed_collections: &[&DungeonCollection],
) -> FameBonusBreakdown {
    let mut lines = Vec::new();
    add_bonus_lines(stats, maxed, |name, flat, percent_tenths| {
        lines.push(FameBonusLine {
            name: name.to_string(),
            flat,
            percent_tenths,
        });
    });
    for collection in completed_collections {
        lines.push(FameBonusLine {
            name: collection.name.to_string(),
            flat: collection.fame_flat as i64,
            percent_tenths: (collection.fame_percent * 10.0).round() as i64,
        });
    }
    FameBonusBreakdown { base_fame, lines }
}

/// Estimate total dead fame without allocating a display breakdown.
pub fn dead_fame_total(
    base_fame: i64,
    stats: &CharacterStats,
    maxed: [bool; 8],
    collections: &[DungeonCollection],
) -> i64 {
    let mut bonus = 0;
    add_bonus_lines(stats, maxed, |_, flat, percent_tenths| {
        bonus += flat + ceil_percent_bonus(base_fame, percent_tenths);
    });
    for collection in collections
        .iter()
        .filter(|collection| collection.is_complete(&stats.dungeons))
    {
        let percent_tenths = (collection.fame_percent * 10.0).round() as i64;
        bonus += collection.fame_flat as i64 + ceil_percent_bonus(base_fame, percent_tenths);
    }
    base_fame + bonus
}

// --- Fame tier breakdown (per-category tooltip) ----------------------------
//
// Tier display names come from <https://www.realmeye.com/wiki/fame-bonuses>.
// Every enemy/biome kill category shares the Foe..Nemesis ladder; every dungeon
// shares the Scout..Master ladder. The five stat categories below have unique
// names. The fifth (top) tier repeats and is shown with a Roman-numeral suffix.

const ENEMY_TIER_NAMES: [&str; 5] = ["Foe", "Adversary", "Slayer", "Slaughterer", "Nemesis"];
const DUNGEON_TIER_NAMES: [&str; 5] = ["Scout", "Explorer", "Adventurer", "Conqueror", "Master"];
const CARTOGRAPHY_NAMES: [&str; 5] = [
    "Traveller",
    "Mapper",
    "Explorer",
    "Cartographer",
    "Map Maker",
];
const QUEST_NAMES: [&str; 5] = [
    "Pursuer of Deeds",
    "Doer of Deeds",
    "Performer of Deeds",
    "Achiever of Deeds",
    "Master of Deeds",
];
const COMBAT_NAMES: [&str; 5] = [
    "Combat Novice",
    "Combat Beginner",
    "Combat Proficient",
    "Combat Expert",
    "Combat Veteran",
];
const PARTY_NAMES: [&str; 5] = [
    "Supporter",
    "Collaborator",
    "Team Player",
    "Leader of Men",
    "Builder of Armies",
];
const POTION_NAMES: [&str; 5] = [
    "Potion Drinker",
    "Potion Enthusiast",
    "Potion Connoisseur",
    "Potion Sage",
    "Potion Fanatic",
];
const ABILITY_NAMES: [&str; 5] = [
    "Ability Neophyte",
    "Ability Apprentice",
    "Ability Ace",
    "Ability Expert",
    "Ability Specialist",
];
const TELEPORT_NAMES: [&str; 5] = [
    "Teleport Newcomer",
    "Teleport Rookie",
    "Teleport Alumni",
    "Teleport Zealot",
    "Teleport Fanatic",
];

/// A single tier row in a fame-category breakdown.
#[derive(Debug, Clone)]
pub struct FameTier {
    pub name: String,
    pub reward: i64,
    pub achieved: bool,
}

/// Per-category fame breakdown for the hover tooltip: the earned tiers plus the
/// single next unearned tier, the running total, and progress toward the next.
#[derive(Debug, Clone)]
pub struct FameTierBreakdown {
    pub category: String,
    pub total: i64,
    pub count: i64,
    pub tiers: Vec<FameTier>,
    /// Fill fraction (0..1) of the connector leading to the next (grey) tier,
    /// or `None` when every tier (including the 20-rep cap) is achieved.
    pub next_progress: Option<f32>,
    /// Completions/count still needed to reach the next (grey) tier, or `None`
    /// when every tier (including the 20-rep cap) is achieved.
    pub next_remaining: Option<i64>,
    /// Whether to show the "top tier caps at 20 repetitions" footnote.
    pub show_cap_note: bool,
    /// Base name of the repeating top tier (e.g. "Ability Specialist", "Master"),
    /// used in the cap footnote.
    pub top_tier_name: String,
}

/// Roman numeral for 1..=20 (the repeating top-tier suffix).
fn roman(n: i64) -> String {
    const NUMERALS: [(i64, &str); 7] = [
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
        (0, ""),
        (0, ""),
    ];
    let mut n = n;
    let mut out = String::new();
    for &(v, s) in NUMERALS.iter() {
        if v == 0 {
            break;
        }
        while n >= v {
            out.push_str(s);
            n -= v;
        }
    }
    out
}

fn build_breakdown(
    category: &str,
    names: [&str; 5],
    thresholds: [i64; 5],
    bonuses: [i64; 5],
    count: i64,
) -> FameTierBreakdown {
    let total = tiered(count, thresholds, bonuses);

    // Ordered (name, reward, reach_count) for named tiers 1-4 plus the repeating
    // top tier expanded to its 20-rep cap.
    let mut ladder: Vec<(String, i64, i64)> = Vec::new();
    for i in 0..4 {
        ladder.push((names[i].to_string(), bonuses[i], thresholds[i]));
    }
    if thresholds[4] > 0 {
        for r in 1..=MAX_TOP_TIER_REPS {
            ladder.push((
                format!("{} {}", names[4], roman(r)),
                bonuses[4],
                thresholds[4] * r,
            ));
        }
    }

    let mut tiers = Vec::new();
    let mut prev_reach = 0i64;
    let mut next_progress = None;
    let mut next_remaining = None;
    for (name, reward, reach) in ladder {
        if count >= reach {
            tiers.push(FameTier {
                name,
                reward,
                achieved: true,
            });
            prev_reach = reach;
        } else {
            let denom = (reach - prev_reach).max(1);
            next_progress = Some(((count - prev_reach) as f32 / denom as f32).clamp(0.0, 1.0));
            next_remaining = Some((reach - count).max(0));
            tiers.push(FameTier {
                name,
                reward,
                achieved: false,
            });
            break;
        }
    }

    FameTierBreakdown {
        category: category.to_string(),
        total,
        count,
        tiers,
        next_progress,
        next_remaining,
        show_cap_note: count >= thresholds[3],
        top_tier_name: names[4].to_string(),
    }
}

/// Fame tier breakdown for a STATISTICS row, or `None` if the row grants no fame.
pub fn stat_tier_breakdown(label: &str, count: i64) -> Option<FameTierBreakdown> {
    let (display, names, table) = match label {
        "Tiles Discovered" => ("Cartography", CARTOGRAPHY_NAMES, CARTOGRAPHY),
        "Quests Completed" => ("Quest Completes", QUEST_NAMES, QUESTS),
        "Hits" => ("Combat Proficiency", COMBAT_NAMES, COMBAT),
        "Party Level-ups" => ("Party Level Ups", PARTY_NAMES, PARTY),
        "Potions Drunk" => ("Potions", POTION_NAMES, POTIONS),
        "Ability Uses" => ("Ability", ABILITY_NAMES, ABILITY),
        "Teleports" => ("Teleport", TELEPORT_NAMES, TELEPORT),
        "Kills" => ("Monster Kills", ENEMY_TIER_NAMES, MONSTER),
        "God Kills" => ("God Kills", ENEMY_TIER_NAMES, GOD),
        "Lesser Gods Kills" => ("Lesser God Kills", ENEMY_TIER_NAMES, LESSER_GOD),
        "Oryx Kills" => ("Oryx Kills", ENEMY_TIER_NAMES, ORYX),
        "Encounter Kills" => ("Encounter Kills", ENEMY_TIER_NAMES, ENCOUNTER),
        "Hero Kills" => ("Hero Kills", ENEMY_TIER_NAMES, HERO),
        "Cube Kills" | "Critter Kills" | "Beast Kills" | "Humanoid Kills" | "Undead Kills"
        | "Nature Kills" | "Construct Kills" | "Grotesque Kills" | "Structure Kills" => {
            (label, ENEMY_TIER_NAMES, STANDARD_KILL)
        }
        _ => return None,
    };
    Some(build_breakdown(display, names, table.0, table.1, count))
}

/// Fame tier breakdown for a single biome's kills. Uses the same (buggy)
/// thresholds as [`biome_kill_bonus`], so the achieved tiers match the payout.
pub fn biome_tier_breakdown(biome_name: &str, count: i64) -> FameTierBreakdown {
    build_breakdown(
        biome_name,
        ENEMY_TIER_NAMES,
        BIOME_KILL.0,
        BIOME_KILL.1,
        count,
    )
}

/// Fame tier breakdown for a dungeon's completions, or `None` for dungeons with
/// no known Fame Overview bonus.
pub fn dungeon_tier_breakdown(name: &str, count: i64) -> Option<FameTierBreakdown> {
    let bonuses = dungeon_bonuses(name);
    if bonuses == [0; 5] {
        return None;
    }
    Some(build_breakdown(
        name,
        DUNGEON_TIER_NAMES,
        DUNGEON_THRESHOLDS,
        bonuses,
        count,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every STATISTICS label that awards fame must expose a positive first-tier
    /// bonus and a tier breakdown, so the panel's predicted-fame preview and the
    /// dash fallback stay consistent.
    #[test]
    fn stat_first_tier_matches_breakdown_labels() {
        const FAME_LABELS: [&str; 21] = [
            "Tiles Discovered",
            "Quests Completed",
            "Hits",
            "Party Level-ups",
            "Potions Drunk",
            "Ability Uses",
            "Teleports",
            "Kills",
            "God Kills",
            "Lesser Gods Kills",
            "Oryx Kills",
            "Encounter Kills",
            "Hero Kills",
            "Cube Kills",
            "Critter Kills",
            "Beast Kills",
            "Humanoid Kills",
            "Undead Kills",
            "Nature Kills",
            "Construct Kills",
            "Grotesque Kills",
        ];
        for label in FAME_LABELS {
            assert!(
                stat_first_tier_bonus(label) > 0,
                "no first-tier bonus for {label}"
            );
            assert!(
                stat_tier_breakdown(label, 0).is_some(),
                "no breakdown for {label}"
            );
        }
        // A non-fame stat must expose neither, so the panel shows a dash.
        assert_eq!(stat_first_tier_bonus("Shots Fired"), 0);
        assert!(stat_tier_breakdown("Shots Fired", 0).is_none());
        // Biome first-tier preview must be positive.
        assert!(biome_first_tier_bonus() > 0);
    }

    #[test]
    fn tiered_cumulative_and_repeating_top() {
        let t = [200, 1_000, 5_000, 10_000, 50_000];
        let b = [20, 100, 400, 1_000, 2_500];
        // Below first threshold.
        assert_eq!(tiered(199, t, b), 0);
        // First tier only.
        assert_eq!(tiered(200, t, b), 20);
        // Tiers 1-3.
        assert_eq!(tiered(5_000, t, b), 20 + 100 + 400);
        // Tiers 1-4 plus top tier once at exactly 50k.
        assert_eq!(tiered(50_000, t, b), 20 + 100 + 400 + 1_000 + 2_500);
        // Top tier repeats: 120k -> floor(120k/50k) = 2 top-tier awards.
        assert_eq!(tiered(120_000, t, b), 20 + 100 + 400 + 1_000 + 2 * 2_500);
    }

    #[test]
    fn percent_and_flat_combine_on_base() {
        let breakdown = FameBonusBreakdown {
            base_fame: 10_000,
            lines: vec![
                FameBonusLine {
                    name: "Maxed HP".into(),
                    flat: 200,
                    percent_tenths: 50,
                },
                FameBonusLine {
                    name: "Quests".into(),
                    flat: 800,
                    percent_tenths: 0,
                },
            ],
        };
        // percent: 5% of 10,000 = 500; flat: 1000; total bonus = 1500.
        assert_eq!(breakdown.flat_total(), 1_000);
        assert_eq!(breakdown.percent_tenths_total(), 50);
        assert_eq!(breakdown.bonus_fame(), 1_500);
        assert_eq!(breakdown.total_dead_fame(), 11_500);
    }

    #[test]
    fn ice_citadel_tier_one_matches_the_game() {
        // The game awards Ice Citadel Scout (tier 1) as +125, not the wiki's
        // 120. Verified in-game at 10 clears: Scout +125, Explorer +250 = 375.
        assert_eq!(dungeon_completion_bonus("Ice Citadel", 1), 125);
        assert_eq!(dungeon_completion_bonus("Ice Citadel", 10), 375);
    }

    #[test]
    fn collection_bonus_uses_percent_and_flat() {
        // Hero of the Nexus: +12.5%, +5000.
        let c = DungeonCollection {
            name: "Test",
            dungeons: &[],
            fame_percent: 12.5,
            fame_flat: 5_000,
        };
        // 12.5% of 8_000 = 1_000; + 5_000 flat = 6_000.
        assert_eq!(collection_bonus(8_000, &c), 6_000);
    }

    #[test]
    fn percentage_bonuses_round_up_per_line() {
        // Percentages are rounded up (ceiling), matching the in-game overview.
        let c = DungeonCollection {
            name: "Test",
            dungeons: &[],
            fame_percent: 5.0,
            fame_flat: 0,
        };
        // 5% of 9 = 0.45 -> 1; of 10 = 0.5 -> 1; of 20 = 1.0 -> 1 (exact, no +1).
        assert_eq!(collection_bonus(9, &c), 1);
        assert_eq!(collection_bonus(10, &c), 1);
        assert_eq!(collection_bonus(20, &c), 1);
        assert_eq!(collection_bonus(21, &c), 2);

        // 7.5% of 180,723 = 13,554.225 -> 13,555 (undercounted as 13,554 before).
        let c2 = DungeonCollection {
            fame_percent: 7.5,
            ..c
        };
        assert_eq!(collection_bonus(180_723, &c2), 13_555);

        let breakdown = FameBonusBreakdown {
            base_fame: 10,
            lines: vec![FameBonusLine {
                name: "Test".into(),
                flat: 0,
                percent_tenths: 50,
            }],
        };
        assert_eq!(breakdown.bonus_fame(), 1);
    }

    #[test]
    fn maxed_percentages_ceil_each_stat() {
        // 8/8 maxed at 180,723 base fame: 2x 5% + 6x 2.5%, each ceiled.
        // ceil(9,036.15)=9,037 twice; ceil(4,518.075)=4,519 six times; +1,000 flat.
        let stats = CharacterStats::default();
        let breakdown = dead_fame_breakdown(180_723, &stats, [true; 8], &[]);
        assert_eq!(breakdown.bonus_fame(), 2 * 9_037 + 6 * 4_519 + 1_000);
    }

    #[test]
    fn aggregate_total_matches_breakdown() {
        let stats = CharacterStats::default();
        let collections = [DungeonCollection {
            name: "Test",
            dungeons: &[],
            fame_percent: 12.5,
            fame_flat: 500,
        }];
        let completed: Vec<&DungeonCollection> = collections.iter().collect();
        let breakdown = dead_fame_breakdown(1_234, &stats, [true; 8], &completed);

        assert_eq!(
            dead_fame_total(1_234, &stats, [true; 8], &collections),
            breakdown.total_dead_fame()
        );
    }

    #[test]
    fn maxed_stats_award_expected_lines() {
        let stats = CharacterStats::default();
        // All eight stats maxed: 2x(+5%,+200) + 6x(+2.5%,+100).
        let breakdown = dead_fame_breakdown(0, &stats, [true; 8], &[]);
        // percent tenths: 2*50 + 6*25 = 250 (25%). flat: 2*200 + 6*100 = 1000.
        assert_eq!(breakdown.percent_tenths_total(), 250);
        assert_eq!(breakdown.flat_total(), 1_000);
    }

    #[test]
    fn biome_kills_match_the_games_current_buggy_payout() {
        // The live game pays each even biome tier one threshold early:
        //   * +100 "Adversary" starts at 200 (tier 1's threshold), so every
        //     biome with >=200 kills is paid +120 up until 5,000.
        //   * +1,000 "Slaughterer" starts at 5,000 (tier 3's threshold), so
        //     every biome with >=5,000 kills is paid +400 and +1,000 together.
        // Verified against a test Necromancer (biome subtotal 3,560) and the
        // Deca bug report ("when you hit 5000 biome kills it also counts the
        // 10000 kill fame bonus").
        assert_eq!(biome_kill_bonus(0), 0);
        assert_eq!(biome_kill_bonus(120), 0); // Dark Forest (necro)
        assert_eq!(biome_kill_bonus(191), 0); // Desert (necro)
        assert_eq!(biome_kill_bonus(200), 120); // first paid tier -> +20+100
        assert_eq!(biome_kill_bonus(295), 120); // Wither (necro)
        assert_eq!(biome_kill_bonus(504), 120); // Beach (necro), below old 1,000
        assert_eq!(biome_kill_bonus(999), 120);
        assert_eq!(biome_kill_bonus(1_000), 120); // no jump at the old +100 tier
        assert_eq!(biome_kill_bonus(2_028), 120); // Ruins (necro)
        assert_eq!(biome_kill_bonus(4_999), 120);
        assert_eq!(biome_kill_bonus(5_000), 1_520); // +400 and +1,000 together
        assert_eq!(biome_kill_bonus(9_999), 1_520); // no jump at the old +1,000 tier
        assert_eq!(biome_kill_bonus(10_000), 1_520);
        assert_eq!(biome_kill_bonus(35_494), 1_520); // Dead Church (necro)
        assert_eq!(biome_kill_bonus(50_000), 4_020); // +2,500 repeating top tier
    }

    #[test]
    fn biome_kill_correct_values_for_when_deca_fixes_it() {
        // Documents the wiki-correct payout (BIOME_KILL_CORRECT) so this test
        // fails loudly and reminds us to update it once the game is fixed and
        // BIOME_KILL is swapped back. Same thresholds as STANDARD_KILL.
        let correct = |count: i64| tiered(count, STANDARD_KILL.0, STANDARD_KILL.1);
        assert_eq!(correct(191), 0);
        assert_eq!(correct(295), 20); // correct: only the +20 tier below 1,000
        assert_eq!(correct(999), 20);
        assert_eq!(correct(1_000), 120); // correct: +100 tier starts at 1,000
        assert_eq!(correct(5_000), 520); // correct: +400 tier, no +1,000 yet
        assert_eq!(correct(9_999), 520);
        assert_eq!(correct(10_000), 1_520); // correct: +1,000 tier starts at 10,000
        assert_eq!(correct(35_494), 1_520);

        // While the game bug stands, the two diverge below the correct thresholds.
        assert_ne!(biome_kill_bonus(295), correct(295));
        assert_ne!(biome_kill_bonus(5_000), correct(5_000));
    }

    #[test]
    fn top_tier_repeats_cap_at_twenty() {
        let t = [1, 10, 20, 40, 100];
        let b = [50, 100, 200, 400, 1_000];
        // 20 reps at exactly 2,000 completions (100 * 20).
        let named = 50 + 100 + 200 + 400;
        assert_eq!(tiered(2_000, t, b), named + 20 * 1_000);
        // Beyond the cap the total stops growing.
        assert_eq!(tiered(5_000, t, b), named + 20 * 1_000);
        assert_eq!(tiered(2_000, t, b), tiered(2_500, t, b));
    }

    #[test]
    fn roman_numerals_cover_one_to_twenty() {
        assert_eq!(roman(1), "I");
        assert_eq!(roman(4), "IV");
        assert_eq!(roman(7), "VII");
        assert_eq!(roman(14), "XIV");
        assert_eq!(roman(20), "XX");
    }

    #[test]
    fn cartography_breakdown_matches_the_game_example() {
        // In-game Cartography at 3 Map Maker reps totals +9020 with Map Maker IV
        // as the next (unearned) tier. 25,000,000 * 3 = 75,000,000 tiles.
        let bd = stat_tier_breakdown("Tiles Discovered", 76_000_000).unwrap();
        assert_eq!(bd.category, "Cartography");
        assert_eq!(bd.total, 20 + 100 + 400 + 1_000 + 3 * 2_500); // 9020
                                                                  // Named 1-4 + Map Maker I,II,III achieved + Map Maker IV next.
        assert_eq!(bd.tiers.len(), 8);
        assert_eq!(bd.tiers[0].name, "Traveller");
        assert_eq!(bd.tiers[4].name, "Map Maker I");
        assert_eq!(bd.tiers[6].name, "Map Maker III");
        assert!(bd.tiers[6].achieved);
        assert_eq!(bd.tiers[7].name, "Map Maker IV");
        assert!(!bd.tiers[7].achieved);
        assert!(bd.show_cap_note);
    }

    #[test]
    fn biome_breakdown_shows_both_bugged_low_tiers_achieved() {
        // At 320 kills the game shows Foe (+20) and Adversary (+100) both earned.
        let bd = biome_tier_breakdown("Beach", 320);
        assert_eq!(bd.total, 120);
        assert!(bd.tiers[0].achieved && bd.tiers[0].name == "Foe");
        assert!(bd.tiers[1].achieved && bd.tiers[1].name == "Adversary");
        assert!(!bd.tiers[2].achieved && bd.tiers[2].name == "Slayer");
        // Next tier is Slayer; progress measured from the 200 reach to 5,000.
        let expected = (320.0 - 200.0) / (5_000.0 - 200.0);
        assert!((bd.next_progress.unwrap() - expected).abs() < 1e-4);
    }

    #[test]
    fn dungeon_breakdown_uses_master_ladder_and_skips_unlisted() {
        let bd = dungeon_tier_breakdown("Mad Lab", 40).unwrap();
        assert_eq!(bd.tiers[0].name, "Scout");
        assert_eq!(bd.tiers[4].name, "Master I");
        assert!(dungeon_tier_breakdown("Nonexistent Dungeon", 5).is_none());
    }
}

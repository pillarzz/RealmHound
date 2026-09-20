//! Account-level dungeon snapshot builder.
//!
//! Aggregates owned items (all characters + vault) by dungeon origin,
//! and sums PCStats dungeon completions across all living characters.

use std::collections::HashMap;

use crate::api::{decode_pcstats, DungeonStats};
use crate::assets::{dungeon_difficulty, AssetManager};
use crate::stats::collections::ALL_DUNGEONS;
use crate::vault::AccountData;

/// A single row in the dungeon snapshot: one dungeon's aggregated data.
#[derive(Debug, Clone)]
pub struct DungeonSnapshotRow {
    pub dungeon_name: String,
    pub items: Vec<SnapshotItem>,
    pub total_completions: i32,
    pub difficulty: Option<f32>,
}

/// An item in the snapshot with its count.
#[derive(Debug, Clone)]
pub struct SnapshotItem {
    pub item_id: i32,
    pub count: u32,
}

/// Build an account-level dungeon snapshot.
///
/// Joins all owned items (equipment, inventory, vault) against the ORG_ item-dungeon
/// mapping from assets, then sums PCStats dungeon completions across all living characters.
pub fn build_dungeon_snapshot(
    account_data: &AccountData,
    asset_mgr: &AssetManager,
) -> Vec<DungeonSnapshotRow> {
    let mut item_counts: HashMap<String, HashMap<i32, u32>> = HashMap::new();

    // Collect all item IDs from characters (living + graveyard: a character's
    // items and dungeon progress remain part of the account after death).
    for ch in account_data
        .characters
        .characters
        .iter()
        .chain(account_data.characters.get_dead_characters())
    {
        collect_character_items(ch, asset_mgr, &mut item_counts);
    }

    // Collect vault items (regular + seasonal)
    collect_vault_items(&account_data.regular_vault, asset_mgr, &mut item_counts);
    collect_vault_items(&account_data.seasonal_vault, asset_mgr, &mut item_counts);

    // Sum PCStats dungeon completions across all characters, including the graveyard.
    let summed_dungeons = sum_dungeon_completions(
        account_data
            .characters
            .characters
            .iter()
            .chain(account_data.characters.get_dead_characters()),
    );

    // Build result rows: merge items + completions
    let mut results: HashMap<String, DungeonSnapshotRow> = HashMap::new();

    for (dungeon, items_map) in &item_counts {
        let mut items: Vec<SnapshotItem> = items_map
            .iter()
            .map(|(&item_id, &count)| SnapshotItem { item_id, count })
            .collect();

        // Sort: Shiny UT > UT > ST > Tiered > rest, then by feed power descending
        items.sort_by(|a, b| {
            let a_rank = snapshot_item_rarity_rank(a.item_id, asset_mgr);
            let b_rank = snapshot_item_rarity_rank(b.item_id, asset_mgr);
            b_rank.cmp(&a_rank).then_with(|| {
                let a_feed = asset_mgr
                    .get_object(a.item_id)
                    .map(|o| o.feed_power)
                    .unwrap_or(0);
                let b_feed = asset_mgr
                    .get_object(b.item_id)
                    .map(|o| o.feed_power)
                    .unwrap_or(0);
                b_feed.cmp(&a_feed)
            })
        });

        let completions = get_completions_for_dungeon(dungeon, &summed_dungeons);
        let difficulty = dungeon_difficulty(dungeon);

        results.insert(
            dungeon.clone(),
            DungeonSnapshotRow {
                dungeon_name: dungeon.clone(),
                items,
                total_completions: completions,
                difficulty,
            },
        );
    }

    // Add dungeons that have completions but no items
    for dungeon_def in ALL_DUNGEONS.iter() {
        let name = dungeon_def.name;
        if !results.contains_key(name) {
            let completions = (dungeon_def.get_count)(&summed_dungeons);
            if completions > 0 {
                results.insert(
                    name.to_string(),
                    DungeonSnapshotRow {
                        dungeon_name: name.to_string(),
                        items: Vec::new(),
                        total_completions: completions,
                        difficulty: dungeon_difficulty(name),
                    },
                );
            }
        }
    }

    let mut rows: Vec<DungeonSnapshotRow> = results.into_values().collect();
    rows.sort_by(|a, b| {
        // Sort by difficulty (hardest first), then alphabetical
        let diff_cmp = b
            .difficulty
            .unwrap_or(0.0)
            .partial_cmp(&a.difficulty.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal);
        diff_cmp.then_with(|| a.dungeon_name.cmp(&b.dungeon_name))
    });
    rows
}

fn collect_character_items(
    ch: &crate::vault::CachedCharacter,
    asset_mgr: &AssetManager,
    counts: &mut HashMap<String, HashMap<i32, u32>>,
) {
    let all_items = ch
        .equipment
        .iter()
        .chain(ch.inventory.iter())
        .chain(ch.backpack.iter())
        .chain(ch.backpack_ext.iter())
        .chain(ch.belt.iter());

    for item in all_items {
        if item.item_id > 0 {
            add_item(item.item_id, asset_mgr, counts);
        }
    }
}

fn collect_vault_items(
    vault: &crate::vault::LiveVaultStorage,
    asset_mgr: &AssetManager,
    counts: &mut HashMap<String, HashMap<i32, u32>>,
) {
    let all_items = vault
        .vault_items
        .iter()
        .chain(vault.material_items.iter())
        .chain(vault.gift_items.iter())
        .chain(vault.potion_items.iter())
        .chain(vault.spoils_items.iter());

    for item in all_items {
        if item.item_id > 0 {
            add_item(item.item_id, asset_mgr, counts);
        }
    }
}

fn add_item(
    item_id: i32,
    _asset_mgr: &AssetManager,
    counts: &mut HashMap<String, HashMap<i32, u32>>,
) {
    // Use RealmEye data as primary source
    let realmeye = crate::assets::get_realmeye_drops();
    let dungeons = realmeye.dungeons_for_item(item_id);

    for dungeon in dungeons {
        *counts
            .entry(dungeon.to_string())
            .or_default()
            .entry(item_id)
            .or_default() += 1;
    }
}

fn sum_dungeon_completions<'a>(
    characters: impl Iterator<Item = &'a crate::vault::CachedCharacter>,
) -> DungeonStats {
    let mut summed = DungeonStats::default();
    for ch in characters {
        if ch.pc_stats_raw.is_empty() {
            continue;
        }
        if let Some(stats) = decode_pcstats(&ch.pc_stats_raw) {
            add_dungeon_stats(&mut summed, &stats.dungeons);
        }
    }
    summed
}

fn get_completions_for_dungeon(dungeon: &str, stats: &DungeonStats) -> i32 {
    ALL_DUNGEONS
        .iter()
        .find(|d| d.name == dungeon)
        .map(|d| (d.get_count)(stats))
        .unwrap_or(0)
}

fn add_dungeon_stats(target: &mut DungeonStats, source: &DungeonStats) {
    target.abyss_of_demons += source.abyss_of_demons;
    target.advanced_kogbold_steamworks += source.advanced_kogbold_steamworks;
    target.ancient_ruins += source.ancient_ruins;
    target.battle_for_the_nexus += source.battle_for_the_nexus;
    target.beachzone += source.beachzone;
    target.belladonnas_garden += source.belladonnas_garden;
    target.bilgewaters_grotto += source.bilgewaters_grotto;
    target.candyland_hunting_grounds += source.candyland_hunting_grounds;
    target.cave_of_thousand_treasures += source.cave_of_thousand_treasures;
    target.cnidarian_reef += source.cnidarian_reef;
    target.crystal_cavern += source.crystal_cavern;
    target.cultist_hideout += source.cultist_hideout;
    target.cursed_library += source.cursed_library;
    target.davy_jones_locker += source.davy_jones_locker;
    target.deadwater_docks += source.deadwater_docks;
    target.forax += source.forax;
    target.forbidden_jungle += source.forbidden_jungle;
    target.forest_maze += source.forest_maze;
    target.fungal_cavern += source.fungal_cavern;
    target.haunted_cemetery += source.haunted_cemetery;
    target.hidden_interregnum += source.hidden_interregnum;
    target.high_tech_terror += source.high_tech_terror;
    target.ice_citadel += source.ice_citadel;
    target.ice_tomb += source.ice_tomb;
    target.infernal_abyss_of_demons += source.infernal_abyss_of_demons;
    target.ivory_wyvern_portal += source.ivory_wyvern_portal;
    target.katalund += source.katalund;
    target.kogbold_steamworks += source.kogbold_steamworks;
    target.lair_of_draconis += source.lair_of_draconis;
    target.lair_of_shaitan += source.lair_of_shaitan;
    target.legacy_abyss_of_demons += source.legacy_abyss_of_demons;
    target.legacy_forest_maze += source.legacy_forest_maze;
    target.legacy_heroic_abyss_of_demons += source.legacy_heroic_abyss_of_demons;
    target.legacy_heroic_undead_lair += source.legacy_heroic_undead_lair;
    target.legacy_lair_of_shaitan += source.legacy_lair_of_shaitan;
    target.legacy_pirate_cave += source.legacy_pirate_cave;
    target.legacy_spider_den += source.legacy_spider_den;
    target.legacy_sprite_world += source.legacy_sprite_world;
    target.legacy_the_crawling_depths += source.legacy_the_crawling_depths;
    target.legacy_the_shatters += source.legacy_the_shatters;
    target.legacy_undead_lair += source.legacy_undead_lair;
    target.legacy_woodland_labyrinth += source.legacy_woodland_labyrinth;
    target.lost_halls += source.lost_halls;
    target.mad_lab += source.mad_lab;
    target.magic_woods += source.magic_woods;
    target.malogia += source.malogia;
    target.manor_of_the_immortals += source.manor_of_the_immortals;
    target.moonlight_village += source.moonlight_village;
    target.mountain_temple += source.mountain_temple;
    target.ocean_trench += source.ocean_trench;
    target.mad_god_mayhem += source.mad_god_mayhem;
    target.neo_forax += source.neo_forax;
    target.neo_katalund += source.neo_katalund;
    target.neo_malogia += source.neo_malogia;
    target.neo_untaris += source.neo_untaris;
    target.oryxs_castle += source.oryxs_castle;
    target.oryxs_chamber += source.oryxs_chamber;
    target.oryxs_sanctuary += source.oryxs_sanctuary;
    target.parasite_chambers += source.parasite_chambers;
    target.pirate_cave += source.pirate_cave;
    target.plagued_nest += source.plagued_nest;
    target.puppet_masters_encore += source.puppet_masters_encore;
    target.puppet_masters_theatre += source.puppet_masters_theatre;
    target.queen_bunny_chamber += source.queen_bunny_chamber;
    target.rainbow_road += source.rainbow_road;
    target.santas_workshop += source.santas_workshop;
    target.secluded_thicket += source.secluded_thicket;
    target.snake_pit += source.snake_pit;
    target.spider_den += source.spider_den;
    target.spectral_penitentiary += source.spectral_penitentiary;
    target.sprite_world += source.sprite_world;
    target.sulfurous_wetlands += source.sulfurous_wetlands;
    target.the_crawling_depths += source.the_crawling_depths;
    target.the_hive += source.the_hive;
    target.the_machine += source.the_machine;
    target.the_nest += source.the_nest;
    target.the_shatters += source.the_shatters;
    target.the_tavern += source.the_tavern;
    target.the_third_dimension += source.the_third_dimension;
    target.the_trials_of_cronus += source.the_trials_of_cronus;
    target.the_void += source.the_void;
    target.tomb_of_the_ancients += source.tomb_of_the_ancients;
    target.toxic_sewers += source.toxic_sewers;
    target.undead_lair += source.undead_lair;
    target.undead_lair_heroic += source.undead_lair_heroic;
    target.untaris += source.untaris;
    target.white_snake_invasion_i += source.white_snake_invasion_i;
    target.white_snake_invasion_ii += source.white_snake_invasion_ii;
    target.white_snake_invasion_iii += source.white_snake_invasion_iii;
    target.wine_cellar += source.wine_cellar;
    target.woodland_labyrinth += source.woodland_labyrinth;
}

/// Rarity rank for sorting: Shiny UT (4) > UT (3) > ST (2) > Tiered (1) > rest (0).
fn snapshot_item_rarity_rank(item_id: i32, asset_mgr: &AssetManager) -> u8 {
    let Some(obj) = asset_mgr.get_object(item_id) else {
        return 0;
    };
    if obj.is_shiny() && obj.is_ut() {
        4
    } else if obj.is_ut() {
        3
    } else if obj.is_st() {
        2
    } else if obj.is_tiered() {
        1
    } else {
        0
    }
}

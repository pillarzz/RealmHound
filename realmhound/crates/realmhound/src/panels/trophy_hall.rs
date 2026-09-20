//! Trophy Hall panel - unified item-collection tracker across all dungeons.
//!
//! An item is "Unlocked" if it has been recorded by RealmHound Loot History,
//! found in an imported RealmShark log, or is currently stored somewhere on
//! the account (character, vault, gift chest, etc). Otherwise it's shown
//! darkened as "Not Unlocked".
use crate::ui_ext::HoverTooltipExt;
use eframe::egui::{self, Color32, RichText, ScrollArea};
use realmhound_core::{
    assets::{dungeon_difficulty, get_asset_manager, get_dungeon_portal_map},
    loot::{DungeonItemStat, LootDatabase, MobItemStat},
    realmshark_import::{self, RealmSharkStats},
    stats::{
        collections::ALL_DUNGEONS,
        dungeon_collection::{
            build_collection_for_dungeon, CollectionItem, DungeonCollectionDef, ItemGroup,
        },
        dungeon_snapshot::build_dungeon_snapshot,
    },
    vault::AccountData,
};
use std::collections::{HashMap, HashSet};

use crate::panels::missions::attach_dungeon_tooltip;
use crate::panels::{AppAction, Panel, PanelContext};
use crate::rendering::rarity::{has_no_enchant_slots, rarity_name, restricted_rarities};
use crate::rendering::SpriteRenderer;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    List,
    Icon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    AlphaAsc,
    DifficultyAsc,
    DifficultyDesc,
}

/// Which tracked-loot source(s) feed drop counts, the detail page breakdown,
/// and the tracked-loot half of the Unlocked rule. Account ownership always
/// counts toward Unlocked regardless of this selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSource {
    RealmHound,
    RealmShark,
    Both,
}

impl DataSource {
    /// Stable string key for persistence in settings.
    pub fn as_key(self) -> &'static str {
        match self {
            DataSource::RealmHound => "realmhound",
            DataSource::RealmShark => "realmshark",
            DataSource::Both => "both",
        }
    }

    /// Parse a persisted key; unknown values fall back to `None` (default source).
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "realmhound" => Some(DataSource::RealmHound),
            "realmshark" => Some(DataSource::RealmShark),
            "both" => Some(DataSource::Both),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
enum Page {
    Index,
    Detail { name: String },
}

/// A row in the index grid.
#[derive(Debug, Clone)]
struct IndexRow {
    name: String,
    /// Raw dungeon names from loot DB that resolved to this display name.
    /// Used for detail queries (DB stores raw names, not normalized).
    db_names: Vec<String>,
    total_items: u64,
    completions: i32,
    difficulty: Option<f32>,
    /// Total time spent in this dungeon (milliseconds), merged from the
    /// RealmHound per-dungeon counter and/or RealmShark import per the Data
    /// Source selector. 0 when unknown.
    time_ms: u64,
}

/// Cached detail data for the currently viewed dungeon.
#[derive(Debug, Clone)]
struct DetailCache {
    items: Vec<DungeonItemStat>,
    mob_items: Vec<MobItemStat>,
    /// Precomputed: which items can have enchantments (WEAPON/ABILITY/ARMOR/RING label)
    enchantable: HashSet<i32>,
    /// Precomputed rarity breakdowns: item_id -> [(enchant_count, drop_count)]
    rarity_breakdowns: std::collections::HashMap<i32, Vec<(i32, i64)>>,
}

// ---------------------------------------------------------------------------
// Panel
// ---------------------------------------------------------------------------

pub struct TrophyHallPanel {
    view_mode: ViewMode,
    sort: SortOrder,
    data_source: DataSource,
    page: Page,
    index_rows: Vec<IndexRow>,
    search_query: String,
    // Cache keys
    cached_drop_id: Option<i64>,
    cached_account_gen: u64,
    cached_data_source: Option<DataSource>,
    /// Signature (sum) of the combat DB per-dungeon time counters at last
    /// rebuild; a change means newly flushed run time to fold in.
    cached_dungeon_time_sig: Option<i64>,
    /// Set when a control changes that isn't covered by the cache keys above,
    /// forcing a rebuild on next refresh.
    force_rebuild: bool,
    column_count: u8,
    /// Show dungeons whose collection was intentionally emptied (duplicate
    /// drop tables, e.g. White Snake Invasion I/II). Off by default since
    /// these rows have no items to show, just completions.
    show_no_collection_dungeons: bool,
    show_st: bool,
    show_ut: bool,
    show_shiny: bool,
    compact_view: bool,
    // Detail cache (tracked-loot breakdown on the detail page)
    detail_cache: Option<DetailCache>,
    // RealmShark import
    realmshark_data: Option<RealmSharkStats>,
    realmshark_load_attempted: bool,
    /// Explicit per-account RealmShark import path. `None` when no account is
    /// selected, which disables import, autoload, and RealmShark selection.
    realmshark_import_path: Option<std::path::PathBuf>,
    // Item collections
    collections: HashMap<String, DungeonCollectionDef>,
    obtained_items: HashSet<i32>,
    /// item_id -> set of enchant slot-counts (0-4) ever observed, merging loot
    /// history (gated by data_source) and current account ownership (always on).
    rarity_seen: HashMap<i32, HashSet<u8>>,
    /// Expanded sectioned dungeons (O3, Machine)
    expanded_collections: HashSet<String>,
    /// Whether we've applied the "expand all" default on first collection
    /// build. Only happens once so the user's manual collapses stick.
    expand_all_initialized: bool,
}

impl TrophyHallPanel {
    pub fn new() -> Self {
        Self {
            view_mode: ViewMode::List,
            sort: SortOrder::DifficultyDesc,
            data_source: DataSource::RealmHound,
            page: Page::Index,
            index_rows: Vec::new(),
            search_query: String::new(),
            cached_drop_id: None,
            cached_account_gen: 0,
            cached_data_source: None,
            cached_dungeon_time_sig: None,
            force_rebuild: false,
            column_count: 2,
            show_no_collection_dungeons: false,
            show_st: true,
            show_ut: true,
            show_shiny: true,
            compact_view: false,
            detail_cache: None,
            realmshark_data: None,
            realmshark_load_attempted: false,
            realmshark_import_path: None,
            collections: HashMap::new(),
            obtained_items: HashSet::new(),
            rarity_seen: HashMap::new(),
            expanded_collections: HashSet::new(),
            expand_all_initialized: false,
        }
    }

    // ------ Settings-menu accessors (Settings > Trophy Hall) ------

    pub fn data_source(&self) -> DataSource {
        self.data_source
    }

    pub fn set_data_source(&mut self, source: DataSource) {
        if self.data_source != source {
            self.data_source = source;
            self.detail_cache = None;
        }
    }

    pub fn realmshark_loaded(&self) -> bool {
        self.realmshark_data.is_some()
    }

    /// Bind the per-account RealmShark import path. `None` disables import,
    /// autoload, and RealmShark data-source selection (no account selected).
    pub fn set_realmshark_import_path(&mut self, path: Option<std::path::PathBuf>) {
        self.realmshark_import_path = path;
    }

    /// Whether a per-account RealmShark import path is bound (account selected).
    pub fn has_realmshark_path(&self) -> bool {
        self.realmshark_import_path.is_some()
    }

    pub fn compact_view_mut(&mut self) -> &mut bool {
        &mut self.compact_view
    }

    pub fn compact_view(&self) -> bool {
        self.compact_view
    }

    pub fn show_no_collection_dungeons_mut(&mut self) -> &mut bool {
        &mut self.show_no_collection_dungeons
    }

    pub fn show_no_collection_dungeons(&self) -> bool {
        self.show_no_collection_dungeons
    }

    /// Open a file picker for a RealmShark `dungeon.stats` file and import it to
    /// the selected account's import path. No-op when no account is selected.
    /// Switches the data source to `Both` on success, same as the old inline button.
    pub fn import_realmshark(&mut self) {
        let Some(dest) = self.realmshark_import_path.clone() else {
            tracing::warn!("[TROPHY] RealmShark import requires a selected, verified account");
            return;
        };
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("RealmShark Stats", &["stats"])
            .set_title("Select dungeon.stats file")
            .pick_file()
        {
            match realmshark_import::parse_dungeon_stats(&path) {
                Ok(stats) => {
                    let _ = realmshark_import::copy_to_import_dir(&path, &dest);
                    self.realmshark_data = Some(stats);
                    self.data_source = DataSource::Both;
                    self.detail_cache = None;
                    self.cached_data_source = None;
                }
                Err(e) => {
                    tracing::warn!("Failed to import RealmShark stats: {}", e);
                }
            }
        }
    }

    fn refresh_if_stale(
        &mut self,
        loot_db: Option<&LootDatabase>,
        account_data: &AccountData,
        combat_db: Option<&realmhound_core::combat::CombatDatabase>,
        account_verified: bool,
    ) {
        // Auto-load RealmShark data from the selected account's import path, only
        // once the account is selected and currently verified.
        if !self.realmshark_load_attempted {
            if let Some(path) = self.realmshark_import_path.clone() {
                if account_verified {
                    self.realmshark_load_attempted = true;
                    if path.exists() {
                        self.realmshark_data = realmshark_import::parse_dungeon_stats(&path).ok();
                    }
                }
            }
        }

        let current_drop_id = loot_db.and_then(|db| db.last_drop_id().ok().flatten());
        let account_gen = account_data.generation;
        let dungeon_time_sig = combat_db.and_then(|db| db.dungeon_time_signature().ok());

        let stale = self.cached_drop_id != current_drop_id
            || self.cached_account_gen != account_gen
            || self.cached_data_source != Some(self.data_source)
            || self.cached_dungeon_time_sig != dungeon_time_sig
            || self.force_rebuild;

        if !stale {
            return;
        }

        self.detail_cache = None;

        self.rebuild_index(loot_db, account_data, combat_db);

        self.cached_drop_id = current_drop_id;
        self.cached_account_gen = account_gen;
        self.cached_data_source = Some(self.data_source);
        self.cached_dungeon_time_sig = dungeon_time_sig;
        self.force_rebuild = false;

        // Rebuild collections, obtained items, and rarity-seen map
        self.rebuild_collections(loot_db, account_data);
    }

    fn rebuild_collections(&mut self, loot_db: Option<&LootDatabase>, account_data: &AccountData) {
        // Build collection definitions for any dungeon not yet cached
        for row in &self.index_rows {
            if !self.collections.contains_key(&row.name) {
                let col = build_collection_for_dungeon(&row.name);
                self.collections.insert(row.name.clone(), col);
            }
        }

        // Default to "Expand All" the first time collections are available,
        // so the page opens fully expanded instead of requiring a manual click.
        if !self.expand_all_initialized && !self.collections.is_empty() {
            self.expand_all_initialized = true;
            for (name, col) in &self.collections {
                if col.is_sectioned() || col.items.len() > MAX_INLINE_COLLECTION_ITEMS {
                    self.expanded_collections.insert(name.clone());
                }
            }
        }

        let include_realmhound =
            matches!(self.data_source, DataSource::RealmHound | DataSource::Both);
        let include_realmshark =
            matches!(self.data_source, DataSource::RealmShark | DataSource::Both);

        // Unlocked = tracked loot (per Data Source selector) OR currently on account (always on).
        self.obtained_items.clear();
        if include_realmhound {
            if let Some(db) = loot_db {
                if let Ok(items) = db.get_distinct_items() {
                    self.obtained_items.extend(items.iter().map(|(id, _)| *id));
                }
            }
        }
        if include_realmshark {
            if let Some(rs) = &self.realmshark_data {
                for dungeon in rs.dungeons.values() {
                    for item in &dungeon.items {
                        self.obtained_items.insert(item.item_id);
                    }
                }
            }
        }
        Self::collect_owned_item_ids(account_data, &mut self.obtained_items);

        // Rarity-seen: which enchant slot-counts have ever been observed per item,
        // merging loot-history rarity breakdowns (RealmHound only, gated by selector)
        // with currently-owned instances' enchant counts (always on).
        self.rarity_seen.clear();
        if include_realmhound {
            if let Some(db) = loot_db {
                let all_db_names: Vec<String> = self
                    .index_rows
                    .iter()
                    .flat_map(|r| r.db_names.clone())
                    .collect();
                if let Ok(breakdowns) = db.all_rarity_breakdowns(&all_db_names) {
                    for (item_id, counts) in breakdowns {
                        let entry = self.rarity_seen.entry(item_id).or_default();
                        for (slots, _count) in counts {
                            entry.insert(slots.clamp(0, 4) as u8);
                        }
                    }
                }
            }
        }
        Self::collect_owned_item_rarities(account_data, &mut self.rarity_seen);
    }

    fn collect_owned_item_ids(account_data: &AccountData, out: &mut HashSet<i32>) {
        // Living characters + graveyard: a character's items remain part of the
        // account's history after death.
        for ch in account_data
            .characters
            .characters
            .iter()
            .chain(account_data.characters.get_dead_characters())
        {
            let all_items = ch
                .equipment
                .iter()
                .chain(ch.inventory.iter())
                .chain(ch.backpack.iter())
                .chain(ch.backpack_ext.iter())
                .chain(ch.belt.iter());
            for item in all_items {
                if item.item_id > 0 {
                    out.insert(item.item_id);
                }
            }
        }
        for vault in [&account_data.regular_vault, &account_data.seasonal_vault] {
            let all_items = vault
                .vault_items
                .iter()
                .chain(vault.material_items.iter())
                .chain(vault.gift_items.iter())
                .chain(vault.potion_items.iter())
                .chain(vault.spoils_items.iter());
            for item in all_items {
                if item.item_id > 0 {
                    out.insert(item.item_id);
                }
            }
        }
    }

    /// item_id -> set of enchant slot-counts observed on any currently-owned instance
    /// (living + graveyard characters, both vaults).
    fn collect_owned_item_rarities(
        account_data: &AccountData,
        out: &mut HashMap<i32, HashSet<u8>>,
    ) {
        fn add(item_id: i32, enchant_count: usize, out: &mut HashMap<i32, HashSet<u8>>) {
            if item_id > 0 {
                out.entry(item_id)
                    .or_default()
                    .insert(enchant_count.min(4) as u8);
            }
        }

        for ch in account_data
            .characters
            .characters
            .iter()
            .chain(account_data.characters.get_dead_characters())
        {
            let all_items = ch
                .equipment
                .iter()
                .chain(ch.inventory.iter())
                .chain(ch.backpack.iter())
                .chain(ch.backpack_ext.iter())
                .chain(ch.belt.iter());
            for item in all_items {
                add(item.item_id, item.enchant_count(), out);
            }
        }
        for vault in [&account_data.regular_vault, &account_data.seasonal_vault] {
            let all_items = vault
                .vault_items
                .iter()
                .chain(vault.material_items.iter())
                .chain(vault.gift_items.iter())
                .chain(vault.potion_items.iter())
                .chain(vault.spoils_items.iter());
            for item in all_items {
                add(item.item_id, item.enchant_count(), out);
            }
        }
    }

    fn rebuild_index(
        &mut self,
        loot_db: Option<&LootDatabase>,
        account_data: &AccountData,
        combat_db: Option<&realmhound_core::combat::CombatDatabase>,
    ) {
        let asset_mgr = get_asset_manager();
        let portal_map = get_dungeon_portal_map();
        self.index_rows.clear();

        let include_realmhound =
            matches!(self.data_source, DataSource::RealmHound | DataSource::Both);
        let include_realmshark =
            matches!(self.data_source, DataSource::RealmShark | DataSource::Both);

        let mut merged: std::collections::HashMap<String, IndexRow> =
            std::collections::HashMap::new();

        // RealmHound DB drop counts
        if include_realmhound {
            if let Some(db) = loot_db {
                let dungeon_counts = db.count_drops_by_dungeon().unwrap_or_default();

                for (raw_name, count) in &dungeon_counts {
                    if raw_name.is_empty() || is_non_dungeon_location(raw_name) {
                        continue;
                    }
                    let display = portal_map.normalize_dungeon_name(raw_name);
                    let registry = realmhound_core::stats::dungeon_registry::get_dungeon_registry();
                    let canonical = registry.normalize(&display);
                    let entry = merged.entry(canonical.clone()).or_insert_with(|| IndexRow {
                        name: canonical.clone(),
                        db_names: Vec::new(),
                        total_items: 0,
                        completions: 0,
                        difficulty: dungeon_difficulty(&canonical),
                        time_ms: 0,
                    });
                    entry.total_items += *count as u64;
                    entry.db_names.push(raw_name.clone());
                }
            }
        }

        // RealmShark imported drop counts
        if include_realmshark {
            if let Some(rs) = &self.realmshark_data {
                for (canonical, dungeon) in &rs.dungeons {
                    let entry = merged.entry(canonical.clone()).or_insert_with(|| IndexRow {
                        name: canonical.clone(),
                        db_names: Vec::new(),
                        total_items: 0,
                        completions: 0,
                        difficulty: dungeon_difficulty(canonical),
                        time_ms: 0,
                    });
                    let item_total: i64 = dungeon.items.iter().map(|i| i.total_count).sum();
                    entry.total_items += item_total as u64;
                    entry.completions += dungeon.completions;
                }
            }
        }

        // Completions from the account snapshot PCStats (summed across living +
        // graveyard characters) when RealmHound is a selected source; RealmShark
        // completions were already added above. Difficulty always comes from the
        // snapshot regardless of source.
        let snapshot_rows = build_dungeon_snapshot(account_data, &asset_mgr);
        for snap in &snapshot_rows {
            let entry = merged
                .entry(snap.dungeon_name.clone())
                .or_insert_with(|| IndexRow {
                    name: snap.dungeon_name.clone(),
                    db_names: Vec::new(),
                    total_items: 0,
                    completions: 0,
                    difficulty: snap.difficulty,
                    time_ms: 0,
                });
            if include_realmhound {
                entry.completions += snap.total_completions;
            }
            if entry.difficulty.is_none() {
                entry.difficulty = snap.difficulty;
            }
        }

        self.index_rows = merged.into_values().collect();

        // Merge "Remnant of the Void" into "The Void" - PCStats already counts
        // completions for both under a single "The Void" stat, so keeping them
        // as separate rows would be misleading.
        Self::merge_remnant_into_void(&mut self.index_rows);

        let seen: HashSet<String> = self.index_rows.iter().map(|r| r.name.clone()).collect();
        for d in ALL_DUNGEONS.iter() {
            if seen.contains(d.name) {
                continue;
            }
            if d.name == "Remnant of the Void" {
                continue;
            }
            self.index_rows.push(IndexRow {
                name: d.name.to_string(),
                db_names: Vec::new(),
                total_items: 0,
                completions: 0,
                difficulty: dungeon_difficulty(d.name),
                time_ms: 0,
            });
        }

        // "Realm" (Biome Whites / Event Whites / etc.) is a permanent category
        // that should always be visible, even with zero tracked drops or owned
        // items - it has no PCStats completion field so it's not in ALL_DUNGEONS.
        if !seen.contains("Realm") {
            self.index_rows.push(IndexRow {
                name: "Realm".to_string(),
                db_names: Vec::new(),
                total_items: 0,
                completions: 0,
                difficulty: dungeon_difficulty("Realm"),
                time_ms: 0,
            });
        }

        // Time spent per dungeon: RealmHound counter and/or RealmShark import,
        // keyed to the same canonical row names used above.
        let mut time_by_dungeon: std::collections::HashMap<String, u64> =
            std::collections::HashMap::new();
        let registry = realmhound_core::stats::dungeon_registry::get_dungeon_registry();
        if include_realmhound {
            if let Some(db) = combat_db {
                if let Ok(totals) = db.dungeon_time_totals() {
                    for t in totals {
                        if t.total_time_ms <= 0 {
                            continue;
                        }
                        let display = portal_map.normalize_dungeon_name(&t.dungeon);
                        let canonical = registry.normalize(&display);
                        *time_by_dungeon.entry(canonical).or_insert(0) += t.total_time_ms as u64;
                    }
                }
            }
        }
        if include_realmshark {
            if let Some(rs) = &self.realmshark_data {
                for (canonical, dungeon) in &rs.dungeons {
                    if dungeon.total_time_ms > 0 {
                        *time_by_dungeon.entry(canonical.clone()).or_insert(0) +=
                            dungeon.total_time_ms as u64;
                    }
                }
            }
        }
        if !time_by_dungeon.is_empty() {
            for row in &mut self.index_rows {
                if let Some(ms) = time_by_dungeon.get(&row.name) {
                    row.time_ms = *ms;
                }
            }
        }

        self.sort_index();
    }

    fn merge_remnant_into_void(rows: &mut Vec<IndexRow>) {
        let remnant_idx = rows.iter().position(|r| r.name == "Remnant of the Void");
        let void_idx = rows.iter().position(|r| r.name == "The Void");
        match (remnant_idx, void_idx) {
            (Some(ri), Some(vi)) => {
                let remnant = rows.remove(ri);
                let vi = if ri < vi { vi - 1 } else { vi };
                rows[vi].total_items += remnant.total_items;
                rows[vi].completions += remnant.completions;
                rows[vi].db_names.extend(remnant.db_names);
            }
            (Some(ri), None) => {
                // Rename Remnant to The Void
                rows[ri].name = "The Void".to_string();
                rows[ri].difficulty = dungeon_difficulty("The Void");
            }
            _ => {}
        }
    }

    fn sort_index(&mut self) {
        match self.sort {
            SortOrder::DifficultyDesc => {
                self.index_rows.sort_by(|a, b| {
                    let da = a.difficulty.unwrap_or(-1.0);
                    let db = b.difficulty.unwrap_or(-1.0);
                    db.partial_cmp(&da)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| a.name.cmp(&b.name))
                });
            }
            SortOrder::DifficultyAsc => {
                self.index_rows.sort_by(|a, b| {
                    let da = a.difficulty.unwrap_or(f32::MAX);
                    let db = b.difficulty.unwrap_or(f32::MAX);
                    da.partial_cmp(&db)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| a.name.cmp(&b.name))
                });
            }
            SortOrder::AlphaAsc => {
                self.index_rows.sort_by(|a, b| a.name.cmp(&b.name));
            }
        }
    }

    fn load_detail(
        &mut self,
        db_names: &[String],
        loot_db: Option<&LootDatabase>,
        dungeon_name: &str,
    ) {
        let include_realmhound =
            matches!(self.data_source, DataSource::RealmHound | DataSource::Both);
        let include_realmshark =
            matches!(self.data_source, DataSource::RealmShark | DataSource::Both);

        let mut all_items = Vec::new();
        let mut all_mob_items = Vec::new();

        // RealmHound DB items
        if include_realmhound {
            if let Some(db) = loot_db {
                for raw_name in db_names {
                    for item in db.items_by_dungeon(raw_name).unwrap_or_default() {
                        if let Some(existing) = all_items
                            .iter_mut()
                            .find(|i: &&mut DungeonItemStat| i.item_id == item.item_id)
                        {
                            existing.total_count += item.total_count;
                        } else {
                            all_items.push(item);
                        }
                    }
                    for mob_item in db.items_by_dungeon_and_mob(raw_name).unwrap_or_default() {
                        if let Some(existing) =
                            all_mob_items.iter_mut().find(|i: &&mut MobItemStat| {
                                i.mob_type == mob_item.mob_type && i.item_id == mob_item.item_id
                            })
                        {
                            existing.count += mob_item.count;
                        } else {
                            all_mob_items.push(mob_item);
                        }
                    }
                }
            }
        }

        // RealmShark items
        if include_realmshark {
            if let Some(rs) = &self.realmshark_data {
                if let Some(dungeon) = rs.dungeons.get(dungeon_name) {
                    for item in &dungeon.items {
                        if let Some(existing) =
                            all_items.iter_mut().find(|i| i.item_id == item.item_id)
                        {
                            existing.total_count += item.total_count;
                        } else {
                            all_items.push(item.clone());
                        }
                    }
                    for mob_item in &dungeon.mob_items {
                        if let Some(existing) = all_mob_items.iter_mut().find(|i| {
                            i.mob_type == mob_item.mob_type && i.item_id == mob_item.item_id
                        }) {
                            existing.count += mob_item.count;
                        } else {
                            all_mob_items.push(mob_item.clone());
                        }
                    }
                }
            }
        }

        let asset_mgr = get_asset_manager();
        all_items.sort_by(|a, b| {
            let a_rank = item_rarity_rank(a.item_id, asset_mgr);
            let b_rank = item_rarity_rank(b.item_id, asset_mgr);
            b_rank.cmp(&a_rank).then_with(|| {
                let a_obj = asset_mgr.get_object(a.item_id);
                let b_obj = asset_mgr.get_object(b.item_id);
                let a_fame = a_obj.as_ref().map(|o| o.fame_bonus).unwrap_or(0);
                let b_fame = b_obj.as_ref().map(|o| o.fame_bonus).unwrap_or(0);
                b_fame.cmp(&a_fame).then_with(|| {
                    let a_feed = a_obj.as_ref().map(|o| o.feed_power).unwrap_or(0);
                    let b_feed = b_obj.as_ref().map(|o| o.feed_power).unwrap_or(0);
                    b_feed.cmp(&a_feed)
                })
            })
        });
        all_mob_items.sort_by(|a, b| {
            a.mob_name.cmp(&b.mob_name).then_with(|| {
                let a_rank = item_rarity_rank(a.item_id, asset_mgr);
                let b_rank = item_rarity_rank(b.item_id, asset_mgr);
                b_rank.cmp(&a_rank).then_with(|| {
                    let a_obj = asset_mgr.get_object(a.item_id);
                    let b_obj = asset_mgr.get_object(b.item_id);
                    let a_fame = a_obj.as_ref().map(|o| o.fame_bonus).unwrap_or(0);
                    let b_fame = b_obj.as_ref().map(|o| o.fame_bonus).unwrap_or(0);
                    b_fame.cmp(&a_fame).then_with(|| {
                        let a_feed = a_obj.as_ref().map(|o| o.feed_power).unwrap_or(0);
                        let b_feed = b_obj.as_ref().map(|o| o.feed_power).unwrap_or(0);
                        b_feed.cmp(&a_feed)
                    })
                })
            })
        });

        // Precompute enchantable set
        let mut enchantable = HashSet::new();
        let all_ids: HashSet<i32> = all_items
            .iter()
            .map(|i| i.item_id)
            .chain(all_mob_items.iter().map(|i| i.item_id))
            .collect();
        for &id in &all_ids {
            if let Some(obj) = asset_mgr.get_object(id) {
                if obj
                    .labels
                    .split(',')
                    .any(|l| matches!(l, "WEAPON" | "ABILITY" | "ARMOR" | "RING"))
                {
                    enchantable.insert(id);
                }
            }
        }

        // Rarity breakdowns only from DB (RealmShark has no enchant data)
        let rarity_breakdowns = if include_realmhound {
            loot_db
                .map(|db| db.all_rarity_breakdowns(db_names).unwrap_or_default())
                .unwrap_or_default()
        } else {
            std::collections::HashMap::new()
        };

        self.detail_cache = Some(DetailCache {
            items: all_items,
            mob_items: all_mob_items,
            enchantable,
            rarity_breakdowns,
        });
    }

    // ------ Rendering ------

    fn render_index(&mut self, ui: &mut egui::Ui, ctx: &mut PanelContext) -> Vec<AppAction> {
        realmhound_core::prof_function!();
        let actions = Vec::new();
        let shadcn = ctx.shadcn;

        // Controls row: view mode + sort dropdown + search + data source
        shadcn.header_band(ui, shadcn.secondary_header_fill(), |ui| {
            shadcn.band_row(ui, |ui| {
                ui.label("View:");
                egui::ComboBox::from_id_salt("dungeon_view_mode")
                    .selected_text(match self.view_mode {
                        ViewMode::List => "☰ List",
                        ViewMode::Icon => "⊞ Icon",
                    })
                    .width(70.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.view_mode, ViewMode::List, "☰ List");
                        ui.selectable_value(&mut self.view_mode, ViewMode::Icon, "⊞ Icon");
                    });
                ui.separator();
                ui.label("Sort:");
                egui::ComboBox::from_id_salt("dungeon_sort")
                    .selected_text(match self.sort {
                        SortOrder::AlphaAsc => "A-Z",
                        SortOrder::DifficultyAsc => "Difficulty ↑",
                        SortOrder::DifficultyDesc => "Difficulty ↓",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.sort, SortOrder::AlphaAsc, "A-Z");
                        ui.selectable_value(
                            &mut self.sort,
                            SortOrder::DifficultyAsc,
                            "Difficulty ↑",
                        );
                        ui.selectable_value(
                            &mut self.sort,
                            SortOrder::DifficultyDesc,
                            "Difficulty ↓",
                        );
                    });

                ui.separator();
                ui.label("🔍");
                ui.add(
                    egui::TextEdit::singleline(&mut self.search_query)
                        .desired_width(120.0)
                        .hint_text("Search..."),
                );

                ui.separator();
                if self.view_mode == ViewMode::List {
                    ui.label("Columns:");
                    egui::ComboBox::from_id_salt("dungeon_column_count")
                        .selected_text(format!("{}", self.column_count))
                        .width(40.0)
                        .show_ui(ui, |ui| {
                            for n in 1..=3u8 {
                                ui.selectable_value(&mut self.column_count, n, format!("{n}"));
                            }
                        });

                    let collapsible_names: Vec<&String> = self
                        .collections
                        .iter()
                        .filter(|(_, c)| {
                            c.is_sectioned() || c.items.len() > MAX_INLINE_COLLECTION_ITEMS
                        })
                        .map(|(name, _)| name)
                        .collect();
                    let all_expanded = !collapsible_names.is_empty()
                        && collapsible_names
                            .iter()
                            .all(|name| self.expanded_collections.contains(*name));

                    let label = if all_expanded {
                        "⏶ Collapse All"
                    } else {
                        "⏷ Expand All"
                    };
                    let hover = if all_expanded {
                        "Fold every collapsible collection"
                    } else {
                        "Unfold every collapsible collection"
                    };
                    if ui.button(label).hover_tip(hover).clicked() {
                        if all_expanded {
                            self.expanded_collections.clear();
                        } else {
                            for name in collapsible_names {
                                self.expanded_collections.insert(name.clone());
                            }
                        }
                    }
                }
                ui.separator();
                ui.label("Show:");
                shadcn
                    .tgl(ui, &mut self.show_ut, RichText::new("UT").small())
                    .hover_tip("Show Untiered (UT) items");
                shadcn
                    .tgl(ui, &mut self.show_st, RichText::new("ST").small())
                    .hover_tip("Show Set Tier (ST) items");
                ctx.sprite_renderer.shiny_toggle(
                    ui,
                    shadcn,
                    &mut self.show_shiny,
                    "Show Shiny items",
                );
            });
        });

        {
            realmhound_core::prof_scope!("dstats_sort");
            self.sort_index();
        }

        if self.index_rows.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(
                    "No dungeon data available yet.\nCapture loot drops or load account data.",
                );
            });
            return actions;
        }

        match self.view_mode {
            ViewMode::List => self.render_list_view(ui, ctx),
            ViewMode::Icon => self.render_icon_view(ui, ctx),
        }

        actions
    }

    /// Estimated relative row-height weight (in wrapped-line units) for one
    /// index row in the list view, given how many icons fit per line at the
    /// current column width — matches `render_list_column`/
    /// `draw_collection_items_row` so balancing accounts for wrapped rows.
    fn estimated_row_weight(&self, row: &IndexRow, per_line: usize) -> usize {
        let Some(collection) = self.collections.get(&row.name) else {
            return 1;
        };
        let lines_for = |count: usize| -> usize {
            if count == 0 {
                return 1;
            }
            count.div_ceil(per_line).clamp(1, Self::MAX_ITEM_ROW_LINES)
        };
        let total = collection.items.len();
        let visible_total = self.visible_items(&collection.items).len();
        let collapsible = collection.is_sectioned() || total > MAX_INLINE_COLLECTION_ITEMS;
        if collapsible {
            if self.expanded_collections.contains(&row.name) {
                if let Some(sections) = &collection.sections {
                    1 + sections
                        .iter()
                        .map(|s| lines_for(self.visible_items(&s.items).len()))
                        .sum::<usize>()
                } else {
                    1 + lines_for(visible_total)
                }
            } else {
                1
            }
        } else {
            lines_for(visible_total)
        }
    }

    /// Rough number of icon slots ("Collection" column) that fit per line
    /// given the outer index-list column width, after subtracting the space
    /// used by the icon/name/comp/gravestone/% columns that precede it.
    /// Deliberately generous (covers the longest dungeon names, e.g. "Cave of
    /// a Thousand Treasures") so this slightly under-estimates rather than
    /// over-estimates available space — wrapping a little early is much less
    /// visually broken than silently overflowing past the column edge.
    const ICON_ROW_OVERHEAD: f32 = 370.0;
    fn icon_slots_per_line(col_width: f32) -> usize {
        let available = (col_width - Self::ICON_ROW_OVERHEAD).max(16.0);
        ((available / 17.0).floor() as usize).max(1)
    }

    /// Picks `num_columns - 1` split points for `indices` (kept in original
    /// order across all columns) that best balance estimated total
    /// row-height across the columns, so expanding a collapsible collection
    /// in one column doesn't leave it far taller than the others.
    fn balanced_split_points(
        &self,
        indices: &[usize],
        num_columns: usize,
        per_line: usize,
    ) -> Vec<usize> {
        let n = indices.len();
        let num_columns = num_columns.min(n.max(1)).max(1);
        if num_columns <= 1 {
            return Vec::new();
        }

        let weights: Vec<usize> = indices
            .iter()
            .map(|&i| self.estimated_row_weight(&self.index_rows[i], per_line))
            .collect();
        let mut prefix: Vec<usize> = Vec::with_capacity(n + 1);
        prefix.push(0);
        for &w in &weights {
            prefix.push(prefix.last().unwrap() + w);
        }
        let total = prefix[n];

        let mut splits: Vec<usize> = Vec::with_capacity(num_columns - 1);
        for col in 1..num_columns {
            let target = total as f32 * col as f32 / num_columns as f32;
            let prev = *splits.last().unwrap_or(&0);
            let lo = prev.max(col);
            let hi = n - (num_columns - col);
            let hi = hi.max(lo);

            let mut best = lo;
            let mut best_diff = f32::MAX;
            for i in lo..=hi {
                let diff = (prefix[i] as f32 - target).abs();
                if diff < best_diff {
                    best_diff = diff;
                    best = i;
                }
            }
            splits.push(best);
        }
        splits
    }

    fn render_list_view(&mut self, ui: &mut egui::Ui, ctx: &mut PanelContext) {
        let portal_map = get_dungeon_portal_map();
        let query_lower = self.search_query.to_lowercase();
        let show_no_collection = self.show_no_collection_dungeons;
        let collections = &self.collections;
        let filtered_indices: Vec<usize> = {
            realmhound_core::prof_scope!("dstats_filter");
            self.index_rows
                .iter()
                .enumerate()
                .filter(|(_, r)| {
                    query_lower.is_empty() || r.name.to_lowercase().contains(&query_lower)
                })
                .filter(|(_, r)| {
                    show_no_collection
                        || collections
                            .get(&r.name)
                            .map(|c| !c.items.is_empty())
                            .unwrap_or(true)
                })
                .map(|(i, _)| i)
                .collect()
        };

        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            realmhound_core::prof_scope!("dstats_index_scroll_body");
            let mut navigate_to: Option<(String, Vec<String>)> = None;

            let num_columns = (self.column_count as usize).clamp(1, 3);

            if num_columns <= 1 {
                let col_width = ui.available_width();
                self.render_list_column(
                    ui,
                    ctx,
                    &portal_map,
                    &filtered_indices,
                    &mut navigate_to,
                    col_width,
                );
            } else {
                let col_spacing = 3.0;
                // Reserve a right-hand margin so the last column's icon row
                // has breathing room before the true window edge, instead of
                // sitting flush against it.
                let right_margin = 24.0;
                let num_cols_f = num_columns as f32;
                let total_width = (ui.available_width() - right_margin).max(1.0);
                let col_width =
                    ((total_width - col_spacing * 2.0 * (num_cols_f - 1.0)) / num_cols_f).max(1.0);
                let per_line = Self::icon_slots_per_line(col_width);

                let splits = self.balanced_split_points(&filtered_indices, num_columns, per_line);
                let mut bounds = Vec::with_capacity(num_columns + 1);
                bounds.push(0);
                bounds.extend(splits);
                bounds.push(filtered_indices.len());

                // Top-aligned: plain `ui.horizontal` vertically centers children,
                // which misaligns columns whenever their content heights differ
                // (e.g. one column has an expanded collection the others don't).
                ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP), |ui| {
                    // egui inserts its own `item_spacing.x` between every widget
                    // in a horizontal layout (and `Separator` reserves its own
                    // default 6px on top of that) -- none of which was accounted
                    // for in `col_width`'s math above. Left uncontrolled, that
                    // hidden spacing silently eats into the reserved right
                    // margin. Zero it out and give the separator an explicit
                    // width of exactly `col_spacing` so total gap width
                    // between columns matches `col_spacing * 2` precisely.
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for c in 0..(bounds.len() - 1) {
                        if c > 0 {
                            ui.add_space(col_spacing);
                            ui.add(egui::Separator::default().spacing(0.0));
                            ui.add_space(col_spacing);
                        }
                        let slice = &filtered_indices[bounds[c]..bounds[c + 1]];
                        // Reserve this column's exact share of width up front
                        // (rather than `ui.vertical`/`ui.scope`+`set_width`, which
                        // report only the space actually *used* back to the
                        // horizontal layout) so a column whose content is
                        // narrower than its share doesn't let a later column
                        // silently inherit the leftover width and overflow past
                        // the window edge.
                        ui.allocate_ui_with_layout(
                            egui::vec2(col_width, ui.available_height()),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(col_width);
                                // Hard-clip horizontally to the reserved rect so any
                                // rounding or long "+N" label overflow is cut off
                                // instead of visually bleeding into the next column's
                                // space or the window's right-margin. Only clip the
                                // x-axis here -- intersecting with `ui.max_rect()`
                                // (whose height comes from the desired_size passed to
                                // `allocate_ui_with_layout` above) would bound the
                                // column's vertical clip rect to that fixed height,
                                // which silently caps how tall the ScrollArea thinks
                                // this column's content is and cuts off long expanded
                                // collections even though scrolling further should
                                // reveal them.
                                let mut clip_rect = ui.clip_rect();
                                let col_max_rect = ui.max_rect();
                                clip_rect.min.x = clip_rect.min.x.max(col_max_rect.min.x);
                                clip_rect.max.x = clip_rect.max.x.min(col_max_rect.max.x);
                                ui.set_clip_rect(clip_rect);
                                self.render_list_column(
                                    ui,
                                    ctx,
                                    &portal_map,
                                    slice,
                                    &mut navigate_to,
                                    col_width,
                                );
                            },
                        );
                    }
                });
            }

            // Reserve a little breathing room at the very bottom so the last
            // row's icons (e.g. a multi-line wrapped collection) don't sit
            // flush against -- or get clipped by -- the ScrollArea's measured
            // content edge.
            ui.add_space(12.0);

            if let Some((display_name, db_names)) = navigate_to {
                self.load_detail(&db_names, ctx.loot_database, &display_name);
                self.page = Page::Detail { name: display_name };
            }
        });
    }

    fn render_list_column(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &mut PanelContext,
        portal_map: &realmhound_core::assets::DungeonPortalMap,
        indices: &[usize],
        navigate_to: &mut Option<(String, Vec<String>)>,
        col_width: f32,
    ) {
        const MAX_INLINE: usize = MAX_INLINE_COLLECTION_ITEMS;
        let icon_size = 20.0;
        let header_style = RichText::new("DUNGEONS")
            .strong()
            .size(16.0)
            .color(Color32::from_rgb(150, 200, 255));
        // The true right edge of this column, used to measure exactly how
        // much room the icon row has at draw time (`col_left + col_width`)
        // instead of guessing via a fixed per-column overhead estimate.
        let col_right = ui.cursor().left() + col_width;

        let compact = self.compact_view;
        let num_grid_cols = if compact { 5 } else { 6 };

        egui::Grid::new(format!(
            "dungeon_list_col_{}",
            indices.first().copied().unwrap_or(0)
        ))
        .num_columns(num_grid_cols)
        .spacing([8.0, 2.0])
        .show(ui, |ui| {
            // Header row
            ui.label("");
            if !compact {
                ui.label(header_style);
            }
            ui.label(
                RichText::new("Comp.")
                    .strong()
                    .size(12.0)
                    .color(Color32::from_rgb(150, 200, 255)),
            );
            let (grave_rect, _) =
                ui.allocate_exact_size(egui::vec2(icon_size, icon_size), egui::Sense::hover());
            ctx.sprite_renderer
                .draw_sprite_in_rect(ui, GRAVESTONE_ID, grave_rect);
            ui.label(
                RichText::new("%")
                    .strong()
                    .size(12.0)
                    .color(Color32::from_rgb(150, 200, 255)),
            );
            ui.label(
                RichText::new("Collection")
                    .strong()
                    .size(12.0)
                    .color(Color32::from_rgb(150, 200, 255)),
            );
            ui.end_row();

            for &idx in indices {
                let row = &self.index_rows[idx];
                let color = if row.total_items > 0 || row.completions > 0 {
                    Color32::WHITE
                } else {
                    Color32::from_rgb(80, 80, 80)
                };

                let portal_id = portal_map.get_portal_id(&row.name);

                // Icon
                let (sprite_rect, sprite_resp) =
                    ui.allocate_exact_size(egui::vec2(icon_size, icon_size), egui::Sense::hover());
                if let Some(id) = portal_id {
                    ctx.sprite_renderer
                        .draw_outlined_sprite_in_rect(ui, id, sprite_rect);
                }
                attach_dungeon_tooltip(sprite_resp, ctx.sprite_renderer, &row.name, &row.name);

                // Name
                if !compact {
                    let name_resp = ui.label(RichText::new(&row.name).color(color));
                    attach_dungeon_tooltip(name_resp, ctx.sprite_renderer, &row.name, &row.name);
                }

                // Completions
                if row.completions > 0 {
                    ui.label(RichText::new(format!("{}", row.completions)).color(color));
                } else {
                    ui.label(RichText::new("-").color(Color32::from_rgb(80, 80, 80)));
                }

                // Difficulty
                if let Some(diff) = row.difficulty {
                    let diff_color = if row.total_items > 0 || row.completions > 0 {
                        Color32::LIGHT_GRAY
                    } else {
                        Color32::from_rgb(80, 80, 80)
                    };
                    ui.label(RichText::new(format_difficulty(diff)).color(diff_color));
                } else {
                    ui.label(RichText::new("?").color(Color32::from_rgb(80, 80, 80)));
                }

                // Capture right edge before collection columns for click rect
                let click_right = ui.cursor().left();

                // Collection % and item sprites
                if let Some(collection) = self.collections.get(&row.name) {
                    let total = collection.items.len();
                    let visible = self.visible_items(&collection.items);
                    // % and counts always reflect the full (unfiltered)
                    // collection so applying a display filter doesn't
                    // change completion progress, only which icons show.
                    let obtained_count = {
                        realmhound_core::prof_scope!("dstats_pct");
                        collection
                            .items
                            .iter()
                            .filter(|item| self.obtained_items.contains(&item.item_id))
                            .count()
                    };
                    let pct = if total > 0 {
                        (obtained_count * 100) / total
                    } else {
                        0
                    };
                    let pct_color = if pct == 100 {
                        Color32::from_rgb(100, 255, 100)
                    } else if pct > 0 {
                        Color32::from_rgb(200, 200, 100)
                    } else {
                        Color32::from_rgb(80, 80, 80)
                    };
                    if total > 0 {
                        ui.label(RichText::new(format!("{pct}%")).size(11.0).color(pct_color));
                    } else {
                        ui.label(
                            RichText::new("-")
                                .size(11.0)
                                .color(Color32::from_rgb(80, 80, 80)),
                        );
                    }

                    if collection.is_sectioned() || total > MAX_INLINE {
                        // Collapsible dungeon: show toggle arrow
                        let expanded = self.expanded_collections.contains(&row.name);
                        let arrow = if expanded { "▼" } else { "▶" };
                        let toggle_text = format!("{arrow} {obtained_count}/{total}");
                        let btn = ui.small_button(&toggle_text);
                        if btn.clicked() {
                            if expanded {
                                self.expanded_collections.remove(&row.name);
                            } else {
                                self.expanded_collections.insert(row.name.clone());
                            }
                        }
                    } else {
                        // Regular dungeon: render inline sprites
                        let item_size = 16.0;
                        let spacing = 1.0;
                        self.draw_collection_items_row(
                            ui, ctx, &visible, item_size, spacing, col_right,
                        );
                    }
                } else {
                    ui.label("");
                    ui.label("");
                }

                ui.end_row();

                // Expanded view for collapsible dungeons
                if let Some(collection) = self.collections.get(&row.name) {
                    if self.expanded_collections.contains(&row.name) {
                        let item_size = 16.0;
                        let spacing = 1.0;

                        if let Some(sections) = &collection.sections {
                            // Sectioned: show boss groups
                            for section in sections {
                                let visible_section = self.visible_items(&section.items);
                                ui.label("");
                                if !compact {
                                    let section_obtained = section
                                        .items
                                        .iter()
                                        .filter(|i| self.obtained_items.contains(&i.item_id))
                                        .count();
                                    ui.label(
                                        RichText::new(format!(
                                            "  {} ({}/{})",
                                            section.name,
                                            section_obtained,
                                            section.items.len()
                                        ))
                                        .size(11.0)
                                        .color(Color32::from_rgb(180, 180, 220)),
                                    );
                                }
                                ui.label("");
                                ui.label("");
                                ui.label("");
                                self.draw_collection_items_row(
                                    ui,
                                    ctx,
                                    &visible_section,
                                    item_size,
                                    spacing,
                                    col_right,
                                );
                                ui.end_row();
                            }
                        } else {
                            // Non-sectioned: show all items in one expanded row
                            ui.label("");
                            if !compact {
                                ui.label("");
                            }
                            ui.label("");
                            ui.label("");
                            ui.label("");
                            let visible_all = self.visible_items(&collection.items);
                            self.draw_collection_items_row(
                                ui,
                                ctx,
                                &visible_all,
                                item_size,
                                spacing,
                                col_right,
                            );
                            ui.end_row();
                        }
                    }
                }

                // Handle row click via hover highlight on last row rect
                let resp = ui.interact(
                    egui::Rect::from_min_max(
                        egui::pos2(sprite_rect.left(), sprite_rect.top()),
                        egui::pos2(click_right, sprite_rect.bottom()),
                    ),
                    ui.id().with(("row_click", &row.name)),
                    egui::Sense::click(),
                );
                if resp.hovered() {
                    ui.painter().rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(sprite_rect.left(), sprite_rect.top()),
                            egui::pos2(click_right, sprite_rect.bottom()),
                        ),
                        2.0,
                        Color32::from_white_alpha(15),
                    );
                }
                if resp.clicked() {
                    *navigate_to = Some((row.name.clone(), row.db_names.clone()));
                }
                // The row-wide click rect is drawn on top of the icon and
                // name, so attach the drop tooltip here too -- otherwise the
                // occluded icon/name responses never register as hovered.
                attach_dungeon_tooltip(resp, ctx.sprite_renderer, &row.name, &row.name);
            }
        });
    }

    fn render_icon_view(&mut self, ui: &mut egui::Ui, ctx: &mut PanelContext) {
        let portal_map = get_dungeon_portal_map();
        let query_lower = self.search_query.to_lowercase();
        let show_no_collection = self.show_no_collection_dungeons;
        let collections = &self.collections;
        let filtered_indices: Vec<usize> = self
            .index_rows
            .iter()
            .enumerate()
            .filter(|(_, r)| query_lower.is_empty() || r.name.to_lowercase().contains(&query_lower))
            .filter(|(_, r)| {
                show_no_collection
                    || collections
                        .get(&r.name)
                        .map(|c| !c.items.is_empty())
                        .unwrap_or(true)
            })
            .map(|(i, _)| i)
            .collect();

        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let mut navigate_to: Option<(String, Vec<String>)> = None;

            let available_width = ui.available_width();
            let button_width = 150.0;
            let cols = (available_width / button_width).floor().max(1.0) as usize;

            egui::Grid::new("dungeon_icon_grid")
                .num_columns(cols)
                .spacing(egui::vec2(12.0, 12.0))
                .show(ui, |ui| {
                    for (i, &idx) in filtered_indices.iter().enumerate() {
                        if i > 0 && i % cols == 0 {
                            ui.end_row();
                        }
                        let row = &self.index_rows[idx];

                        let portal_id = portal_map.get_portal_id(&row.name);
                        let tile_width = button_width - 12.0;
                        let tile_height = 120.0;

                        let (rect, resp) = ui.allocate_exact_size(
                            egui::vec2(tile_width, tile_height),
                            egui::Sense::click(),
                        );

                        if resp.clicked() {
                            navigate_to = Some((row.name.clone(), row.db_names.clone()));
                        }
                        attach_dungeon_tooltip(
                            resp.clone(),
                            ctx.sprite_renderer,
                            &row.name,
                            &row.name,
                        );

                        if ui.is_rect_visible(rect) {
                            let painter = ui.painter();

                            let bg_color = if resp.hovered() {
                                Color32::from_white_alpha(25)
                            } else {
                                Color32::from_white_alpha(10)
                            };
                            painter.rect_filled(rect, 4.0, bg_color);

                            // Portal sprite (centered vertically in upper portion)
                            let sprite_size = 40.0;
                            let sprite_rect = egui::Rect::from_center_size(
                                egui::pos2(rect.center().x, rect.top() + 38.0),
                                egui::vec2(sprite_size, sprite_size),
                            );
                            if let Some(id) = portal_id {
                                ctx.sprite_renderer.draw_outlined_sprite_in_rect(
                                    ui,
                                    id,
                                    sprite_rect,
                                );
                            } else {
                                painter.text(
                                    sprite_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "◆",
                                    egui::FontId::proportional(20.0),
                                    Color32::GRAY,
                                );
                            }

                            // Completions (top-right corner)
                            if row.completions > 0 {
                                let comp_text = format!("{}", row.completions);
                                let comp_font = egui::FontId::proportional(10.0);
                                painter.text(
                                    egui::pos2(rect.right() - 4.0, rect.top() + 4.0),
                                    egui::Align2::RIGHT_TOP,
                                    &comp_text,
                                    comp_font,
                                    Color32::from_rgb(180, 220, 255),
                                );
                            }

                            // Dungeon name - wrap and center each line independently
                            let name_font = egui::FontId::proportional(12.5);
                            let wrap_width = tile_width - 8.0;
                            let name_top = rect.top() + 62.0;

                            // First layout to get line breaks
                            let name_galley = painter.layout(
                                row.name.clone(),
                                name_font.clone(),
                                Color32::WHITE,
                                wrap_width,
                            );
                            let name_height = name_galley.size().y;
                            let line_height = if name_galley.rows.len() > 1 {
                                name_galley.rows[0].height()
                            } else {
                                name_height
                            };

                            // Render each row centered
                            for (line_idx, row_info) in name_galley.rows.iter().enumerate() {
                                let line_text: String =
                                    row_info.glyphs.iter().map(|g| g.chr).collect();
                                let line_text = line_text.trim().to_string();
                                if line_text.is_empty() {
                                    continue;
                                }
                                let y = name_top + line_idx as f32 * line_height;
                                painter.text(
                                    egui::pos2(rect.center().x, y),
                                    egui::Align2::CENTER_TOP,
                                    &line_text,
                                    name_font.clone(),
                                    Color32::WHITE,
                                );
                            }

                            // Difficulty: N [gravestone] - always below 2 lines of name space
                            if let Some(diff) = row.difficulty {
                                let diff_text = format!("Difficulty: {}", format_difficulty(diff));
                                let diff_font = egui::FontId::proportional(10.0);
                                let text_galley = painter.layout_no_wrap(
                                    diff_text.clone(),
                                    diff_font.clone(),
                                    Color32::LIGHT_GRAY,
                                );
                                let grave_size = 12.0;
                                let total_w = text_galley.size().x + 4.0 + grave_size;
                                let left_x = rect.center().x - total_w / 2.0;
                                let two_line_height = line_height * 2.0;
                                let diff_y = name_top + two_line_height + 2.0;

                                painter.text(
                                    egui::pos2(left_x, diff_y),
                                    egui::Align2::LEFT_TOP,
                                    &diff_text,
                                    diff_font,
                                    Color32::LIGHT_GRAY,
                                );

                                let grave_rect = egui::Rect::from_center_size(
                                    egui::pos2(
                                        left_x + text_galley.size().x + 4.0 + grave_size / 2.0,
                                        diff_y + text_galley.size().y / 2.0,
                                    ),
                                    egui::vec2(grave_size, grave_size),
                                );
                                ctx.sprite_renderer.draw_sprite_in_rect(
                                    ui,
                                    GRAVESTONE_ID,
                                    grave_rect,
                                );
                            }
                        }
                    }
                });

            if let Some((display_name, db_names)) = navigate_to {
                self.load_detail(&db_names, ctx.loot_database, &display_name);
                self.page = Page::Detail { name: display_name };
            }
        });
    }

    fn render_detail(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &mut PanelContext,
        dungeon: &str,
    ) -> Vec<AppAction> {
        let actions = Vec::new();

        // Lazily reload detail cache if cleared by refresh
        if self.detail_cache.is_none() {
            let db_names: Vec<String> = self
                .index_rows
                .iter()
                .find(|r| r.name == dungeon)
                .map(|r| r.db_names.clone())
                .unwrap_or_default();
            self.load_detail(&db_names, ctx.loot_database, dungeon);
        }

        ui.horizontal(|ui| {
            ui.set_min_height(32.0);
            ui.add(egui::Label::new(""));
            if ui.button("← Back").clicked() {
                self.page = Page::Index;
                self.detail_cache = None;
            }

            let portal_map = get_dungeon_portal_map();
            let portal_id = portal_map.get_portal_id(dungeon);
            let portal_resp = render_portal_sprite_sized(ui, portal_id, ctx.sprite_renderer, 32.0);
            attach_dungeon_tooltip(portal_resp, ctx.sprite_renderer, dungeon, dungeon);

            let name_resp = ui.heading(dungeon);
            attach_dungeon_tooltip(name_resp, ctx.sprite_renderer, dungeon, dungeon);

            let sep_color = Color32::from_rgb(150, 150, 150);

            if let Some(diff) = dungeon_difficulty(dungeon) {
                ui.label(RichText::new("•").size(15.0).strong().color(sep_color));
                ui.label(
                    RichText::new(format!("Difficulty: {}", format_difficulty(diff)))
                        .color(Color32::LIGHT_GRAY),
                );
                let (grave_rect, _) =
                    ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                ctx.sprite_renderer
                    .draw_sprite_in_rect(ui, GRAVESTONE_ID, grave_rect);
            }

            // Completion %: obtained/total of the full (unfiltered) collection.
            if let Some(collection) = self.collections.get(dungeon) {
                let total = collection.items.len();
                if total > 0 {
                    let obtained = collection
                        .items
                        .iter()
                        .filter(|i| self.obtained_items.contains(&i.item_id))
                        .count();
                    let pct = (obtained * 100) / total;
                    let pct_color = if pct == 100 {
                        Color32::from_rgb(100, 255, 100)
                    } else if pct > 0 {
                        Color32::from_rgb(200, 200, 100)
                    } else {
                        Color32::from_rgb(80, 80, 80)
                    };
                    ui.label(RichText::new("•").size(15.0).strong().color(sep_color));
                    ui.label(RichText::new(format!("{pct}%")).color(pct_color));
                }
            }

            // Runs: total completions across all characters.
            let completions = self
                .index_rows
                .iter()
                .find(|r| r.name == dungeon)
                .map(|r| r.completions)
                .unwrap_or(0);
            ui.label(RichText::new("•").size(15.0).strong().color(sep_color));
            let runs_label = if completions == 1 {
                "1 run".to_string()
            } else {
                format!("{completions} runs")
            };
            ui.label(RichText::new(runs_label).color(Color32::LIGHT_GRAY));

            // Time spent: running per-dungeon counter (RealmHound and/or
            // RealmShark, per the Data Source selector). Hidden when unknown.
            let time_ms = self
                .index_rows
                .iter()
                .find(|r| r.name == dungeon)
                .map(|r| r.time_ms)
                .unwrap_or(0);
            if time_ms > 0 {
                ui.label(RichText::new("•").size(15.0).strong().color(sep_color));
                ui.label(
                    RichText::new(format!("Time spent: {}", format_dungeon_time(time_ms)))
                        .color(Color32::LIGHT_GRAY),
                );
            }
        });

        ui.separator();

        self.render_detail_body(ui, ctx, dungeon);

        actions
    }

    /// Enlarged collection grid shown at the top of the dungeon detail page.
    fn render_detail_collection(&self, ui: &mut egui::Ui, ctx: &mut PanelContext, dungeon: &str) {
        let Some(collection) = self.collections.get(dungeon) else {
            return;
        };

        let item_size = 38.0;
        let spacing = 3.0;
        let visible = self.visible_items(&collection.items);
        // Counts always reflect the full (unfiltered) collection so applying
        // a display filter doesn't change completion progress.
        let total = collection.items.len();
        let obtained_count = collection
            .items
            .iter()
            .filter(|i| self.obtained_items.contains(&i.item_id))
            .count();

        ui.horizontal(|ui| {
            ui.heading("Collection");
            ui.label(
                RichText::new(format!("({obtained_count}/{total})")).color(Color32::LIGHT_GRAY),
            );
        });
        ui.add_space(6.0);

        if let Some(sections) = &collection.sections {
            for section in sections {
                let visible_section = self.visible_items(&section.items);
                let sec_obtained = section
                    .items
                    .iter()
                    .filter(|i| self.obtained_items.contains(&i.item_id))
                    .count();
                egui::CollapsingHeader::new(
                    RichText::new(format!(
                        "{} ({}/{})",
                        section.name,
                        sec_obtained,
                        section.items.len()
                    ))
                    .strong()
                    .color(Color32::from_rgb(180, 180, 220)),
                )
                .id_salt((dungeon, &section.name))
                .default_open(true)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(spacing, spacing);
                        for item in &visible_section {
                            self.draw_collection_item_icon(
                                ui,
                                ctx,
                                item.item_id,
                                &item.name,
                                item_size,
                                true,
                            );
                        }
                    });
                });
                ui.add_space(6.0);
            }
        } else {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(spacing, spacing);
                for item in &visible {
                    self.draw_collection_item_icon(
                        ui,
                        ctx,
                        item.item_id,
                        &item.name,
                        item_size,
                        true,
                    );
                }
            });
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);
    }

    /// Draws collection item icons, wrapping onto up to `MAX_ITEM_ROW_LINES`
    /// lines to use available vertical space before falling back to a "+N"
    /// truncation indicator (hover shows the hidden item names). Capping the
    /// line count (rather than wrapping unbounded) keeps the index list's
    /// column-balancing estimate (`estimated_row_weight`) reasonably accurate;
    /// unbounded wrapping previously let one row grow far taller than
    /// estimated, causing columns to overlap/misalign after a resize.
    const MAX_ITEM_ROW_LINES: usize = 2;

    /// Whether items of `group` should currently be shown, per the top-bar
    /// ST/UT/Shiny filters. Upgrade materials are never hidden by these
    /// filters since they aren't ST/UT/Shiny items.
    fn group_visible(&self, group: ItemGroup) -> bool {
        match group {
            ItemGroup::ShinyUt => self.show_shiny,
            ItemGroup::Ut => self.show_ut,
            ItemGroup::St => self.show_st,
            ItemGroup::Upgrade => true,
        }
    }

    /// Clones the subset of `items` currently visible under the ST/UT/Shiny
    /// filters, for use anywhere a collection's items are counted or drawn.
    fn visible_items(&self, items: &[CollectionItem]) -> Vec<CollectionItem> {
        items
            .iter()
            .filter(|i| self.group_visible(i.group))
            .cloned()
            .collect()
    }

    fn draw_collection_items_row(
        &self,
        ui: &mut egui::Ui,
        ctx: &mut PanelContext,
        items: &[CollectionItem],
        item_size: f32,
        spacing: f32,
        col_right: f32,
    ) {
        // Measured directly from the known right edge of this column rather
        // than `ui.available_width()`: egui's `Grid` reports the last
        // column's available width using frame-lagged/heuristic logic (see
        // `GridLayout::available_rect`), which was unreliable here and let
        // long icon rows silently skip wrapping or overflow past the edge.
        let available = (col_right - ui.cursor().left()).max(item_size);
        let slot_width = item_size + spacing;
        let per_line = ((available / slot_width).floor() as usize).max(1);
        let capacity = per_line * Self::MAX_ITEM_ROW_LINES;

        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            if items.len() <= capacity {
                for chunk in items.chunks(per_line) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = spacing;
                        for item in chunk {
                            self.draw_collection_item_icon(
                                ui,
                                ctx,
                                item.item_id,
                                &item.name,
                                item_size,
                                false,
                            );
                        }
                    });
                }
            } else {
                let full_lines = Self::MAX_ITEM_ROW_LINES.saturating_sub(1);
                for chunk in items[..full_lines * per_line].chunks(per_line) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = spacing;
                        for item in chunk {
                            self.draw_collection_item_icon(
                                ui,
                                ctx,
                                item.item_id,
                                &item.name,
                                item_size,
                                false,
                            );
                        }
                    });
                }
                let remaining = &items[full_lines * per_line..];
                // Reserve one slot's worth of width for the "+N" label.
                let shown = per_line.saturating_sub(1).max(1).min(remaining.len());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = spacing;
                    for item in &remaining[..shown] {
                        self.draw_collection_item_icon(
                            ui,
                            ctx,
                            item.item_id,
                            &item.name,
                            item_size,
                            false,
                        );
                    }
                    let hidden = remaining.len() - shown;
                    ui.label(
                        RichText::new(format!("+{hidden}"))
                            .size(10.0)
                            .color(Color32::from_rgb(150, 150, 150)),
                    )
                    .hover_tip(
                        remaining[shown..]
                            .iter()
                            .map(|i| i.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                    );
                });
            }
        });
    }

    fn draw_collection_item_icon(
        &self,
        ui: &mut egui::Ui,
        ctx: &mut PanelContext,
        item_id: i32,
        item_name: &str,
        item_size: f32,
        detail: bool,
    ) {
        let obtained = self.obtained_items.contains(&item_id);
        let (item_rect, resp) =
            ui.allocate_exact_size(egui::vec2(item_size, item_size), egui::Sense::hover());
        if detail {
            // Match the "All Items" loot tiles: native-frame art inset inside a
            // padded slot (frameless here) so overlays like the shiny sparkle
            // have top/left margin and aren't clipped.
            let art_rect = item_rect.shrink(item_size * 3.0 / 38.0);
            if obtained {
                ctx.sprite_renderer
                    .draw_outlined_sprite_in_rect_native(ui, item_id, art_rect);
            } else {
                ctx.sprite_renderer
                    .draw_outlined_sprite_in_rect_native_tinted(
                        ui,
                        item_id,
                        art_rect,
                        Color32::from_rgb(30, 30, 30),
                    );
            }
            if ctx.sprite_renderer.is_shiny(item_id) {
                // Darken the sparkle for unobtained items so it only lights up
                // once the item is discovered.
                let sparkle_tint = if obtained {
                    Color32::WHITE
                } else {
                    Color32::from_rgb(30, 30, 30)
                };
                ctx.sprite_renderer
                    .draw_shiny_overlay_in_rect(ui, item_rect, sparkle_tint);
            }
        } else {
            // Index list: tight, sparkle-free icons filling the cell.
            if obtained {
                ctx.sprite_renderer
                    .draw_outlined_sprite_in_rect(ui, item_id, item_rect);
            } else {
                ctx.sprite_renderer.draw_outlined_sprite_in_rect_tinted(
                    ui,
                    item_id,
                    item_rect,
                    Color32::from_rgb(30, 30, 30),
                );
            }
        }
        let seen = self.rarity_seen.get(&item_id);
        let possible: &[u8] = if has_no_enchant_slots(item_id) {
            &[]
        } else {
            restricted_rarities(item_id).unwrap_or(&[0, 1, 2, 3, 4])
        };
        let rarities: Vec<(u8, &str, bool)> = possible
            .iter()
            .map(|&slots| {
                (
                    slots,
                    rarity_name(slots),
                    seen.map(|s| s.contains(&slots)).unwrap_or(false),
                )
            })
            .collect();
        ctx.sprite_renderer
            .show_item_tooltip_with_rarities(ui, &resp, item_id, item_name, &rarities);
    }

    /// Tracked-loot breakdown (per-item drop counts + mob-item table), sourced
    /// via the Data Source selector, shown below the collection on the detail page.
    fn render_detail_body(&mut self, ui: &mut egui::Ui, ctx: &mut PanelContext, dungeon: &str) {
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            self.render_detail_collection(ui, ctx, dungeon);

            let Some(detail) = &self.detail_cache else {
                ui.label("No tracked loot drops recorded.");
                return;
            };

            if detail.items.is_empty() {
                ui.label("No tracked loot drops recorded.");
                return;
            }

            ui.heading("All Items");
            ui.add_space(4.0);

            let available_width = ui.available_width();
            let tile_width = 46.0;
            let spacing = 4.0;
            let cols = (available_width / tile_width).floor().max(1.0) as usize;
            let row_height = 62.0; // tile + count label
            let total_rows = (detail.items.len() + cols - 1) / cols;

            // Virtualized grid: allocate full height, render only visible rows
            let (total_rect, _) = ui.allocate_exact_size(
                egui::vec2(available_width, total_rows as f32 * (row_height + spacing)),
                egui::Sense::hover(),
            );

            let clip_rect = ui.clip_rect();
            let first_visible_row = ((clip_rect.top() - total_rect.top()) / (row_height + spacing))
                .floor()
                .max(0.0) as usize;
            let last_visible_row = ((clip_rect.bottom() - total_rect.top())
                / (row_height + spacing))
                .ceil()
                .min(total_rows as f32) as usize;

            for row in first_visible_row..last_visible_row {
                let row_start = row * cols;
                let row_end = (row_start + cols).min(detail.items.len());
                let row_y = total_rect.top() + row as f32 * (row_height + spacing);

                for (col, item_idx) in (row_start..row_end).enumerate() {
                    let item = &detail.items[item_idx];
                    let tile_x = total_rect.left() + col as f32 * tile_width;
                    let tile_rect = egui::Rect::from_min_size(
                        egui::pos2(tile_x, row_y),
                        egui::vec2(tile_width, row_height),
                    );

                    let mut child_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(tile_rect)
                            .layout(egui::Layout::top_down(egui::Align::Center)),
                    );
                    let rarity_counts: Vec<(u8, &str, u32)> =
                        if detail.enchantable.contains(&item.item_id) {
                            detail
                                .rarity_breakdowns
                                .get(&item.item_id)
                                .map(|breakdown| {
                                    breakdown
                                        .iter()
                                        .map(|&(slots, count)| {
                                            (slots as u8, rarity_name(slots as u8), count as u32)
                                        })
                                        .collect()
                                })
                                .unwrap_or_default()
                        } else {
                            Vec::new()
                        };
                    ctx.sprite_renderer.render_item_tile_with_rarity_counts(
                        &mut child_ui,
                        item.item_id,
                        Some(&item.item_name),
                        &rarity_counts,
                    );
                    child_ui.label(
                        RichText::new(format!("×{}", item.total_count))
                            .size(10.0)
                            .color(Color32::LIGHT_GRAY),
                    );
                }
            }

            if !detail.mob_items.is_empty() {
                ui.add_space(12.0);
                ui.heading("Drops By Monster");
                ui.add_space(4.0);

                let mut i = 0;
                while i < detail.mob_items.len() {
                    let mob_name = detail.mob_items[i].mob_name.clone();
                    let mob_type = detail.mob_items[i].mob_type;

                    let start = i;
                    while i < detail.mob_items.len() && detail.mob_items[i].mob_name == mob_name {
                        i += 1;
                    }

                    let header_id = egui::Id::new(format!("mob_section_{}_{}", mob_type, start));
                    egui::collapsing_header::CollapsingState::load_with_default_open(
                        ui.ctx(),
                        header_id,
                        false,
                    )
                    .show_header(ui, |ui| {
                        let sprite_type =
                            realmhound_core::assets::normalize_train_sprite(mob_type, &mob_name);
                        render_mob_sprite(ui, sprite_type, ctx.sprite_renderer);
                        ui.label(RichText::new(&mob_name).strong());
                    })
                    .body(|ui| {
                        egui::Grid::new(format!("mob_items_{}_{}", mob_type, start))
                            .num_columns(cols)
                            .spacing(egui::vec2(4.0, 4.0))
                            .show(ui, |ui| {
                                for (j, idx) in (start..i).enumerate() {
                                    if j > 0 && j % cols == 0 {
                                        ui.end_row();
                                    }
                                    let mob_item = &detail.mob_items[idx];
                                    ui.vertical(|ui| {
                                        let rarity_counts: Vec<(u8, &str, u32)> =
                                            if detail.enchantable.contains(&mob_item.item_id) {
                                                detail
                                                    .rarity_breakdowns
                                                    .get(&mob_item.item_id)
                                                    .map(|breakdown| {
                                                        breakdown
                                                            .iter()
                                                            .map(|&(slots, count)| {
                                                                (
                                                                    slots as u8,
                                                                    rarity_name(slots as u8),
                                                                    count as u32,
                                                                )
                                                            })
                                                            .collect()
                                                    })
                                                    .unwrap_or_default()
                                            } else {
                                                Vec::new()
                                            };
                                        ctx.sprite_renderer.render_item_tile_with_rarity_counts(
                                            ui,
                                            mob_item.item_id,
                                            Some(&mob_item.item_name),
                                            &rarity_counts,
                                        );
                                        ui.label(
                                            RichText::new(format!("×{}", mob_item.count))
                                                .size(10.0)
                                                .color(Color32::LIGHT_GRAY),
                                        );
                                    });
                                }
                            });
                    });

                    ui.add_space(4.0);
                }
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Panel trait
// ---------------------------------------------------------------------------

impl Panel for TrophyHallPanel {
    fn show(&mut self, ui: &mut egui::Ui, ctx: &mut PanelContext) -> Vec<AppAction> {
        self.refresh_if_stale(
            ctx.loot_database,
            ctx.account_data,
            ctx.combat_database,
            ctx.account_verified,
        );

        match self.page.clone() {
            Page::Index => self.render_index(ui, ctx),
            Page::Detail { name } => self.render_detail(ui, ctx, &name),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn format_difficulty(diff: f32) -> String {
    if diff.fract() == 0.0 {
        format!("{}", diff as i32)
    } else {
        format!("{:.1}", diff)
    }
}

/// Human-readable dungeon time (e.g. "3h 12m", "8m 5s", "42s").
fn format_dungeon_time(ms: u64) -> String {
    let total_secs = ms / 1000;
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    if hours > 0 {
        format!("{hours}h {mins}m")
    } else if mins > 0 {
        format!("{mins}m {secs}s")
    } else {
        format!("{secs}s")
    }
}

const GRAVESTONE_ID: i32 = 1282;
/// Collections above this many items switch from inline sprite rows to a
/// collapsible toggle in the index list (matches render_list_column).
const MAX_INLINE_COLLECTION_ITEMS: usize = 24;

fn render_portal_sprite_sized(
    ui: &mut egui::Ui,
    portal_id: Option<i32>,
    sprite_renderer: &mut SpriteRenderer,
    size: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let drawn = if let Some(id) = portal_id {
        sprite_renderer.draw_sprite_in_rect(ui, id, rect)
    } else {
        false
    };
    if !drawn {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "◆",
            egui::FontId::proportional(size * 0.5),
            Color32::GRAY,
        );
    }
    response
}

fn render_mob_sprite(ui: &mut egui::Ui, mob_type: i32, sprite_renderer: &mut SpriteRenderer) {
    let size = 28.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    // Skip environmental structures wrongly stored as sources; leave the "?" gap.
    if mob_type > 0 && realmhound_core::assets::get_asset_manager().is_valid_drop_source(mob_type) {
        sprite_renderer.draw_outlined_sprite_in_rect(ui, mob_type, rect);
    }
}

/// Rarity rank for sorting: Shiny UT (4) > UT (3) > ST (2) > Tiered (1) > rest (0).
fn item_rarity_rank(item_id: i32, asset_mgr: &realmhound_core::assets::AssetManager) -> u8 {
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

/// Non-dungeon locations that appear in loot DB but shouldn't be shown as dungeons.
fn is_non_dungeon_location(name: &str) -> bool {
    const EXCLUDED: &[&str] = &["Nexus", "Guild Hall", "{s.nexus}"];
    EXCLUDED.iter().any(|&e| name.eq_ignore_ascii_case(e))
}

#[cfg(test)]
mod gating_tests {
    use super::*;

    #[test]
    fn realmshark_gating_requires_a_bound_import_path() {
        let mut panel = TrophyHallPanel::new();
        // No account selected: no import path, RealmShark disabled.
        assert!(!panel.has_realmshark_path());
        // Import is a no-op (never opens a file dialog) with no selected account.
        panel.import_realmshark();
        assert!(!panel.realmshark_loaded());

        // Binding a compatibility import path enables the selection gate.
        panel.set_realmshark_import_path(Some(std::path::PathBuf::from("imports/dungeon.stats")));
        assert!(panel.has_realmshark_path());

        // Clearing it disables RealmShark again.
        panel.set_realmshark_import_path(None);
        assert!(!panel.has_realmshark_path());
    }
}

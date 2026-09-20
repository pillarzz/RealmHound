use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::assets::{get_asset_manager, get_dungeon_portal_map};
use crate::loot::{DungeonItemStat, MobItemStat};
use crate::stats::dungeon_registry::get_dungeon_registry;

#[derive(Debug, Deserialize)]
struct RawDungeonStats {
    data: HashMap<String, RawDungeonInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawDungeonInfo {
    name: String,
    entered_dungeon: i32,
    total_time: i64,
    #[serde(default)]
    #[allow(dead_code)]
    entity_damaged: HashMap<String, i32>,
    #[serde(default)]
    entity_loot: HashMap<String, RawLoot>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawLoot {
    loot_list: HashMap<String, i32>,
}

/// Parsed and resolved RealmShark dungeon statistics.
#[derive(Debug, Clone)]
pub struct RealmSharkStats {
    pub dungeons: HashMap<String, RealmSharkDungeon>,
}

#[derive(Debug, Clone)]
pub struct RealmSharkDungeon {
    pub name: String,
    pub completions: i32,
    pub total_time_ms: i64,
    pub items: Vec<DungeonItemStat>,
    pub mob_items: Vec<MobItemStat>,
}

/// Copies a dungeon.stats file to an explicit destination path, creating parent
/// directories as needed.
pub fn copy_to_import_dir(source: &Path, dest: &Path) -> std::io::Result<PathBuf> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, dest)?;
    Ok(dest.to_path_buf())
}

/// Parses the RealmShark dungeon.stats JSON file.
pub fn parse_dungeon_stats(path: &Path) -> Result<RealmSharkStats, String> {
    let content = fs::read_to_string(path).map_err(|e| format!("Failed to read file: {}", e))?;

    let raw: RawDungeonStats =
        serde_json::from_str(&content).map_err(|e| format!("Failed to parse JSON: {}", e))?;

    let asset_mgr = get_asset_manager();
    let registry = get_dungeon_registry();
    let portal_map = get_dungeon_portal_map();
    let mark_ids = asset_mgr.mark_item_ids();
    let mut dungeons = HashMap::new();

    for (_key, info) in raw.data {
        let display = portal_map.normalize_dungeon_name(&info.name);
        let canonical = registry.normalize(&display);

        let mut items_map: HashMap<i32, DungeonItemStat> = HashMap::new();
        let mut mob_items = Vec::new();
        let mut mark_count: i32 = 0;

        for (mob_id_str, loot) in &info.entity_loot {
            let mob_type: i32 = mob_id_str.parse().unwrap_or(0);
            let mob_name = if mob_type > 0 {
                asset_mgr
                    .object_name(mob_type)
                    .unwrap_or_else(|| format!("Unknown ({})", mob_type))
            } else {
                "Unknown".to_string()
            };

            for (item_id_str, &count) in &loot.loot_list {
                let item_id: i32 = item_id_str.parse().unwrap_or(0);
                if item_id == 0 {
                    continue;
                }

                if mark_ids.contains(&item_id) {
                    mark_count += count;
                }

                let item_name = asset_mgr
                    .object_name(item_id)
                    .unwrap_or_else(|| format!("Unknown ({})", item_id));

                let entry = items_map.entry(item_id).or_insert_with(|| DungeonItemStat {
                    item_id,
                    item_name: item_name.clone(),
                    total_count: 0,
                    bag_type_counts: Vec::new(),
                });
                entry.total_count += count as i64;

                mob_items.push(MobItemStat {
                    mob_type,
                    mob_name: mob_name.clone(),
                    item_id,
                    item_name,
                    count: count as i64,
                });
            }
        }

        let completions = if mark_count > 0 {
            mark_count
        } else {
            info.entered_dungeon
        };

        let mut items: Vec<DungeonItemStat> = items_map.into_values().collect();
        items.sort_by(|a, b| b.total_count.cmp(&a.total_count));
        mob_items.sort_by(|a, b| a.mob_name.cmp(&b.mob_name).then(b.count.cmp(&a.count)));

        let entry = dungeons
            .entry(canonical.clone())
            .or_insert_with(|| RealmSharkDungeon {
                name: canonical.clone(),
                completions: 0,
                total_time_ms: 0,
                items: Vec::new(),
                mob_items: Vec::new(),
            });
        entry.completions += completions;
        entry.total_time_ms += info.total_time;
        // Merge items
        for item in items {
            if let Some(existing) = entry.items.iter_mut().find(|i| i.item_id == item.item_id) {
                existing.total_count += item.total_count;
            } else {
                entry.items.push(item);
            }
        }
        entry.mob_items.extend(mob_items);
    }

    Ok(RealmSharkStats { dungeons })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_to_import_dir_writes_to_explicit_destination() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("dungeon.stats");
        fs::write(&source, br#"{"data":{}}"#).unwrap();

        let dest = temp
            .path()
            .join("imports")
            .join("realmshark")
            .join("dungeon.stats");
        let written = copy_to_import_dir(&source, &dest).unwrap();

        assert_eq!(written, dest);
        assert_eq!(fs::read(&dest).unwrap(), br#"{"data":{}}"#);
    }

    #[test]
    fn two_accounts_import_to_isolated_paths() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("dungeon.stats");
        fs::write(&source, b"payload").unwrap();

        let a = temp.path().join("a").join("dungeon.stats");
        let b = temp.path().join("b").join("dungeon.stats");
        copy_to_import_dir(&source, &a).unwrap();
        copy_to_import_dir(&source, &b).unwrap();

        assert_ne!(a, b);
        assert!(a.exists() && b.exists());
    }
}

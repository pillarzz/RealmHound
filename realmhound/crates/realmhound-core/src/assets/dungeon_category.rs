//! In-game forge/tooltip categories, derived from the `collectionIcon`
//! attribute on items in `equip.xml`.
//!
//! RotMG shows a small category icon in the corner of an item's tooltip. That
//! icon is a frame in the streamed `CollectionIcon` sprite sheet, and the frame
//! index is stored per-item as `collectionIcon="N"`. Items that share an index
//! belong to the same in-game forge category (e.g. Oryx's Castle and Wine
//! Cellar items both use index 154), even when they drop from different bosses.
//!
//! This module groups UT items by that index so the Treasury can offer a
//! category filter that matches the game's own grouping.

use std::collections::BTreeMap;

use super::object_list::ObjectList;
use crate::stats::dungeon_registry::get_dungeon_registry;

/// A single forge/tooltip category, keyed by its `CollectionIcon` frame index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DungeonCategory {
    /// Frame index into the `CollectionIcon` sprite sheet.
    pub icon_index: i32,
    /// Human-readable label (derived from the dungeon(s) whose items share this
    /// icon). Joins multiple dungeon names with " / ".
    pub display_name: String,
    /// All distinct dungeon names contributing UT items to this category,
    /// sorted. Used for search matching (a category matches if the query hits
    /// any of these names). Empty when no item carried an `ORG_` label.
    pub dungeon_names: Vec<String>,
    /// UT item type IDs belonging to this category, sorted.
    pub item_ids: Vec<i32>,
}

impl DungeonCategory {
    /// Whether the given lowercase query matches this category's label or any
    /// of its contained dungeon names.
    pub fn matches_query(&self, query_lower: &str) -> bool {
        if query_lower.is_empty() {
            return true;
        }
        if self.display_name.to_lowercase().contains(query_lower) {
            return true;
        }
        self.dungeon_names
            .iter()
            .any(|n| n.to_lowercase().contains(query_lower))
    }
}

/// Build the list of forge categories from an object list.
///
/// Includes UT items and forge Blueprints that carry a `collectionIcon`, so
/// tiered/ST frames (e.g. the roman-numeral set-tier icons) never leak into the
/// category list. The result is sorted by display name for stable UI ordering.
pub fn build_categories(list: &ObjectList) -> Vec<DungeonCategory> {
    use std::collections::{BTreeSet, HashMap};
    let registry = get_dungeon_registry();

    /// Per-icon accumulator: item ids plus per-dungeon item counts and a tally
    /// of items with no resolvable dungeon (realm/event white bags).
    #[derive(Default)]
    struct Acc {
        ids: BTreeSet<i32>,
        name_counts: HashMap<String, usize>,
        unlabeled: usize,
    }

    let mut acc: BTreeMap<i32, Acc> = BTreeMap::new();

    for (id, obj) in list.iter_with_ids() {
        // Include UT items and forge Blueprints; both carry a forge-category
        // collectionIcon and can be used in the forge for that dungeon.
        if !obj.is_ut() && !obj.is_blueprint() {
            continue;
        }
        let Some(icon) = obj.collection_icon else {
            continue;
        };
        let entry = acc.entry(icon).or_default();
        entry.ids.insert(*id);
        let resolved = registry.dungeons_from_labels(&obj.labels);
        if resolved.is_empty() {
            entry.unlabeled += 1;
        } else {
            for name in resolved {
                *entry.name_counts.entry(name.to_string()).or_insert(0) += 1;
            }
        }
    }

    let mut categories: Vec<DungeonCategory> = acc
        .into_iter()
        .map(|(icon_index, acc)| {
            let mut dungeon_names: Vec<String> = acc.name_counts.keys().cloned().collect();
            dungeon_names.sort();
            let display_name = category_label(icon_index, &acc.name_counts, acc.unlabeled);
            DungeonCategory {
                icon_index,
                display_name,
                dungeon_names,
                item_ids: acc.ids.into_iter().collect(),
            }
        })
        .collect();

    categories.sort_by(|a, b| {
        a.display_name
            .to_lowercase()
            .cmp(&b.display_name.to_lowercase())
            .then(a.icon_index.cmp(&b.icon_index))
    });
    categories
}

/// Derive a display label from the per-dungeon item counts.
///
/// Uses the single dominant dungeon (the one contributing the most items) so a
/// category isn't mislabeled by a stray cross-listed item (e.g. one Deadwater
/// Docks drop that shares The Nest's forge icon). When items without a
/// resolvable dungeon outnumber every dungeon -- i.e. a realm/event white-bag
/// bucket -- the category is labeled "Realm".
///
/// A small set of forge icons need a curated label the raw ORG labels can't
/// produce -- e.g. icon 101 is the Kitsune Umi sub-collection of Moonlight
/// Village, which shares Moonlight's ORG labels but is a distinct forge group.
fn category_label(
    icon_index: i32,
    name_counts: &std::collections::HashMap<String, usize>,
    unlabeled: usize,
) -> String {
    if let Some(name) = curated_label(icon_index) {
        return name.to_string();
    }
    // Highest item count wins; ties break alphabetically for stability.
    let top = name_counts
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)));
    match top {
        None => "Realm".to_string(),
        Some((name, count)) => {
            if unlabeled > *count {
                "Realm".to_string()
            } else {
                name.clone()
            }
        }
    }
}

/// Curated display names for forge icons whose raw ORG labels don't reflect the
/// in-game forge sub-collection. Kept intentionally tiny -- only add an entry
/// when the frequency-based label is genuinely misleading.
fn curated_label(icon_index: i32) -> Option<&'static str> {
    match icon_index {
        101 => Some("Kitsune Umi"),
        152 => Some("Crystal Prisoner"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore] // Run manually: cargo test dump_real_categories -- --ignored --nocapture
    fn dump_real_categories() {
        use crate::assets::{find_assets_dir, AssetManager};
        let am = AssetManager::new();
        if let Some(dir) = find_assets_dir() {
            am.set_assets_dir(&dir);
        }
        let cats = am.dungeon_categories();
        println!("=== {} categories ===", cats.len());
        for c in cats.iter() {
            println!(
                "  idx {:>3}  {:<45} items={:<3} dungeons={:?}",
                c.icon_index,
                c.display_name,
                c.item_ids.len(),
                c.dungeon_names
            );
        }
    }

    fn counts(pairs: &[(&str, usize)]) -> std::collections::HashMap<String, usize> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn label_single_dungeon() {
        assert_eq!(
            category_label(11, &counts(&[("Fungal Cavern", 15)]), 0),
            "Fungal Cavern"
        );
    }

    #[test]
    fn label_uses_dominant_dungeon() {
        // A stray cross-listed item (Big Berdtha / Deadwater Docks) shares The
        // Nest's forge icon; the dominant dungeon should win the label.
        let c = counts(&[("The Nest", 28), ("Deadwater Docks", 1)]);
        assert_eq!(category_label(13, &c, 0), "The Nest");
    }

    #[test]
    fn label_realm_when_unlabeled_dominates() {
        // Realm/event white-bag bucket: most items resolve to no dungeon.
        let c = counts(&[("Moonlight Village", 2)]);
        assert_eq!(category_label(100, &c, 68), "Realm");
    }

    #[test]
    fn label_realm_when_no_dungeon() {
        assert_eq!(category_label(13, &counts(&[]), 13), "Realm");
    }

    #[test]
    fn label_curated_kitsune_umi() {
        // Icon 101 shares Moonlight Village ORG labels but is its own forge group.
        let c = counts(&[("Moonlight Village", 5), ("Untaris", 2)]);
        assert_eq!(category_label(101, &c, 1), "Kitsune Umi");
    }

    #[test]
    fn label_curated_crystal_prisoner() {
        // Icon 152 is the Crystal Prisoner setpiece; ORG_SETPIECE maps to "Realm".
        let c = counts(&[("Realm", 13)]);
        assert_eq!(category_label(152, &c, 0), "Crystal Prisoner");
    }

    #[test]
    fn matches_query_by_dungeon_name() {
        let cat = DungeonCategory {
            icon_index: 154,
            display_name: "Oryx's Castle".to_string(),
            dungeon_names: vec!["Oryx's Castle".to_string(), "Wine Cellar".to_string()],
            item_ids: vec![1, 2],
        };
        assert!(cat.matches_query("wine"));
        assert!(cat.matches_query("castle"));
        assert!(cat.matches_query("")); // empty matches all
        assert!(!cat.matches_query("void"));
    }

    fn asset(labels: &str, collection_icon: Option<i32>) -> super::super::object_list::ObjectAsset {
        use super::super::object_list::ObjectAsset;
        ObjectAsset {
            id: 0,
            id_name: String::new(),
            display_name: String::new(),
            class: "Equipment".to_string(),
            group: String::new(),
            labels: labels.to_string(),
            textures: Vec::new(),
            projectiles: Vec::new(),
            mask: None,
            tex1: 0,
            tex2: 0,
            slot_type: 0,
            defense: 0,
            hitbox_scale: 1.0,
            tier: None,
            feed_power: 0,
            fame_bonus: 0,
            bag_type: 0,
            set_name: None,
            lethal_strike: None,
            collection_icon,
            rate_of_fire: 1.0,
            num_projectiles: 1,
            stat_bonuses: Default::default(),
            weapon_procs: Vec::new(),
        }
    }

    #[test]
    fn build_groups_ut_items_by_icon() {
        use super::super::object_list::ObjectList;
        let mut list = ObjectList::new();
        list.insert_asset(1, asset("UT", Some(154)));
        list.insert_asset(2, asset("UT", Some(154)));
        list.insert_asset(3, asset("UT", Some(17)));
        let cats = build_categories(&list);
        assert_eq!(cats.len(), 2);
        let c154 = cats.iter().find(|c| c.icon_index == 154).unwrap();
        assert_eq!(c154.item_ids, vec![1, 2]);
        let c17 = cats.iter().find(|c| c.icon_index == 17).unwrap();
        assert_eq!(c17.item_ids, vec![3]);
    }

    #[test]
    fn build_excludes_non_ut_and_missing_icon() {
        use super::super::object_list::ObjectList;
        let mut list = ObjectList::new();
        list.insert_asset(1, asset("UT", Some(154))); // included
        list.insert_asset(2, asset("", Some(0))); // not UT -> excluded (ST tier frame)
        list.insert_asset(3, asset("UT,CONSUMABLE", Some(154))); // consumable -> not UT
        list.insert_asset(4, asset("UT", None)); // UT but no icon -> excluded
        let cats = build_categories(&list);
        assert_eq!(cats.len(), 1);
        assert_eq!(cats[0].icon_index, 154);
        assert_eq!(cats[0].item_ids, vec![1]);
    }

    #[test]
    fn build_includes_blueprints() {
        use super::super::object_list::ObjectList;
        // Blueprints are Consumable-class (so not UT) but still belong to the
        // forge category matching their collectionIcon.
        let mut bp = asset("CONSUMABLE", Some(18));
        bp.id_name = "Blueprint_1".to_string();
        let mut bp_no_icon = asset("CONSUMABLE", None);
        bp_no_icon.id_name = "Blueprint_8".to_string();
        let mut list = ObjectList::new();
        list.insert_asset(1, asset("UT", Some(18))); // Sanctuary UT
        list.insert_asset(2, bp); // Sanctuary blueprint -> same category
        list.insert_asset(3, bp_no_icon); // blueprint w/o icon -> excluded
        let cats = build_categories(&list);
        assert_eq!(cats.len(), 1);
        let c = &cats[0];
        assert_eq!(c.icon_index, 18);
        assert_eq!(c.item_ids, vec![1, 2]);
    }
}

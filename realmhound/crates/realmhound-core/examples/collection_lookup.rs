//! Look up where items live in the Trophy Hall dungeon collections.
//!
//! For each requested item name, prints EVERY object that shares that display
//! name (a shiny shares its base's name) with its id, shiny flag, slot, labels
//! and the dungeon collection/section it currently belongs to. Used when a
//! patch adds shinies to existing dungeon UTs: the shiny's own id is not in
//! RealmEye's scrape, so it must be added to `EXTRA_ITEMS_OVERRIDE` (and, in a
//! sectioned dungeon, to `SECTION_OVERRIDE`) next to its base.
//!
//! Usage:
//!   cargo run -p realmhound-core --example collection_lookup -- [Item Name]...

use realmhound_core::assets::{find_assets_dir, get_asset_manager, ObjectList};
use realmhound_core::stats::dungeon_collection::build_all_collections;

fn main() {
    let mut names: Vec<String> = std::env::args().skip(1).collect();
    if names.is_empty() {
        // Season 31 Part 1 shinies (issue #52).
        names = [
            "Flowering Kimono",
            "Sage's Wakibiki",
            "Ethereal Happi",
            "Overwhelming Axehead",
            "Draconic Insignia",
            "Irradiance Sheath",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
    }

    let mgr = get_asset_manager();
    let dir = find_assets_dir();
    if let Some(d) = dir.as_ref() {
        println!("assets: {}", d.display());
        mgr.set_assets_dir(d);
    }
    if !mgr.try_load() {
        eprintln!("failed to load assets: {:?}", mgr.last_error());
        return;
    }
    let Some(list) = dir
        .as_ref()
        .and_then(|d| ObjectList::load_from_file(d.join("ObjectID.list")).ok())
    else {
        eprintln!("could not load ObjectID.list");
        return;
    };
    println!("objects: {}", list.len());

    // Every collection member, by id, remembering its dungeon (and section).
    let collections = build_all_collections();
    let mut members: Vec<(i32, String, Option<String>)> = Vec::new();
    for (dungeon, def) in &collections {
        for it in &def.items {
            members.push((it.item_id, dungeon.clone(), None));
        }
        if let Some(sections) = &def.sections {
            for section in sections {
                for it in &section.items {
                    members.push((it.item_id, dungeon.clone(), Some(section.name.clone())));
                }
            }
        }
    }
    let where_is = |id: i32| -> Vec<String> {
        members
            .iter()
            .filter(|(i, _, _)| *i == id)
            .map(|(_, d, s)| match s {
                Some(s) => format!("{d} / {s}"),
                None => d.clone(),
            })
            .collect()
    };

    for name in &names {
        println!("\n=== {name} ===");
        let mut found: Vec<(i32, bool, String, i32)> = Vec::new();
        for (id, obj) in list.iter_with_ids() {
            if obj.name() == name {
                found.push((*id, obj.is_shiny(), obj.labels.clone(), obj.slot_type));
            }
        }
        found.sort_by_key(|(id, _, _, _)| *id);
        if found.is_empty() {
            println!("  <no object with this display name>");
        }
        for (id, shiny, labels, slot) in &found {
            let places = where_is(*id);
            println!(
                "  id={id} (0x{id:x}) shiny={shiny} slot={slot} collections={}",
                if places.is_empty() {
                    "<none>".to_string()
                } else {
                    places.join(", ")
                }
            );
            println!("     labels: {labels}");
        }
    }
}

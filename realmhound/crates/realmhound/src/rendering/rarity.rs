//! Shared helpers for item rarity, expressed as enchant-slot counts (0-4).
//!
//! An item's rarity is derived from how many enchant slots it rolled:
//! 0 = Common, 1 = Uncommon, 2 = Rare, 3 = Legendary, 4 = Divine.

/// Rarity name for a given enchant slot-count (0-4).
pub fn rarity_name(slots: u8) -> &'static str {
    match slots {
        0 => "Common",
        1 => "Uncommon",
        2 => "Rare",
        3 => "Legendary",
        _ => "Divine",
    }
}

/// Items that only ever roll a subset of the usual 0-4 enchant-slot rarities
/// (e.g. items that skip straight from Common to Divine), keyed by item ID.
pub fn restricted_rarities(item_id: i32) -> Option<&'static [u8]> {
    match item_id {
        1215 => Some(&[0, 4]),     // Jewel Eye Katana Shiny
        65531 => Some(&[0, 4]),    // Kendo Stick Shiny
        14895 => Some(&[0, 4]),    // Nightmatter Circlet
        56463 => Some(&[1, 3, 4]), // Neo Reality Reactor (Uncommon, Legendary, Divine)
        _ => None,
    }
}

/// Forge tokens/upgrade artifacts and Moonlight Village fishing rods don't
/// have enchant slots, so the rarity tooltip is meaningless for them.
const NO_RARITY_ITEMS: &[i32] = &[
    23743, 5338, // Vial of Soul Extract + Shiny
    49473, 3984,  // Kogbold Enhancement Core + Shiny
    20796, // Concentrated Soul Fire
    56190, 56191, 56192, 56193, 56194, // Neo/Malogia/Katalund/Forax/Untaris Essence
    20703, 20722, 20723, 20726, 20810,
    3920, // Fishing Rods (Basic/Intermediate/Master/Legendary/Expert + Master Shiny)
    1159, // Locked Reactor
    42085, 42087, 42088, 42089, // Stone/Iron/Emerald/Diamond Pickaxe
];

/// The four gear-slot labels that identify an enchantable equippable item.
/// Non-gear "Equipment" items (marks, potions, forge materials, sulphur) lack
/// all of these, so they never show a rarity breakdown.
fn has_enchant_slot_label(labels: &str) -> bool {
    labels
        .split(',')
        .any(|l| matches!(l, "WEAPON" | "ABILITY" | "ARMOR" | "RING"))
}

/// Whether an item has no enchant slots and therefore no meaningful rarity.
pub fn has_no_enchant_slots(item_id: i32) -> bool {
    NO_RARITY_ITEMS.contains(&item_id)
}

/// Whether an item is enchantable gear that can carry a rarity. True only for
/// real equipment in one of the four gear slots, excluding manually-listed
/// non-enchantable gear (pickaxes, Locked Reactor).
pub fn is_enchantable(item_id: i32) -> bool {
    if has_no_enchant_slots(item_id) {
        return false;
    }
    realmhound_core::assets::get_asset_manager()
        .get_object(item_id)
        .map(|obj| obj.is_equipment() && has_enchant_slot_label(&obj.labels))
        .unwrap_or(false)
}

/// The set of enchant-slot rarities an item can roll, honoring per-item
/// restrictions. Returns an empty slice for items without enchant slots.
pub fn possible_rarities(item_id: i32) -> &'static [u8] {
    if has_no_enchant_slots(item_id) {
        &[]
    } else {
        restricted_rarities(item_id).unwrap_or(&[0, 1, 2, 3, 4])
    }
}

//! Inventory parsing from packet stat data.
//!
//! Pure data transformations that extract player inventory, pet inventory,
//! vault changes, and encounter spawns from packet status updates.

use crate::api::character::{parse_enchant_slots, EnchantSlots};
use crate::assets::get_asset_manager;
use crate::protocol::data::{ObjectStatusData, StatType};
use crate::vault::CharacterItem;

// ---------------------------------------------------------------------------
// Player inventory
// ---------------------------------------------------------------------------

/// Parsed player inventory from a NewTick status update.
#[derive(Debug, Clone)]
pub struct PlayerInventoryUpdate {
    /// Equipment slots (weapon, ability, armor, ring) -- indices 0..3
    pub equipment: Vec<Option<CharacterItem>>,
    /// Main inventory slots -- indices 4..11
    pub inventory: Vec<Option<CharacterItem>>,
    /// Backpack slots -- indices 12..19
    pub backpack: Vec<Option<CharacterItem>>,
    /// Extended backpack slots -- indices 20..27
    pub backpack_ext: Vec<Option<CharacterItem>>,
    /// Potion belt slots (up to 3)
    pub belt: Vec<Option<CharacterItem>>,
    /// Texture 1 override (if present in the status)
    pub tex1: Option<u32>,
    /// Texture 2 override (if present in the status)
    pub tex2: Option<u32>,
}

/// Check whether the given status contains any inventory slot stats.
pub fn has_inventory_stats(status: &ObjectStatusData) -> bool {
    status.stats.iter().any(|s| {
        matches!(
            s.stat_type,
            StatType::Inventory0
                | StatType::Inventory1
                | StatType::Inventory2
                | StatType::Inventory3
                | StatType::Inventory4
                | StatType::Inventory5
                | StatType::Inventory6
                | StatType::Inventory7
                | StatType::Inventory8
                | StatType::Inventory9
                | StatType::Inventory10
                | StatType::Inventory11
                | StatType::Backpack2_0
                | StatType::Backpack2_1
                | StatType::Backpack2_2
                | StatType::Backpack2_3
                | StatType::Backpack2_4
                | StatType::Backpack2_5
                | StatType::Backpack2_6
                | StatType::Backpack2_7
                | StatType::BackpackExt0
                | StatType::BackpackExt1
                | StatType::BackpackExt2
                | StatType::BackpackExt3
                | StatType::BackpackExt4
                | StatType::BackpackExt5
                | StatType::BackpackExt6
                | StatType::BackpackExt7
                | StatType::PotionOneType
                | StatType::PotionTwoType
                | StatType::PotionThreeType
        )
    })
}

/// Parse player inventory from a NewTick status update.
///
/// Extracts equipment, inventory, backpack, extended backpack, potion belt,
/// and texture overrides from the stat data. Returns `None` when no
/// inventory-related stats are present.
pub fn parse_player_inventory(status: &ObjectStatusData) -> Option<PlayerInventoryUpdate> {
    if !has_inventory_stats(status) {
        return None;
    }

    // Parse enchant data from UniqueDataString
    let unique_data = status
        .stats
        .iter()
        .find(|s| s.stat_type == StatType::UniqueDataString)
        .and_then(|s| s.string_stat_value.as_ref());

    let enchants_by_slot: Vec<EnchantSlots> = if let Some(data) = unique_data {
        data.split(',')
            .map(|part| {
                let trimmed = part.trim();
                if trimmed.is_empty() {
                    EnchantSlots::default()
                } else {
                    parse_enchant_slots(trimmed)
                }
            })
            .collect()
    } else {
        Vec::new()
    };

    let get_enchants =
        |slot: usize| -> EnchantSlots { enchants_by_slot.get(slot).cloned().unwrap_or_default() };

    let get_item = |stat_type: StatType, slot: usize| -> Option<CharacterItem> {
        status
            .stats
            .iter()
            .find(|s| s.stat_type == stat_type)
            .map(|s| CharacterItem::with_enchant_slots(s.stat_value, get_enchants(slot)))
    };

    let equipment = vec![
        get_item(StatType::Inventory0, 0),
        get_item(StatType::Inventory1, 1),
        get_item(StatType::Inventory2, 2),
        get_item(StatType::Inventory3, 3),
    ];
    let inventory = vec![
        get_item(StatType::Inventory4, 4),
        get_item(StatType::Inventory5, 5),
        get_item(StatType::Inventory6, 6),
        get_item(StatType::Inventory7, 7),
        get_item(StatType::Inventory8, 8),
        get_item(StatType::Inventory9, 9),
        get_item(StatType::Inventory10, 10),
        get_item(StatType::Inventory11, 11),
    ];
    let backpack = vec![
        get_item(StatType::Backpack2_0, 12),
        get_item(StatType::Backpack2_1, 13),
        get_item(StatType::Backpack2_2, 14),
        get_item(StatType::Backpack2_3, 15),
        get_item(StatType::Backpack2_4, 16),
        get_item(StatType::Backpack2_5, 17),
        get_item(StatType::Backpack2_6, 18),
        get_item(StatType::Backpack2_7, 19),
    ];
    let backpack_ext = vec![
        get_item(StatType::BackpackExt0, 20),
        get_item(StatType::BackpackExt1, 21),
        get_item(StatType::BackpackExt2, 22),
        get_item(StatType::BackpackExt3, 23),
        get_item(StatType::BackpackExt4, 24),
        get_item(StatType::BackpackExt5, 25),
        get_item(StatType::BackpackExt6, 26),
        get_item(StatType::BackpackExt7, 27),
    ];

    let get_belt_item = |stat_type: StatType| -> Option<CharacterItem> {
        status
            .stats
            .iter()
            .find(|s| s.stat_type == stat_type)
            .map(|s| {
                let stack_count = s.stat_value_two.max(1) as u8;
                CharacterItem::with_stack(s.stat_value, Vec::new(), stack_count)
            })
    };
    let belt = vec![
        get_belt_item(StatType::PotionOneType),
        get_belt_item(StatType::PotionTwoType),
        get_belt_item(StatType::PotionThreeType),
    ];

    let tex1 = status
        .stats
        .iter()
        .find(|s| s.stat_type == StatType::Texture1)
        .map(|s| s.stat_value as u32);
    let tex2 = status
        .stats
        .iter()
        .find(|s| s.stat_type == StatType::Texture2)
        .map(|s| s.stat_value as u32);

    Some(PlayerInventoryUpdate {
        equipment,
        inventory,
        backpack,
        backpack_ext,
        belt,
        tex1,
        tex2,
    })
}

/// Player appearance (skin + dyes) parsed from a player object status.
///
/// Each field is `Some` only when the corresponding stat is present in the
/// status, so applying it never overwrites a known value with a default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlayerAppearanceUpdate {
    /// Skin id override (`StatType::SkinId`); 0 is the default (no skin).
    pub skin: Option<i32>,
    /// Clothing dye/cloth texture (`StatType::Texture1`).
    pub tex1: Option<u32>,
    /// Accessory dye/cloth texture (`StatType::Texture2`).
    pub tex2: Option<u32>,
}

impl PlayerAppearanceUpdate {
    /// Whether any appearance field was observed in the status.
    pub fn is_empty(&self) -> bool {
        self.skin.is_none() && self.tex1.is_none() && self.tex2.is_none()
    }
}

/// Parse the local player's skin + dye textures from an object status.
///
/// Unlike [`parse_player_inventory`], this does not require inventory stats to be
/// present, so pure appearance-change ticks (and the full status delivered on the
/// Update packet) are both handled.
pub fn parse_player_appearance(status: &ObjectStatusData) -> PlayerAppearanceUpdate {
    let find = |st: StatType| status.stats.iter().find(|s| s.stat_type == st);
    PlayerAppearanceUpdate {
        skin: find(StatType::SkinId).map(|s| s.stat_value),
        tex1: find(StatType::Texture1).map(|s| s.stat_value as u32),
        tex2: find(StatType::Texture2).map(|s| s.stat_value as u32),
    }
}

// ---------------------------------------------------------------------------
// Pet inventory
// ---------------------------------------------------------------------------

/// Pet identity fields learned live from a pet object's status stats. Each
/// field is `None` when the corresponding stat is absent (NewTick deltas only
/// carry changed stats). Used to enrich a live-created pet so the character
/// card can render its sprite/name before an API refresh.
#[derive(Debug, Clone, Default)]
pub struct PetIdentity {
    /// Pet display name (`PetName`).
    pub name: Option<String>,
    /// Pet skin object id (`SkinId`), used for sprite rendering when > 0.
    pub skin: Option<i32>,
    /// Pet type id (`PetType`), the sprite fallback when skin is 0.
    pub pet_type: Option<i32>,
    /// Pet rarity (`PetRarity`).
    pub rarity: Option<i32>,
    /// Maximum ability power (`PetMaxAbilityPower`).
    pub max_ability_power: Option<i32>,
}

impl PetIdentity {
    /// Whether any identity field is present.
    pub fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.skin.is_none()
            && self.pet_type.is_none()
            && self.rarity.is_none()
            && self.max_ability_power.is_none()
    }
}

/// Extract pet identity fields (name, skin, type, rarity, ability power) from a
/// pet object's status. Present in full in the object's spawn `Update`; later
/// NewTicks only carry changed fields.
pub fn extract_pet_identity(status: &ObjectStatusData) -> PetIdentity {
    let find = |t: StatType| status.stats.iter().find(|s| s.stat_type == t);
    PetIdentity {
        name: find(StatType::PetName).and_then(|s| s.string_stat_value.clone()),
        skin: find(StatType::SkinId).map(|s| s.stat_value),
        pet_type: find(StatType::PetType).map(|s| s.stat_value),
        rarity: find(StatType::PetRarity).map(|s| s.stat_value),
        max_ability_power: find(StatType::PetMaxAbilityPower).map(|s| s.stat_value),
    }
}

/// A single pet inventory slot update.
#[derive(Debug, Clone)]
pub struct PetSlotUpdate {
    /// Slot index (0..7)
    pub slot: usize,
    /// Item type ID
    pub item_id: i32,
    /// Stack count (0 if not stacked)
    pub stack_count: u8,
}

/// Extract pet inventory slot updates from a NewTick status.
///
/// Returns an empty vec if the status doesn't contain pet inventory stats.
pub fn extract_pet_inventory_updates(status: &ObjectStatusData) -> Vec<PetSlotUpdate> {
    status
        .stats
        .iter()
        .filter_map(|s| {
            let slot = match s.stat_type {
                StatType::Inventory0 => Some(0),
                StatType::Inventory1 => Some(1),
                StatType::Inventory2 => Some(2),
                StatType::Inventory3 => Some(3),
                StatType::Inventory4 => Some(4),
                StatType::Inventory5 => Some(5),
                StatType::Inventory6 => Some(6),
                StatType::Inventory7 => Some(7),
                _ => None,
            };
            slot.map(|slot| PetSlotUpdate {
                slot,
                item_id: s.stat_value,
                stack_count: if s.stat_value_two > 0 {
                    s.stat_value_two as u8
                } else {
                    0
                },
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Vault chest inventory changes
// ---------------------------------------------------------------------------

/// A single vault chest slot change (slot index + new item ID).
pub type VaultSlotChange = (usize, i32);

/// Extract inventory changes from a vault chest status update.
///
/// Returns `(slot_index, item_id)` pairs for each `Inventory0..7` stat present.
pub fn extract_inventory_changes(status: &ObjectStatusData) -> Vec<VaultSlotChange> {
    status
        .stats
        .iter()
        .filter_map(|s| {
            let slot = match s.stat_type {
                StatType::Inventory0 => Some(0),
                StatType::Inventory1 => Some(1),
                StatType::Inventory2 => Some(2),
                StatType::Inventory3 => Some(3),
                StatType::Inventory4 => Some(4),
                StatType::Inventory5 => Some(5),
                StatType::Inventory6 => Some(6),
                StatType::Inventory7 => Some(7),
                _ => None,
            };
            slot.map(|idx| (idx, s.stat_value))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Encounter detection
// ---------------------------------------------------------------------------

/// An encounter entity that spawned in the game world.
#[derive(Debug, Clone)]
pub struct EncounterSpawn {
    /// Object ID of the encounter entity
    pub object_id: i32,
    /// Sprite ID for rendering
    pub sprite_id: u16,
    /// Display name of the encounter
    pub name: String,
}

/// Extract encounter spawns from Update packet new-object list.
///
/// Checks each new object against the asset manager to determine if it's an
/// encounter entity, returning display info for each match.
pub fn extract_encounter_spawns(
    new_objects: &[crate::protocol::data::ObjectData],
) -> Vec<EncounterSpawn> {
    let mgr = get_asset_manager();
    new_objects
        .iter()
        .filter_map(|obj| {
            let type_id = obj.object_type as i32;
            if mgr.is_encounter(type_id) {
                mgr.encounter_display_info(type_id)
                    .map(|(name, sprite_id)| EncounterSpawn {
                        object_id: obj.object_id(),
                        sprite_id: sprite_id as u16,
                        name,
                    })
            } else {
                None
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Account identity extraction (from Update packet stats)
// ---------------------------------------------------------------------------

/// Account identity data extracted from a player object's stats.
#[derive(Debug, Clone, Default)]
pub struct AccountIdentity {
    /// Account ID string (if AccountId stat is present)
    pub account_id: Option<String>,
    /// Account name string (if Name stat is present)
    pub account_name: Option<String>,
}

/// Extract account identity from a player object's stat data.
pub fn extract_account_identity(status: &ObjectStatusData) -> AccountIdentity {
    let account_id = status
        .stats
        .iter()
        .find(|s| s.stat_type == StatType::AccountId)
        .and_then(|s| s.string_stat_value.clone());
    let account_name = status
        .stats
        .iter()
        .find(|s| s.stat_type == StatType::Name)
        .and_then(|s| s.string_stat_value.clone());
    AccountIdentity {
        account_id,
        account_name,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::data::{StatData, WorldPosData};

    fn make_status(stats: Vec<StatData>) -> ObjectStatusData {
        ObjectStatusData {
            object_id: 1,
            pos: WorldPosData { x: 0.0, y: 0.0 },
            stats,
        }
    }

    fn stat(stat_type: StatType, value: i32) -> StatData {
        StatData {
            stat_type_id: 0,
            stat_type,
            stat_value: value,
            stat_value_two: 0,
            string_stat_value: None,
        }
    }

    fn stat_with_two(stat_type: StatType, value: i32, value_two: i32) -> StatData {
        StatData {
            stat_type_id: 0,
            stat_type,
            stat_value: value,
            stat_value_two: value_two,
            string_stat_value: None,
        }
    }

    fn string_stat(stat_type: StatType, value: &str) -> StatData {
        StatData {
            stat_type_id: 0,
            stat_type,
            stat_value: 0,
            stat_value_two: 0,
            string_stat_value: Some(value.to_string()),
        }
    }

    // -- has_inventory_stats --

    #[test]
    fn has_inventory_stats_returns_true_for_equipment() {
        let status = make_status(vec![stat(StatType::Inventory0, 100)]);
        assert!(has_inventory_stats(&status));
    }

    #[test]
    fn has_inventory_stats_returns_true_for_backpack() {
        let status = make_status(vec![stat(StatType::Backpack2_0, 200)]);
        assert!(has_inventory_stats(&status));
    }

    #[test]
    fn has_inventory_stats_returns_true_for_potion_belt() {
        let status = make_status(vec![stat(StatType::PotionOneType, 300)]);
        assert!(has_inventory_stats(&status));
    }

    #[test]
    fn has_inventory_stats_returns_false_for_non_inventory() {
        let status = make_status(vec![stat(StatType::MaxHP, 500)]);
        assert!(!has_inventory_stats(&status));
    }

    // -- parse_player_inventory --

    #[test]
    fn parse_player_inventory_returns_none_for_no_inventory() {
        let status = make_status(vec![stat(StatType::MaxHP, 500)]);
        assert!(parse_player_inventory(&status).is_none());
    }

    #[test]
    fn parse_player_inventory_extracts_equipment() {
        let status = make_status(vec![
            stat(StatType::Inventory0, 0x0A01),
            stat(StatType::Inventory1, 0x0A02),
        ]);
        let update = parse_player_inventory(&status).unwrap();
        assert!(update.equipment[0].is_some());
        assert_eq!(update.equipment[0].as_ref().unwrap().item_id, 0x0A01);
        assert!(update.equipment[1].is_some());
        assert_eq!(update.equipment[1].as_ref().unwrap().item_id, 0x0A02);
        assert!(update.equipment[2].is_none());
        assert!(update.equipment[3].is_none());
    }

    #[test]
    fn parse_player_inventory_extracts_belt_with_stack() {
        let status = make_status(vec![stat_with_two(StatType::PotionOneType, 0x0B01, 3)]);
        let update = parse_player_inventory(&status).unwrap();
        assert!(update.belt[0].is_some());
        let belt_item = update.belt[0].as_ref().unwrap();
        assert_eq!(belt_item.item_id, 0x0B01);
        assert_eq!(belt_item.stack_count, 3);
    }

    #[test]
    fn parse_player_inventory_extracts_textures() {
        let status = make_status(vec![
            stat(StatType::Inventory0, 100), // need at least one inventory stat
            stat(StatType::Texture1, 42),
            stat(StatType::Texture2, 99),
        ]);
        let update = parse_player_inventory(&status).unwrap();
        assert_eq!(update.tex1, Some(42));
        assert_eq!(update.tex2, Some(99));
    }

    #[test]
    fn parse_player_appearance_extracts_skin_and_dyes() {
        let status = make_status(vec![
            stat(StatType::SkinId, 5678),
            stat(StatType::Texture1, 42),
            stat(StatType::Texture2, 99),
        ]);
        let app = parse_player_appearance(&status);
        assert_eq!(app.skin, Some(5678));
        assert_eq!(app.tex1, Some(42));
        assert_eq!(app.tex2, Some(99));
    }

    #[test]
    fn parse_player_appearance_absent_fields_are_none() {
        // No inventory stats present, only a skin change: still parsed.
        let status = make_status(vec![stat(StatType::SkinId, 7)]);
        let app = parse_player_appearance(&status);
        assert_eq!(app.skin, Some(7));
        assert_eq!(app.tex1, None);
        assert_eq!(app.tex2, None);
        assert!(!app.is_empty());

        let empty = parse_player_appearance(&make_status(vec![stat(StatType::MaxHP, 1)]));
        assert!(empty.is_empty());
    }

    // -- extract_pet_inventory_updates --

    #[test]
    fn extract_pet_inventory_returns_slots() {
        let status = make_status(vec![
            stat_with_two(StatType::Inventory0, 500, 2),
            stat_with_two(StatType::Inventory3, 600, 0),
        ]);
        let updates = extract_pet_inventory_updates(&status);
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].slot, 0);
        assert_eq!(updates[0].item_id, 500);
        assert_eq!(updates[0].stack_count, 2);
        assert_eq!(updates[1].slot, 3);
        assert_eq!(updates[1].item_id, 600);
        assert_eq!(updates[1].stack_count, 0);
    }

    #[test]
    fn extract_pet_inventory_ignores_non_inventory_stats() {
        let status = make_status(vec![stat(StatType::MaxHP, 500)]);
        let updates = extract_pet_inventory_updates(&status);
        assert!(updates.is_empty());
    }

    // -- extract_inventory_changes --

    #[test]
    fn extract_inventory_changes_extracts_slot_changes() {
        let status = make_status(vec![
            stat(StatType::Inventory0, 100),
            stat(StatType::Inventory5, 200),
            stat(StatType::MaxHP, 999), // should be ignored
        ]);
        let changes = extract_inventory_changes(&status);
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0], (0, 100));
        assert_eq!(changes[1], (5, 200));
    }

    // -- extract_account_identity --

    #[test]
    fn extract_account_identity_extracts_id_and_name() {
        let status = make_status(vec![
            string_stat(StatType::AccountId, "abc123"),
            string_stat(StatType::Name, "Player1"),
        ]);
        let identity = extract_account_identity(&status);
        assert_eq!(identity.account_id, Some("abc123".to_string()));
        assert_eq!(identity.account_name, Some("Player1".to_string()));
    }

    #[test]
    fn extract_account_identity_returns_none_when_missing() {
        let status = make_status(vec![stat(StatType::MaxHP, 500)]);
        let identity = extract_account_identity(&status);
        assert_eq!(identity.account_id, None);
        assert_eq!(identity.account_name, None);
    }

    // -- extract_pet_identity --

    #[test]
    fn extract_pet_identity_reads_present_fields() {
        let status = make_status(vec![
            string_stat(StatType::PetName, "Mini Sphinx"),
            stat(StatType::PetType, 4444),
            stat(StatType::SkinId, 0),
            stat(StatType::PetRarity, 4),
            stat(StatType::PetMaxAbilityPower, 100),
        ]);
        let id = extract_pet_identity(&status);
        assert_eq!(id.name.as_deref(), Some("Mini Sphinx"));
        assert_eq!(id.pet_type, Some(4444));
        assert_eq!(id.skin, Some(0));
        assert_eq!(id.rarity, Some(4));
        assert_eq!(id.max_ability_power, Some(100));
        assert!(!id.is_empty());
    }

    #[test]
    fn extract_pet_identity_empty_when_no_pet_stats() {
        let status = make_status(vec![stat(StatType::MaxHP, 500)]);
        let id = extract_pet_identity(&status);
        assert!(id.is_empty());
    }
}

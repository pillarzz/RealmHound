//! Tab icon constants for the main UI tabs.
//!
//! Maps each tab to a sprite from the game assets. Icons are rendered
//! at 16x16 pixels in the tab bar.

use crate::panels::treasury::TreasurySection;
use crate::panels::vault::VaultSubTab;
use crate::panels::ActiveTab;
use crate::rendering::EmbeddedIcon;

/// Sprite reference for tab icons.
///
/// Some tabs use object IDs (which have registered sprites in ObjectID.list),
/// while others use direct sheet+index references (for player discovery icons
/// that have low type IDs that conflict with other objects).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabIconSprite {
    /// Use object ID to look up sprite (e.g., White Bag = 1292)
    ObjectId(i32),
    /// Like [`TabIconSprite::ObjectId`] but the sprite is upscaled (fractional)
    /// to fill the icon box, for portal art whose native size is physically
    /// large and would otherwise render small next to 8x8-based icons.
    ObjectIdFilled(i32),
    /// Use direct sheet name and index (e.g., lofiObj, 502)
    SheetIndex(&'static str, i32),
    /// Use embedded PNG icon
    Embedded(EmbeddedIcon),
}

/// Tab icon definitions.
///
/// Each tab has an associated sprite from the game assets:
/// - Live Feed: Nexus Crystal (embedded PNG, not in the atlas)
/// - Loot History: White Bag (object 1292)
/// - Party: Valentine's Candy Heart (lofiObj:818)
/// - Vault: Vault Portal (object 1824)
/// - Quests: Daily Quest Portal (object 5974)
/// - Characters: Char Slot Unlocker (lofiObj3:0x5a0)
/// - Treasury: Secret Hoard (14758)
/// - Chat: Player discovery "chat" icon (lofiObj:515)
pub fn get_tab_icon(tab: ActiveTab) -> TabIconSprite {
    match tab {
        ActiveTab::LiveFeed => TabIconSprite::Embedded(EmbeddedIcon::NexusCrystal), // Nexus Crystal
        ActiveTab::LootHistory => TabIconSprite::ObjectId(1292),                    // White Bag
        ActiveTab::TrophyHall => TabIconSprite::ObjectId(25991), // Abyssal Trophy
        ActiveTab::Party => TabIconSprite::Embedded(EmbeddedIcon::Party), // Party.png
        ActiveTab::Vault => TabIconSprite::ObjectId(1824),       // Vault Portal
        ActiveTab::Quests => TabIconSprite::ObjectId(5974),      // Daily Quest Portal
        ActiveTab::Characters => TabIconSprite::ObjectId(810),   // Char Slot Unlocker
        ActiveTab::Exaltations => exalt_trophy_icon(),           // Exalt Trophy (resolved by name)
        ActiveTab::Treasury => TabIconSprite::ObjectId(14758),   // Secret Hoard
        ActiveTab::Chat => TabIconSprite::SheetIndex("lofiObj", 515), // Player discovery: chat
        ActiveTab::CombatHistory => TabIconSprite::ObjectIdFilled(1883), // Arena Portal (classic)
        ActiveTab::PetYard => TabIconSprite::SheetIndex("lofiObj3", 0x0d), // Pet Yard Portal
        ActiveTab::Missions => TabIconSprite::ObjectId(0x68a5),  // Seasonal Apple
    }
}

/// Resolve the "Exalt Trophy" object id by name (assets are extracted at
/// runtime, so the id isn't fixed). Falls back to Potion of Attack until the
/// object list is loaded.
fn exalt_trophy_icon() -> TabIconSprite {
    let mgr = realmhound_core::assets::get_asset_manager();
    mgr.object_id_for_name("Exalt Trophy")
        .or_else(|| mgr.object_id_for_display_name("Exalt Trophy"))
        .map(TabIconSprite::ObjectId)
        .unwrap_or(TabIconSprite::ObjectId(2591))
}

/// Vault sub-tab icon definitions.
///
/// Each sub-tab has an associated sprite:
/// - Vault: Vault Chest Unlocker (lofiObj3:0x5a1)
/// - Gifts: Gift Chest (lofiObj2:0x7f = 127)
/// - Storage: Stat Potion Choice Chest (d3LofiObjEmbed:571)
/// - Spoils: Seasonal Spoils chest (lofiObj2:354)
pub fn get_vault_subtab_icon(tab: VaultSubTab, _is_seasonal: bool) -> Option<TabIconSprite> {
    match tab {
        VaultSubTab::Vault => Some(TabIconSprite::SheetIndex("lofiObj3", 0x5a1)), // Vault Chest Unlocker
        VaultSubTab::Gifts => Some(TabIconSprite::SheetIndex("lofiObj2", 0x7f)),  // Gift Chest
        VaultSubTab::Storage => Some(TabIconSprite::SheetIndex("d3LofiObjEmbed", 571)), // Stat Potion Choice Chest
        VaultSubTab::Spoils => Some(TabIconSprite::SheetIndex("lofiObj2", 354)), // Seasonal Spoils
    }
}

/// Treasury section icon definitions.
///
/// Each section has an associated sprite:
/// - Characters: Char Slot Unlocker (810)
/// - Vault: Vault Chest Unlocker (lofiObj3:0x5a1)
/// - Materials/Potions: Stat Potion Choice Chest (d3LofiObjEmbed:571)
/// - Pet Inventories: Pet Yard Portal (lofiObj3:0x0d)
/// - Gift Chests: Gift Chest (lofiObj2:0x7f)
/// - Spoils: Seasonal Spoils chest (lofiObj2:354)
pub fn get_treasury_section_icon(section: TreasurySection) -> TabIconSprite {
    match section {
        TreasurySection::SeasonalCharacters => {
            TabIconSprite::Embedded(EmbeddedIcon::SeasonalChar) // SeasonalChar.png
        }
        TreasurySection::RegularCharacters => {
            TabIconSprite::ObjectId(810) // Char Slot Unlocker
        }
        TreasurySection::SeasonalVault | TreasurySection::RegularVault => {
            TabIconSprite::SheetIndex("lofiObj3", 0x5a1) // Vault Chest Unlocker
        }
        TreasurySection::SeasonalMaterials | TreasurySection::RegularMaterials => {
            TabIconSprite::SheetIndex("lofiObj3", 0xBBE) // Forge Chest
        }
        TreasurySection::SeasonalPotionRack | TreasurySection::RegularPotionRack => {
            TabIconSprite::SheetIndex("d3LofiObjEmbed", 571) // Stat Potion Choice Chest
        }
        TreasurySection::SeasonalPetInventories | TreasurySection::RegularPetInventories => {
            TabIconSprite::SheetIndex("lofiObj3", 0x0d) // Pet Yard Portal
        }
        TreasurySection::SeasonalGiftChests | TreasurySection::RegularGiftChest => {
            TabIconSprite::SheetIndex("lofiObj2", 0x7f) // Gift Chest
        }
        TreasurySection::SeasonalSpoils => {
            TabIconSprite::SheetIndex("lofiObj2", 354) // Seasonal Spoils
        }
    }
}

/// Get character sub-tab icon (Regular vs Seasonal).
pub fn get_character_subtab_icon(is_seasonal: bool) -> TabIconSprite {
    if is_seasonal {
        TabIconSprite::Embedded(EmbeddedIcon::SeasonalChar)
    } else {
        TabIconSprite::ObjectId(810) // Char Slot Unlocker
    }
}

/// Map a character's maxed-stat count and level to the default gravestone
/// object id (production RotMG "Default Gravestone" set, ObjectID.list 1827-1837).
///
/// - 1..=8 maxed -> `1830`..`1837` (1/8 .. 8/8)
/// - 0 maxed -> level-based: 1827 (Lv.1), 1828 (Lv.2-19), 1829 (Lv.20+)
fn default_gravestone_id(maxed_count: u8, level: i32) -> i32 {
    match maxed_count {
        1..=8 => 1829 + maxed_count as i32,
        _ => {
            if level <= 1 {
                1827
            } else if level < 20 {
                1828
            } else {
                1829
            }
        }
    }
}

/// Grave sprite for a dead character.
///
/// Prefers the authoritative death-packet `gravestone_type` (the exact grave
/// skin the server spawned). Falls back to the default gravestone derived from
/// maxed-stat count + level for offline/API-missing deaths.
pub fn grave_sprite_for_char(
    gravestone_type: Option<i32>,
    maxed_count: u8,
    level: i32,
) -> TabIconSprite {
    if let Some(t) = gravestone_type {
        if t > 0 {
            return TabIconSprite::ObjectId(t);
        }
    }
    TabIconSprite::ObjectId(default_gravestone_id(maxed_count, level))
}

/// Graveyard sub-tab icon: a Tier 3 Default Gravestone (object 1832, the 3/8
/// grave in the 1830-1837 progression).
pub fn get_graveyard_subtab_icon() -> TabIconSprite {
    TabIconSprite::ObjectId(1832)
}

/// Get vault top-level tab icon (Regular vs Seasonal).
pub fn get_vault_top_tab_icon(is_seasonal: bool) -> TabIconSprite {
    if is_seasonal {
        TabIconSprite::SheetIndex("lofiObj2", 354) // Seasonal Spoils
    } else {
        TabIconSprite::SheetIndex("lofiObj3", 0x5a1) // Vault Chest Unlocker
    }
}

/// Section icons for the Vault "Materials & Potions" tab. These mirror the
/// matching Treasury section icons so both pages stay visually consistent.
pub fn get_vault_materials_icon() -> TabIconSprite {
    TabIconSprite::SheetIndex("lofiObj3", 0xBBE) // Forge Chest
}

pub fn get_vault_potions_icon() -> TabIconSprite {
    TabIconSprite::SheetIndex("d3LofiObjEmbed", 571) // Stat Potion Choice Chest
}

pub fn get_vault_pet_inventories_icon() -> TabIconSprite {
    TabIconSprite::SheetIndex("lofiObj3", 0x0d) // Pet Yard Portal
}

/// Icon size in logical pixels (before DPI scaling).
pub const TAB_ICON_SIZE: f32 = 24.0;

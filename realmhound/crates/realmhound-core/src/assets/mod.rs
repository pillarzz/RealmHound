//! Asset management module for RotMG game assets.
//!
//! This module provides functionality to load and cache game assets including:
//! - Object definitions (items, enemies, NPCs) with names and textures
//! - Tile definitions with texture data
//! - Sprite atlases for rendering item icons
//! - Enchantment definitions for item enchant lookups
//!
//! # Performance
//!
//! The asset system is designed for high performance with thousands of items:
//! - Lazy loading: Assets are only loaded when first accessed
//! - Single load: Assets are loaded once and cached globally
//! - Efficient lookups: HashMap-based O(1) lookups by item ID
//! - Texture batching: Sprite atlas textures are shared across items
//!
//! # Usage
//!
//! ```ignore
//! use realmhound_core::assets::get_asset_manager;
//!
//! // Initialize on startup (extracts from game if needed)
//! let manager = get_asset_manager();
//! manager.initialize().expect("Failed to load assets");
//!
//! // Look up item names
//! if let Some(name) = manager.object_name(0x0b25) {
//!     println!("Item: {}", name);
//! }
//!
//! // Look up enchantment names
//! if let Some(name) = manager.enchant_name(279) {
//!     println!("Enchant: {}", name);
//! }
//! ```

mod boss_group;
mod dungeon_category;
mod dungeon_data;
pub mod dungeon_drops;
mod dungeon_mods_xml;
mod dungeon_portals;
mod enchantments;
pub mod item_category;
mod manager;
mod object_list;
pub mod realmeye_drops;
mod sprite_atlas;
mod stat_bonus;
pub mod unity;

pub use boss_group::{
    boss_exempt_from_flawless, boss_group, dungeon_for_mark_name, encounter_spawn_limited,
    normalize_train_sprite, BossGroup, CatalogEntry, KOGBOLD_TRAIN_DISPLAY_NAME,
    KOGBOLD_TRAIN_LOCOMOTIVE_SPRITE, SEASONAL_NOTIFY_EXCLUDED_IDS,
};
pub use dungeon_category::{build_categories, DungeonCategory};
pub use dungeon_data::dungeon_difficulty;
pub use dungeon_data::dungeons_in_difficulty_range;
pub use dungeon_data::{difficulty_tier, key_pop_tier, KeyPopTier};
pub use dungeon_drops::{
    get_dungeon_drops, BiomeSection, BiomeTier, DropCategory, DungeonDropData, DungeonDropSource,
};
pub use dungeon_mods_xml::{ModifierDef, ModifierTable};
pub use dungeon_portals::{
    get_dungeon_portal_map, legacy_embed_portal_index, DungeonPortalMap, LEGACY_EMBED_PORTAL_BASE,
    LEGACY_EMBED_PORTAL_COUNT,
};
pub use enchantments::{
    EnchantCatalogEntry, EnchantEffect, EnchantEffectKind, EnchantmentDef, EnchantmentList,
    EnchantmentTier,
};
pub use item_category::{slot_type_from_name, ItemCategorizer, ItemCategory, ItemSubCategory};
pub use manager::{
    aux_category_hides_count, aux_target_for_type, boss_for_loot_emitter,
    canonical_dungeon_for_boss, default_assets_dir, encounter_by_id, encounter_completion_all,
    encounter_completion_any, encounter_for_boss_type, encounter_headline_only,
    encounter_ids_matching_name, encounter_loot_completes, encounter_realm_grouped,
    encounter_supports_loot_completion, find_assets_dir, get_asset_manager,
    get_resources_assets_stamp, is_dedup_prone_boss, is_invuln_finish_boss,
    is_legacy_lod_ivory_boss, is_optional_secondary_boss_type, is_post_boss_bonus_type,
    is_second_coming_boss, is_second_coming_transition_taunt, is_treasure_crate_type,
    lod_dragon_chest_pairs, loot_emitter_for_boss, parse_grave_tier, prismimic_display_name,
    AssetManager, AssetStats, AuxTarget, CharacterDyeInfo, DyeInfo, DyeStyle, Encounter, GraveTier,
    ASSET_MANAGER, LEGACY_LOD_IVORY_BOSS,
};
pub use object_list::{
    AbilityEffect, AbilityEffects, BleedingEffect, ConditionSelfEffect, DamageNovaEffect,
    DetonateHexEffect, HexThresholdPoison, LethalStrikeParams, LightningEffect, ObjectAsset,
    ObjectList, ObjectTexture, PoisonGrenadeEffect, ProcEffect, ProcTrigger, ProjectileBurstEffect,
    RestoreEffect, ScalingStat, StatBoostEffect, TexInfo, TileAsset, TileList, TrapEffect,
    VampireBlastEffect, WeaponProc,
};
pub use realmeye_drops::{
    get_realmeye_drops, is_forge_crafted, is_legendary_fishing_rod,
    is_plagued_nest_beehemoth_quiver, is_soulful_affection, source_name_override, DropLocationType,
    DropSource, RealmEyeDropData, FISHING_AWARD_ITEM_ID, GEM_OF_ADORATION_ITEM_ID,
    GEM_OF_TENDERNESS_ITEM_ID, KILLER_BEE_QUEEN_OBJECT_ID, MV_FISHING_LOOT_OBJECT_ID,
    PLAGUED_NEST_PORTAL_OBJECT_ID,
};
pub use sprite_atlas::{MaskPosition, SpriteAtlas, SpriteData};
pub use stat_bonus::{StatBonuses, StatKind};
pub use unity::{find_resources_assets, generate_asset_lists, ExtractionResult, UnityExtractor};

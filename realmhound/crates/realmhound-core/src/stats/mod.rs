//! Statistics tracking for network capture.
//!
//! This module provides bandwidth and packet statistics tracking,
//! including rolling window calculations for bytes-per-second,
//! and inventory parsing from packet stat data.

mod bandwidth;
pub mod collections;
pub mod derived;
pub mod dungeon_collection;
pub mod dungeon_registry;
pub mod dungeon_snapshot;
pub mod exalt_proficiency;
pub mod fame_bonus;
pub mod inventory;

pub use bandwidth::BandwidthStats;
pub use collections::{
    dungeon_tracked, get_dungeon_list, Dungeon, DungeonCollection, ALL_DUNGEONS,
    DUNGEON_COLLECTIONS, UNTRACKED_DUNGEON_TOOLTIP,
};
pub use dungeon_collection::{
    build_all_collections, build_collection_for_dungeon, is_sectioned_dungeon, CollectionItem,
    CollectionSection, DungeonCollectionDef, ItemGroup,
};
pub use dungeon_registry::{get_dungeon_registry, DungeonRegistry};
pub use dungeon_snapshot::{build_dungeon_snapshot, DungeonSnapshotRow, SnapshotItem};
pub use fame_bonus::{
    biome_first_tier_bonus, biome_kill_bonus, biome_tier_breakdown, collection_bonus,
    dead_fame_breakdown, dead_fame_total, dungeon_awards_fame, dungeon_completion_bonus,
    dungeon_first_tier_bonus, dungeon_tier_breakdown, stat_first_tier_bonus, stat_line_bonus,
    stat_tier_breakdown, FameBonusBreakdown, FameBonusLine, FameTier, FameTierBreakdown,
};
pub use inventory::{
    extract_account_identity, extract_encounter_spawns, extract_inventory_changes,
    extract_pet_identity, extract_pet_inventory_updates, has_inventory_stats,
    parse_player_appearance, parse_player_inventory, AccountIdentity, EncounterSpawn, PetIdentity,
    PetSlotUpdate, PlayerAppearanceUpdate, PlayerInventoryUpdate, VaultSlotChange,
};

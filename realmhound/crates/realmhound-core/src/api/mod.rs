//! RotMG API client for fetching character data.
//!
//! This module provides functions to call RotMG's web API using an access token
//! captured from game traffic.

pub mod character;
mod client;
pub mod crucible;
pub mod missions;
pub mod token;

pub use character::{
    accelerator_info, debug_order_alignment, decode_pcstats, decode_pcstats_debug,
    get_unmapped_pcstats, parse_accelerators, parse_account_data, parse_char_list,
    pet_ability_name, AcceleratorInfo, AccountAccelerator, AccountData, AccountVault, BiomeStats,
    CharacterClass, CharacterStats, ClassExaltation, DungeonStats, GiftStorage, MaterialStorage,
    Pet, PetAbility, PetInventoryItem, PotionStorage, RealmCharacter, StorageItem, VaultChest,
};
pub use client::{ApiError, RotmgApiClient};
pub use crucible::{parse_crucible_defs, CrucibleDef, CrucibleEffect, CrucibleStatMod};
pub use missions::{
    is_vortex_target, parse_client_seasons, parse_player_missions, parse_prog_string,
    ClientSeasons, Cond, CondKind, MissionDef, MissionState, PlayerMission, PlayerMissions,
    ProgEntry, Reward, Season, WornRestriction,
};
pub use token::{
    is_token_expired, load_saved_token, load_saved_token_full, parse_saved_token_strict,
    save_token, token_age, token_expiry_message, SavedToken, TokenParse, TOKEN_VALIDITY_HOURS,
};

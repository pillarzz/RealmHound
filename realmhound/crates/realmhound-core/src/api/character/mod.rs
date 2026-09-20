//! Character data structures and API response parsing.
//!
//! This module provides types for representing RotMG character data
//! parsed from the web API XML responses.

mod character;
mod parser;
mod pcstats;
mod storage;

pub use character::{
    pet_ability_name, CharacterClass, ClassExaltation, Pet, PetAbility, PetInventoryItem,
    RealmCharacter,
};
pub use parser::{parse_account_data, parse_char_list};
pub use pcstats::{
    debug_order_alignment, decode_pcstats, decode_pcstats_debug, get_unmapped_pcstats, BiomeStats,
    CharacterStats, DungeonStats,
};
pub use storage::{
    accelerator_info, parse_accelerators, parse_enchant_ids, parse_enchant_slots, AcceleratorInfo,
    AccountAccelerator, AccountData, AccountVault, EnchantSlots, GiftStorage, MaterialStorage,
    PotionStorage, StorageItem, VaultChest,
};

//! Vault and character cache data from packet capture.
//!
//! This module provides storage for vault data captured from VaultContentPacket,
//! keeping separate vaults for regular and seasonal characters.
//! It also provides character caching for the Characters Panel.
//!
//! The `AccountData` struct is the unified source of truth for all account data,
//! consolidating characters and vault storage into a single persistent store.

mod account_data;
mod character_cache;
mod live_vault;

pub use account_data::{
    parse_legacy_character_cache, parse_legacy_vault, AccountData, AccountDataRepository,
    LegacyAccountInputs,
};
pub use character_cache::{
    CachedCharacter, CachedPet, CachedPetAbility, CachedPetItem, CachedStats, CharacterCache,
    CharacterItem, CloseCallSync, StatAwardSync,
};
pub use live_vault::{LiveVaultData, LiveVaultItem, LiveVaultStorage, ParsedEnchants, VaultType};

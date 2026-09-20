//! Unified account data store - single source of truth for all account data.
//!
//! This module consolidates character and vault data into a single store that:
//! - All data sources (API, packets) write to
//! - All panels (Characters, Vault, Treasury) read from
//! - Persists to a single JSON file with migration from legacy files

use super::{CharacterCache, LiveVaultItem, LiveVaultStorage, VaultType};
use crate::api::character::{parse_enchant_ids, RealmCharacter};
use crate::api::AccountData as ApiAccountData;
use crate::dust::DustAmounts;
use crate::protocol::VaultContentPacket;
use crate::storage::{load_json_at, write_json_atomic_at, BackupPolicy, LoadOutcome, StorageError};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Serde module for Option<SystemTime> (serialized as Unix millis).
mod option_system_time {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    pub fn serialize<S>(opt: &Option<SystemTime>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match opt {
            Some(time) => {
                let millis = time
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                Some(millis).serialize(serializer)
            }
            None => Option::<u64>::None.serialize(serializer),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<SystemTime>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<u64> = Option::deserialize(deserializer)?;
        Ok(opt.map(|millis| UNIX_EPOCH + Duration::from_millis(millis)))
    }
}

/// Unified account data store containing all character and vault data.
///
/// This is the single source of truth for all account-related data.
/// Panels read from this store; packet handlers and API results write to it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountData {
    /// Schema version for future migrations
    pub version: u32,

    /// Generation counter - increments on any data change.
    /// Used by UI panels to detect when to recompute derived data.
    #[serde(default)]
    pub generation: u64,

    /// When API data was last fetched
    #[serde(default, with = "option_system_time")]
    pub last_api_update: Option<SystemTime>,

    /// All character data (includes custom labels, sort order, pets)
    pub characters: CharacterCache,

    /// Regular account vault (vault chests, gifts, materials, potions)
    #[serde(default)]
    pub regular_vault: LiveVaultStorage,

    /// Seasonal account vault (vault chests, gifts, materials, potions)
    #[serde(default)]
    pub seasonal_vault: LiveVaultStorage,

    /// Cached dust amounts for regular account.
    #[serde(default)]
    pub regular_dust: Option<DustAmounts>,

    /// Cached dust amounts for seasonal account.
    #[serde(default)]
    pub seasonal_dust: Option<DustAmounts>,

    /// Account-level live stats (stars, fame, gold, forge fire, materials)
    /// sourced from the local player object. Backs the Widget Bar.
    #[serde(default)]
    pub account_stats: crate::account_stats::AccountStats,

    /// Total character slots on the account (`maxNumChars` from char/list).
    /// `0` when unknown.
    #[serde(default)]
    pub max_num_chars: i32,

    /// Price of the next character slot (`NextCharSlotPrice` from char/list).
    #[serde(default)]
    pub next_char_slot_price: i32,

    /// Number of skins owned (`OwnedSkins` list length from char/list).
    #[serde(default)]
    pub owned_skins_count: i32,

    /// Dirty flag - true if data changed since last save.
    /// Not serialized - always starts false on load.
    #[serde(skip)]
    dirty: bool,
}

impl Default for AccountData {
    fn default() -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            generation: 0,
            last_api_update: None,
            characters: CharacterCache::default(),
            regular_vault: LiveVaultStorage::default(),
            seasonal_vault: LiveVaultStorage::default(),
            regular_dust: None,
            seasonal_dust: None,
            account_stats: crate::account_stats::AccountStats::default(),
            max_num_chars: 0,
            next_char_slot_price: 0,
            owned_skins_count: 0,
            dirty: false,
        }
    }
}

impl AccountData {
    /// Current schema version.
    pub const CURRENT_VERSION: u32 = 1;

    /// Create a new empty account data store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load account data through an explicit [`AccountDataRepository`].
    ///
    /// Preserved for callers that already hold a repository; equivalent to
    /// [`AccountDataRepository::load`].
    pub fn load(repository: &AccountDataRepository) -> io::Result<Self> {
        repository.load()
    }

    /// Save account data to disk through an explicit [`AccountDataRepository`].
    pub fn save(&self, repository: &AccountDataRepository) -> io::Result<()> {
        repository.save(self)
    }

    /// Save account data and clear the dirty flag.
    pub fn save_and_clear_dirty(&mut self, repository: &AccountDataRepository) -> io::Result<()> {
        self.save(repository)?;
        self.dirty = false;
        Ok(())
    }

    /// Check if data has changed since last save.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Mark data as dirty (changed).
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Clear the dirty flag without saving.
    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    /// Increment the generation counter and mark dirty.
    /// Call this after any data mutation.
    pub fn bump_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.dirty = true;
    }

    /// Get the current generation counter.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Update cached dust amounts and save.
    pub fn update_dust(
        &mut self,
        amounts: DustAmounts,
        is_seasonal: bool,
        repository: &AccountDataRepository,
    ) {
        if is_seasonal {
            self.seasonal_dust = Some(amounts);
        } else {
            self.regular_dust = Some(amounts);
        }
        self.bump_generation();
        if let Err(e) = self.save(repository) {
            tracing::error!("[ACCOUNT] Failed to save dust update: {}", e);
        }
    }

    /// Merge a live account-stats update. Bumps generation only when a value
    /// actually changed (these arrive on ordinary ticks). Does not save to disk
    /// on every change; relies on the periodic save.
    pub fn update_account_stats(
        &mut self,
        update: &crate::account_stats::AccountStatsUpdate,
    ) -> bool {
        if self.account_stats.merge(update) {
            self.bump_generation();
            true
        } else {
            false
        }
    }

    /// Check if we have any character data (living or deceased).
    pub fn has_characters(&self) -> bool {
        !self.characters.characters.is_empty() || !self.characters.dead_characters.is_empty()
    }

    /// Check if we have any vault data.
    pub fn has_vault_data(&self) -> bool {
        self.regular_vault.has_data() || self.seasonal_vault.has_data()
    }

    /// Get time since last API update, if known.
    pub fn time_since_api_update(&self) -> Option<Duration> {
        self.last_api_update.and_then(|t| t.elapsed().ok())
    }

    /// Format time since last API update as human-readable string.
    pub fn format_last_api_update(&self) -> String {
        match self.time_since_api_update() {
            Some(elapsed) => {
                let secs = elapsed.as_secs();
                if secs < 60 {
                    format!("{}s ago", secs)
                } else if secs < 3600 {
                    format!("{}m ago", secs / 60)
                } else if secs < 86400 {
                    format!("{}h ago", secs / 3600)
                } else {
                    format!("{}d ago", secs / 86400)
                }
            }
            None => "Never".to_string(),
        }
    }

    // =========================================================================
    // Data Update Methods (FR-2: Data Sources)
    // =========================================================================

    /// Update from API response - both characters and vault data.
    /// This is the primary update method for the Refresh button.
    pub fn update_from_api_response(&mut self, api_data: &ApiAccountData) {
        // Update characters (handles dead detection, pets, etc.). If the response
        // belongs to a different account (mule/alt), the update is rejected and we
        // must not overwrite the vault/exaltation data either.
        if !self
            .characters
            .update_from_api(&api_data.characters, api_data.account_id.as_deref())
        {
            tracing::warn!(
                "[ACCOUNT] Skipped API update (different account): vault and exaltation \
                 data left unchanged."
            );
            return;
        }

        // Convert API vault data to LiveVaultStorage format
        self.update_vault_from_api(api_data, false); // Regular vault
                                                     // Note: API doesn't distinguish seasonal vault - that comes from packets

        self.last_api_update = Some(SystemTime::now());
        self.bump_generation();

        tracing::info!(
            "[ACCOUNT] Updated from API: {} characters, {} vault items, {} gifts",
            api_data.characters.len(),
            api_data.vault.total_items(),
            api_data.gifts.count()
        );
    }

    /// Update from character list only (simpler API parse).
    pub fn update_characters_from_api(&mut self, characters: &[RealmCharacter]) {
        self.characters.update_from_api(characters, None);
        self.last_api_update = Some(SystemTime::now());
        self.bump_generation();
    }

    /// Convert API vault/storage data to LiveVaultStorage format.
    fn update_vault_from_api(&mut self, api_data: &ApiAccountData, _seasonal: bool) {
        // For now, API data goes to regular vault (API doesn't distinguish seasonal)
        let storage = &mut self.regular_vault;

        // Convert vault chests to flat item list
        storage.vault_items.clear();
        for chest in &api_data.vault.chests {
            for item in &chest.items {
                let enchant_ids = item
                    .enchant_data
                    .as_ref()
                    .map(|data| parse_enchant_ids(data))
                    .unwrap_or_default();
                storage
                    .vault_items
                    .push(LiveVaultItem::with_enchants(item.item_id, enchant_ids));
            }
        }

        // Convert gifts
        storage.gift_items.clear();
        for item in &api_data.gifts.items {
            let enchant_ids = item
                .enchant_data
                .as_ref()
                .map(|data| parse_enchant_ids(data))
                .unwrap_or_default();
            storage
                .gift_items
                .push(LiveVaultItem::with_enchants(item.item_id, enchant_ids));
        }

        // Convert potions
        storage.potion_items.clear();
        for item in &api_data.potions.items {
            storage.potion_items.push(LiveVaultItem::new(item.item_id));
        }

        // Convert materials
        storage.material_items.clear();
        for chest in &api_data.materials.chests {
            for item in &chest.items {
                storage
                    .material_items
                    .push(LiveVaultItem::new(item.item_id));
            }
        }

        storage.last_updated = Some(SystemTime::now());
    }

    /// Update vault from VaultContentPacket.
    /// Uses the seasonal flag to determine which vault to update.
    pub fn update_vault_from_packet(&mut self, packet: &VaultContentPacket, vault_type: VaultType) {
        let storage = match vault_type {
            VaultType::Regular => &mut self.regular_vault,
            VaultType::Seasonal => &mut self.seasonal_vault,
        };

        // If this is a new vault session (not a continuation), clear existing data
        if !storage.pending_update {
            storage.clear();
        }

        // Parse enchants and update storage
        let enchants = super::ParsedEnchants::from_packet(packet);
        storage.update_from_packet(packet, &enchants);

        self.bump_generation();
    }

    /// Mark a character as dead (from Death packet).
    pub fn mark_character_dead(
        &mut self,
        char_id: i32,
        killed_by: &str,
        death_fame: i32,
        gravestone_type: Option<i32>,
    ) {
        self.characters
            .mark_dead(char_id, killed_by, death_fame, gravestone_type);
        self.bump_generation();
    }

    /// Add a new character (from Create packet).
    pub fn add_new_character(&mut self, char_id: i32, class_id: u16, skin: i32, seasonal: bool) {
        self.characters
            .add_new_character(char_id, class_id, skin, seasonal);
        self.bump_generation();
    }

    /// Update a single character from NewCharacterInfo packet.
    pub fn update_single_character(&mut self, character: &RealmCharacter) {
        self.characters.update_single_character(character);
        self.bump_generation();
    }

    /// Update character's seasonal status (from Update packet with seasonal stat).
    pub fn update_seasonal_status(&mut self, char_id: i32, is_seasonal: bool) {
        self.characters.update_seasonal_status(char_id, is_seasonal);
        self.bump_generation();
    }

    /// Update character's crucible status (from the live player object's
    /// CRUCIBLE stat), correcting the stale char-list `CrucibleActive` flag.
    pub fn update_crucible_status(&mut self, char_id: i32, is_active: bool) {
        self.characters.update_crucible_status(char_id, is_active);
        self.bump_generation();
    }

    /// Remove a dead character from the list.
    pub fn remove_character(&mut self, char_id: i32) {
        self.characters.remove_character(char_id);
        self.bump_generation();
    }

    /// Find a character by ID.
    pub fn find_character(&self, char_id: i32) -> Option<&super::CachedCharacter> {
        self.characters.find_character(char_id)
    }

    /// Find a character by ID (mutable).
    pub fn find_character_mut(&mut self, char_id: i32) -> Option<&mut super::CachedCharacter> {
        self.characters.find_character_mut(char_id)
    }

    /// Get the appropriate vault storage for a vault type.
    pub fn get_vault(&self, vault_type: VaultType) -> &LiveVaultStorage {
        match vault_type {
            VaultType::Regular => &self.regular_vault,
            VaultType::Seasonal => &self.seasonal_vault,
        }
    }

    /// Get the appropriate vault storage for a vault type (mutable).
    pub fn get_vault_mut(&mut self, vault_type: VaultType) -> &mut LiveVaultStorage {
        match vault_type {
            VaultType::Regular => &mut self.regular_vault,
            VaultType::Seasonal => &mut self.seasonal_vault,
        }
    }
}

/// Legacy flat-layout inputs consulted only by the compatibility loader when no
/// canonical `account_data.json` exists yet. Profile loading never probes these.
#[derive(Debug, Clone)]
pub struct LegacyAccountInputs {
    /// Flat `characters_cache.json` under the storage root.
    pub characters_cache: PathBuf,
    /// Roaming `characters_cache.json`, used only when the local file is absent.
    pub roaming_characters_cache: Option<PathBuf>,
    /// Flat `live_vault.json` under the storage root.
    pub live_vault: PathBuf,
}

/// Explicit persistence binding for one account's canonical `AccountData`.
///
/// The repository is the only source of the `account_data.json` path. In
/// flat-layout compatibility mode it also carries the legacy inputs used to
/// convert a pre-profile installation once; profile loading omits them so it
/// can never read another layout's data.
#[derive(Debug, Clone)]
pub struct AccountDataRepository {
    account_data: PathBuf,
    legacy: Option<LegacyAccountInputs>,
}

impl AccountDataRepository {
    /// Bind to a profile's canonical snapshot with no legacy fallback.
    pub fn for_profile(account_data: PathBuf) -> Self {
        Self {
            account_data,
            legacy: None,
        }
    }

    /// Bind to the flat-layout `account_data.json`, retaining the legacy
    /// character/vault inputs for the one-time compatibility conversion.
    pub fn flat_compat(account_data: PathBuf, legacy: LegacyAccountInputs) -> Self {
        Self {
            account_data,
            legacy: Some(legacy),
        }
    }

    /// Return the bound `account_data.json` path.
    pub fn account_data_path(&self) -> &Path {
        &self.account_data
    }

    /// Load the canonical snapshot, recovering the rolling backup when needed and
    /// running the in-line schema migrations. Falls back to legacy conversion
    /// only in flat-compat mode when no canonical snapshot exists.
    pub fn load(&self) -> io::Result<AccountData> {
        let outcome = match load_json_at::<AccountData>(&self.account_data) {
            Ok(outcome) => outcome,
            Err(error @ StorageError::InvalidDocument { .. })
            | Err(error @ StorageError::InvalidDocuments { .. }) => {
                self.preserve_invalid_documents();
                return Err(io::Error::new(io::ErrorKind::InvalidData, error));
            }
            Err(error) => return Err(io::Error::new(io::ErrorKind::InvalidData, error)),
        };
        match outcome {
            LoadOutcome::Primary(data) | LoadOutcome::RecoveredBackup(data) => {
                Ok(Self::finish_load(data))
            }
            LoadOutcome::Missing => match &self.legacy {
                Some(legacy) => self.migrate_from_legacy(legacy),
                None => Ok(AccountData::new()),
            },
        }
    }

    /// Run the in-line migrations and clear transient state after a successful
    /// deserialization.
    fn finish_load(mut data: AccountData) -> AccountData {
        // Migrate any legacy in-line dead characters into the graveyard list.
        data.characters.migrate_dead_characters();
        // One-time clear of stale char-list crucible flags.
        data.characters.migrate_crucible_flags();

        if data.version != AccountData::CURRENT_VERSION {
            tracing::warn!(
                "[ACCOUNT] Schema version mismatch: {} vs {}, may need migration",
                data.version,
                AccountData::CURRENT_VERSION
            );
            // Future: add migration logic here
        }

        data.dirty = false;
        tracing::info!(
            "[ACCOUNT] Loaded account data: {} characters, {} regular vault items, {} seasonal vault items",
            data.characters.characters.len(),
            data.regular_vault.vault_item_count(),
            data.seasonal_vault.vault_item_count()
        );
        data
    }

    /// Strict, non-destructive load for selected-profile startup.
    ///
    /// Unlike [`Self::load`], a corrupt existing snapshot is never renamed or
    /// replaced with empty state: the error is returned so retries keep failing
    /// against the original document. A legitimately missing snapshot (opened
    /// only after manifest validation and the profile lock) initializes empty.
    /// Legacy flat inputs are never consulted.
    pub fn load_strict(&self) -> io::Result<AccountData> {
        match load_json_at::<AccountData>(&self.account_data) {
            Ok(LoadOutcome::Primary(data)) | Ok(LoadOutcome::RecoveredBackup(data)) => {
                Ok(Self::finish_load(data))
            }
            Ok(LoadOutcome::Missing) => Ok(AccountData::new()),
            Err(error) => Err(io::Error::new(io::ErrorKind::InvalidData, error)),
        }
    }

    /// Save the snapshot atomically, keeping one rolling backup.
    pub fn save(&self, data: &AccountData) -> io::Result<()> {
        write_json_atomic_at(&self.account_data, data, BackupPolicy::Single)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    fn preserve_invalid_documents(&self) {
        if self.account_data.exists() {
            crate::settings::back_up_corrupt_file(&self.account_data);
        }
        if let Some(file_name) = self.account_data.file_name() {
            let mut backup_name = file_name.to_os_string();
            backup_name.push(".bak");
            let backup = self.account_data.with_file_name(backup_name);
            if backup.exists() {
                crate::settings::back_up_corrupt_file(&backup);
            }
        }
    }

    /// Convert flat `characters_cache.json`/`live_vault.json` into the canonical
    /// snapshot. The legacy sources are read non-destructively and left in place;
    /// flat-layout migration owns their relocation into a timestamped backup.
    fn migrate_from_legacy(&self, legacy: &LegacyAccountInputs) -> io::Result<AccountData> {
        let mut data = AccountData::new();
        let mut migrated_any = false;

        // Character cache: prefer the local flat file, fall back to Roaming.
        let chars_source = if legacy.characters_cache.exists() {
            Some(legacy.characters_cache.clone())
        } else {
            legacy
                .roaming_characters_cache
                .as_ref()
                .filter(|p| p.exists())
                .cloned()
        };
        if let Some(char_path) = &chars_source {
            match fs::read_to_string(char_path) {
                Ok(contents) => match parse_legacy_character_cache(&contents) {
                    Ok(cache) => {
                        tracing::info!(
                            "[ACCOUNT] Migrating {} characters from legacy cache",
                            cache.characters.len()
                        );
                        data.characters = cache;
                        data.characters.migrate_dead_characters();
                        data.characters.migrate_crucible_flags();
                        data.last_api_update = data.characters.last_updated;
                        migrated_any = true;
                    }
                    Err(e) => {
                        tracing::warn!("[ACCOUNT] Failed to parse legacy character cache: {}", e);
                    }
                },
                Err(e) => {
                    tracing::warn!("[ACCOUNT] Failed to read legacy character cache: {}", e);
                }
            }
        }

        // Legacy live vault.
        if legacy.live_vault.exists() {
            match fs::read_to_string(&legacy.live_vault) {
                Ok(contents) => match parse_legacy_vault(&contents) {
                    Ok((regular, seasonal)) => {
                        tracing::info!(
                            "[ACCOUNT] Migrating vault data: {} regular, {} seasonal items",
                            regular.vault_item_count(),
                            seasonal.vault_item_count()
                        );
                        data.regular_vault = regular;
                        data.seasonal_vault = seasonal;
                        migrated_any = true;
                    }
                    Err(e) => {
                        tracing::warn!("[ACCOUNT] Failed to parse legacy vault data: {}", e);
                    }
                },
                Err(e) => {
                    tracing::warn!("[ACCOUNT] Failed to read legacy vault data: {}", e);
                }
            }
        }

        if migrated_any {
            if let Err(e) = self.save(&data) {
                tracing::warn!("[ACCOUNT] Failed to save migrated data: {}", e);
            }
        }

        Ok(data)
    }
}

/// Parse a legacy `characters_cache.json` document without mutating any file.
///
/// Shared non-destructive parsing used by both the flat-compatibility loader and
/// flat-layout migration validation.
pub fn parse_legacy_character_cache(contents: &str) -> serde_json::Result<CharacterCache> {
    serde_json::from_str::<CharacterCache>(contents)
}

/// Parse a legacy `live_vault.json` document into its regular and seasonal
/// storages without mutating any file.
pub fn parse_legacy_vault(
    contents: &str,
) -> serde_json::Result<(LiveVaultStorage, LiveVaultStorage)> {
    #[derive(Deserialize)]
    struct LegacyVaultData {
        #[serde(default)]
        regular: LiveVaultStorage,
        #[serde(default)]
        seasonal: LiveVaultStorage,
    }

    let data = serde_json::from_str::<LegacyVaultData>(contents)?;
    Ok((data.regular, data.seasonal))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_account_data() {
        let data = AccountData::new();
        assert_eq!(data.version, AccountData::CURRENT_VERSION);
        assert_eq!(data.generation, 0);
        assert!(!data.has_characters());
        assert!(!data.has_vault_data());
        assert!(!data.is_dirty());
    }

    #[test]
    fn test_bump_generation() {
        let mut data = AccountData::new();
        assert_eq!(data.generation, 0);
        assert!(!data.is_dirty());

        data.bump_generation();
        assert_eq!(data.generation, 1);
        assert!(data.is_dirty());

        data.bump_generation();
        assert_eq!(data.generation, 2);
    }

    #[test]
    fn test_dirty_flag() {
        let mut data = AccountData::new();
        assert!(!data.is_dirty());

        data.mark_dirty();
        assert!(data.is_dirty());

        data.clear_dirty();
        assert!(!data.is_dirty());
    }

    #[test]
    fn round_trips_through_repository_with_backup() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("account_data.json");
        let repo = AccountDataRepository::for_profile(path.clone());

        let mut data = AccountData::new();
        data.max_num_chars = 8;
        data.bump_generation();
        repo.save(&data).unwrap();

        let loaded = repo.load().unwrap();
        assert_eq!(loaded.max_num_chars, 8);
        assert!(!loaded.is_dirty(), "loaded snapshot starts clean");

        // A second save rotates a single bounded backup.
        repo.save(&loaded).unwrap();
        assert!(path.with_extension("json.bak").exists());
    }

    #[test]
    fn corrupt_primary_recovers_from_backup() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("account_data.json");
        let repo = AccountDataRepository::for_profile(path.clone());

        let mut first = AccountData::new();
        first.max_num_chars = 3;
        repo.save(&first).unwrap();
        first.max_num_chars = 5;
        repo.save(&first).unwrap();

        // Corrupt the primary; the rolling backup still holds the prior value.
        std::fs::write(&path, b"{ not json").unwrap();
        let recovered = repo.load().unwrap();
        assert_eq!(recovered.max_num_chars, 3);
    }

    #[test]
    fn malformed_snapshot_is_preserved_across_future_saves() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("account_data.json");
        let repo = AccountDataRepository::for_profile(path.clone());
        let malformed = b"{ not json";
        std::fs::write(&path, malformed).unwrap();

        assert!(repo.load().is_err());
        repo.save(&AccountData::new()).unwrap();
        repo.save(&AccountData::new()).unwrap();

        let preserved = std::fs::read_dir(temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|candidate| {
                candidate
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("account_data.json.corrupt-"))
            })
            .expect("timestamped corrupt snapshot should remain");
        assert_eq!(std::fs::read(preserved).unwrap(), malformed);
    }

    #[test]
    fn profile_load_ignores_missing_snapshot_without_legacy() {
        let temp = tempfile::tempdir().unwrap();
        let repo = AccountDataRepository::for_profile(temp.path().join("account_data.json"));
        let data = repo.load().unwrap();
        assert!(!data.has_characters());
        assert!(!data.has_vault_data());
    }

    #[test]
    fn strict_load_treats_missing_snapshot_as_empty() {
        let temp = tempfile::tempdir().unwrap();
        let repo = AccountDataRepository::for_profile(temp.path().join("account_data.json"));
        let data = repo.load_strict().unwrap();
        assert!(!data.has_characters());
        assert!(!data.has_vault_data());
    }

    #[test]
    fn strict_load_fails_repeatedly_and_never_replaces_corrupt_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("account_data.json");
        let repo = AccountDataRepository::for_profile(path.clone());
        let malformed = b"{ not json";
        std::fs::write(&path, malformed).unwrap();

        // A corrupt snapshot is an error, not empty state.
        assert!(repo.load_strict().is_err());
        // The strict path is non-destructive: no corrupt-quarantine copy is made
        // and the original bytes are untouched, so retries keep failing.
        assert_eq!(std::fs::read(&path).unwrap(), malformed);
        assert!(repo.load_strict().is_err());
        let siblings = std::fs::read_dir(temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("account_data.json.corrupt-"))
            });
        assert!(!siblings, "strict load must not rename a corrupt snapshot");
    }

    #[test]
    fn flat_compat_loader_imports_and_preserves_legacy_inputs() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        let account_data = base.join("account_data.json");
        let characters_cache = base.join("characters_cache.json");
        let live_vault = base.join("live_vault.json");

        // Seed a legacy character cache with an identifiable custom label.
        let mut legacy_chars = CharacterCache::default();
        legacy_chars.custom_labels.insert(42, "Legacy".to_string());
        std::fs::write(
            &characters_cache,
            serde_json::to_string(&legacy_chars).unwrap(),
        )
        .unwrap();
        std::fs::write(&live_vault, br#"{"regular":{},"seasonal":{}}"#).unwrap();

        let repo = AccountDataRepository::flat_compat(
            account_data.clone(),
            LegacyAccountInputs {
                characters_cache: characters_cache.clone(),
                roaming_characters_cache: None,
                live_vault: live_vault.clone(),
            },
        );
        let loaded = repo.load().unwrap();

        assert_eq!(
            loaded.characters.custom_labels.get(&42).map(String::as_str),
            Some("Legacy")
        );
        // Canonical snapshot was written; legacy inputs remain for migration.
        assert!(account_data.exists());
        assert!(characters_cache.exists());
        assert!(live_vault.exists());
    }

    #[test]
    fn flat_compat_falls_back_to_roaming_characters_cache() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        let account_data = base.join("account_data.json");
        let local_cache = base.join("characters_cache.json");
        let roaming_cache = base.join("roaming_characters_cache.json");

        // Only the Roaming legacy file exists; the local one is absent.
        let mut legacy_chars = CharacterCache::default();
        legacy_chars.custom_labels.insert(7, "Roaming".to_string());
        std::fs::write(
            &roaming_cache,
            serde_json::to_string(&legacy_chars).unwrap(),
        )
        .unwrap();

        let repo = AccountDataRepository::flat_compat(
            account_data.clone(),
            LegacyAccountInputs {
                characters_cache: local_cache,
                roaming_characters_cache: Some(roaming_cache.clone()),
                live_vault: base.join("live_vault.json"),
            },
        );
        let loaded = repo.load().unwrap();

        assert_eq!(
            loaded.characters.custom_labels.get(&7).map(String::as_str),
            Some("Roaming")
        );
        assert!(
            roaming_cache.exists(),
            "roaming source preserved after conversion"
        );
    }
}

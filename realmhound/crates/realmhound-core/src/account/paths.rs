use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{AccountId, AccountKey};
use crate::storage::{StorageError, StorageRoot};

pub(crate) const PROFILE_VERSION: u32 = 1;

/// Validated paths belonging to one account profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPaths {
    storage_root: StorageRoot,
    account_key: AccountKey,
}

impl AccountPaths {
    /// Resolve profile paths for an account below a configured storage root.
    pub fn new(storage_root: StorageRoot, account_key: AccountKey) -> Self {
        Self {
            storage_root,
            account_key,
        }
    }

    /// Return the account key represented by these paths.
    pub fn account_key(&self) -> AccountKey {
        self.account_key
    }

    /// Return the profile root.
    pub fn root(&self) -> Result<PathBuf, StorageError> {
        self.resolve("")
    }

    /// Return the profile manifest path.
    pub fn manifest(&self) -> Result<PathBuf, StorageError> {
        self.resolve("profile.json")
    }

    /// Return the account snapshot path.
    pub fn account_data(&self) -> Result<PathBuf, StorageError> {
        self.resolve("account_data.json")
    }

    /// Return the functional character-list cache path.
    pub fn character_list_cache(&self) -> Result<PathBuf, StorageError> {
        self.resolve(Path::new("cache").join("char_list.xml"))
    }

    /// Return the quest data path.
    pub fn quests(&self) -> Result<PathBuf, StorageError> {
        self.resolve(Path::new("quests").join("quests.json"))
    }

    /// Return the loot history database path.
    pub fn loot_database(&self) -> Result<PathBuf, StorageError> {
        self.resolve(Path::new("databases").join("loot_history.db"))
    }

    /// Return the combat history database path.
    pub fn combat_database(&self) -> Result<PathBuf, StorageError> {
        self.resolve(Path::new("databases").join("combat_history.db"))
    }

    /// Return the RealmShark import path.
    pub fn realmshark_import(&self) -> Result<PathBuf, StorageError> {
        self.resolve(
            Path::new("imports")
                .join("realmshark")
                .join("dungeon.stats"),
        )
    }

    /// Return the per-account chat log directory.
    pub fn chat_logs(&self) -> Result<PathBuf, StorageError> {
        self.resolve(Path::new("logs").join("chat"))
    }

    /// Return the per-account event notification log.
    pub fn event_notifications(&self) -> Result<PathBuf, StorageError> {
        self.resolve(Path::new("logs").join("event_notifications.csv"))
    }

    /// Return the per-account watchlist detection log.
    pub fn watchlist_detections(&self) -> Result<PathBuf, StorageError> {
        self.resolve(Path::new("logs").join("watchlist_detections.log"))
    }

    /// Return the debug-only API diagnostics directory.
    pub fn api_diagnostics(&self) -> Result<PathBuf, StorageError> {
        self.resolve(Path::new("diagnostics").join("api"))
    }

    pub(crate) fn relative_root(&self) -> PathBuf {
        Path::new("accounts").join(self.account_key.to_string())
    }

    pub(crate) fn relative_manifest(&self) -> PathBuf {
        self.relative_root().join("profile.json")
    }

    fn resolve(&self, relative: impl AsRef<Path>) -> Result<PathBuf, StorageError> {
        self.storage_root
            .ensure_no_links(self.relative_root().join(relative))
    }
}

/// Schema versions for account-scoped profile documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSchemas {
    /// Account snapshot schema version.
    pub account_data: u32,
    /// Loot history database schema version.
    pub loot_database: u32,
    /// Combat history database schema version.
    pub combat_database: u32,
}

impl Default for ProfileSchemas {
    fn default() -> Self {
        Self {
            account_data: 1,
            loot_database: 1,
            combat_database: 1,
        }
    }
}

/// Versioned identity manifest stored inside one account profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileManifest {
    version: u32,
    account_key: AccountKey,
    account_id: AccountId,
    created_at: DateTime<Utc>,
    schemas: ProfileSchemas,
}

impl ProfileManifest {
    /// Create a version-1 profile manifest.
    pub fn new(account_key: AccountKey, account_id: AccountId, created_at: DateTime<Utc>) -> Self {
        Self {
            version: PROFILE_VERSION,
            account_key,
            account_id,
            created_at,
            schemas: ProfileSchemas::default(),
        }
    }

    /// Return the document version.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Return the profile's local account key.
    pub fn account_key(&self) -> AccountKey {
        self.account_key
    }

    /// Return the authoritative server account identity.
    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Return the profile creation time.
    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    /// Return the profile schema versions.
    pub fn schemas(&self) -> ProfileSchemas {
        self.schemas
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_paths_resolve_below_account_key() {
        let temp = tempfile::tempdir().unwrap();
        let storage_root = StorageRoot::from_path(temp.path()).unwrap();
        let key = AccountKey::generate();
        let paths = AccountPaths::new(storage_root, key);

        assert_eq!(
            paths.combat_database().unwrap(),
            temp.path()
                .join("accounts")
                .join(key.to_string())
                .join("databases")
                .join("combat_history.db")
        );
    }

    #[test]
    fn account_paths_cover_documented_profile_layout() {
        let temp = tempfile::tempdir().unwrap();
        let storage_root = StorageRoot::from_path(temp.path()).unwrap();
        let key = AccountKey::generate();
        let paths = AccountPaths::new(storage_root, key);
        let profile_root = temp.path().join("accounts").join(key.to_string());
        let resolved = vec![
            paths.manifest().unwrap(),
            paths.account_data().unwrap(),
            paths.character_list_cache().unwrap(),
            paths.quests().unwrap(),
            paths.loot_database().unwrap(),
            paths.combat_database().unwrap(),
            paths.realmshark_import().unwrap(),
            paths.chat_logs().unwrap(),
            paths.event_notifications().unwrap(),
            paths.watchlist_detections().unwrap(),
            paths.api_diagnostics().unwrap(),
        ];
        let expected = vec![
            profile_root.join("profile.json"),
            profile_root.join("account_data.json"),
            profile_root.join("cache").join("char_list.xml"),
            profile_root.join("quests").join("quests.json"),
            profile_root.join("databases").join("loot_history.db"),
            profile_root.join("databases").join("combat_history.db"),
            profile_root
                .join("imports")
                .join("realmshark")
                .join("dungeon.stats"),
            profile_root.join("logs").join("chat"),
            profile_root.join("logs").join("event_notifications.csv"),
            profile_root.join("logs").join("watchlist_detections.log"),
            profile_root.join("diagnostics").join("api"),
        ];

        assert_eq!(resolved, expected);
    }

    #[test]
    fn profile_manifest_round_trips() {
        let manifest = ProfileManifest::new(
            AccountKey::generate(),
            AccountId::new("account").unwrap(),
            Utc::now(),
        );

        let json = serde_json::to_string(&manifest).unwrap();
        let loaded: ProfileManifest = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded, manifest);
    }
}

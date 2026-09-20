//! Centralized account-scoped persistence paths.
//!
//! [`AccountPersistencePaths`] is created once and injected into every
//! account-scoped component, so no component resolves `dirs::data_local_dir()`
//! itself. It has two construction modes:
//!
//! - [`AccountPersistencePaths::flat_compat`] preserves the current
//!   single-account flat layout.
//! - [`AccountPersistencePaths::for_profile`] derives every path from an
//!   [`AccountPaths`], which remains the only source of profile paths.

use std::path::{Path, PathBuf};

use super::AccountPaths;
use crate::storage::{StorageError, StorageRoot};
use crate::vault::{AccountDataRepository, LegacyAccountInputs};

/// Explicit persistence paths for one account, resolved once from either the
/// flat compatibility layout or an [`AccountPaths`] profile.
#[derive(Debug, Clone)]
pub struct AccountPersistencePaths {
    account_data: PathBuf,
    legacy_account_inputs: Option<LegacyAccountInputs>,
    char_list_cache: PathBuf,
    quests: PathBuf,
    loot_database: PathBuf,
    combat_database: PathBuf,
    realmshark_import: PathBuf,
    chat_logs: PathBuf,
    event_notifications: PathBuf,
    watchlist_detections: PathBuf,
    legacy_access_token: Option<PathBuf>,
    api_diagnostics: Option<PathBuf>,
}

impl AccountPersistencePaths {
    /// Resolve the flat single-account layout below the storage root.
    ///
    /// This is the production binding until the migration phases run: it maps
    /// the exact current flat files and retains the legacy character/vault and
    /// access-token inputs for compatibility. No profile diagnostics path exists
    /// yet, so API diagnostics stay disabled.
    pub fn flat_compat(root: &StorageRoot) -> Result<Self, StorageError> {
        let characters_cache = root.ensure_no_links("characters_cache.json")?;
        let live_vault = root.ensure_no_links("live_vault.json")?;
        let legacy = LegacyAccountInputs {
            characters_cache,
            roaming_characters_cache: roaming_characters_cache(),
            live_vault,
        };
        Ok(Self {
            account_data: root.ensure_no_links("account_data.json")?,
            legacy_account_inputs: Some(legacy),
            char_list_cache: root.ensure_no_links("char_list.xml")?,
            quests: root.ensure_no_links("quests.json")?,
            loot_database: root.ensure_no_links("loot_history.db")?,
            combat_database: root.ensure_no_links("combat_history.db")?,
            realmshark_import: root.ensure_no_links(Path::new("imports").join("dungeon.stats"))?,
            chat_logs: root.ensure_no_links(Path::new("logs").join("chat"))?,
            event_notifications: root.ensure_no_links("event_notification_log.csv")?,
            watchlist_detections: root.ensure_no_links("watchlist_detections.log")?,
            legacy_access_token: Some(root.ensure_no_links("access_token.txt")?),
            api_diagnostics: None,
        })
    }

    /// Derive every path from an isolated account profile.
    ///
    /// Profile mode never carries legacy flat inputs, so profile loading can
    /// never read another layout's data. Debug builds may write API diagnostics
    /// below the verified profile.
    pub fn for_profile(paths: &AccountPaths) -> Result<Self, StorageError> {
        Ok(Self {
            account_data: paths.account_data()?,
            legacy_account_inputs: None,
            char_list_cache: paths.character_list_cache()?,
            quests: paths.quests()?,
            loot_database: paths.loot_database()?,
            combat_database: paths.combat_database()?,
            realmshark_import: paths.realmshark_import()?,
            chat_logs: paths.chat_logs()?,
            event_notifications: paths.event_notifications()?,
            watchlist_detections: paths.watchlist_detections()?,
            legacy_access_token: None,
            api_diagnostics: Some(paths.api_diagnostics()?),
        })
    }

    /// Build the canonical [`AccountDataRepository`] for this account.
    pub fn account_data_repository(&self) -> AccountDataRepository {
        match &self.legacy_account_inputs {
            Some(legacy) => {
                AccountDataRepository::flat_compat(self.account_data.clone(), legacy.clone())
            }
            None => AccountDataRepository::for_profile(self.account_data.clone()),
        }
    }

    /// Path to the canonical `account_data.json`.
    pub fn account_data(&self) -> &Path {
        &self.account_data
    }

    /// Path to the functional `char_list.xml` cache.
    pub fn char_list_cache(&self) -> &Path {
        &self.char_list_cache
    }

    /// Path to the quest data document.
    pub fn quests(&self) -> &Path {
        &self.quests
    }

    /// Path to the loot history database.
    pub fn loot_database(&self) -> &Path {
        &self.loot_database
    }

    /// Path to the combat history database.
    pub fn combat_database(&self) -> &Path {
        &self.combat_database
    }

    /// Path to the imported RealmShark `dungeon.stats` file.
    pub fn realmshark_import(&self) -> &Path {
        &self.realmshark_import
    }

    /// Directory for per-account chat logs.
    pub fn chat_logs(&self) -> &Path {
        &self.chat_logs
    }

    /// Path to the opt-in event-notification diagnostic log.
    pub fn event_notifications(&self) -> &Path {
        &self.event_notifications
    }

    /// Path to the per-account watchlist detection log.
    pub fn watchlist_detections(&self) -> &Path {
        &self.watchlist_detections
    }

    /// Debug-only API diagnostics directory, present only in profile mode.
    pub fn api_diagnostics(&self) -> Option<&Path> {
        self.api_diagnostics.as_deref()
    }

    /// Legacy flat access-token file, present only in flat compatibility mode.
    pub fn legacy_access_token(&self) -> Option<&Path> {
        self.legacy_access_token.as_deref()
    }
}

/// Resolve the Roaming `characters_cache.json`, matching the historical legacy
/// fallback consulted when no LocalAppData replacement exists.
fn roaming_characters_cache() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|app_data| {
        PathBuf::from(app_data)
            .join("RealmHound")
            .join("characters_cache.json")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::AccountKey;

    #[test]
    fn flat_compat_maps_exact_current_layout() {
        let temp = tempfile::tempdir().unwrap();
        let root = StorageRoot::from_path(temp.path()).unwrap();
        let paths = AccountPersistencePaths::flat_compat(&root).unwrap();
        let base = temp.path();

        assert_eq!(paths.account_data(), base.join("account_data.json"));
        assert_eq!(paths.char_list_cache(), base.join("char_list.xml"));
        assert_eq!(paths.quests(), base.join("quests.json"));
        assert_eq!(paths.loot_database(), base.join("loot_history.db"));
        assert_eq!(paths.combat_database(), base.join("combat_history.db"));
        assert_eq!(
            paths.realmshark_import(),
            base.join("imports").join("dungeon.stats")
        );
        assert_eq!(paths.chat_logs(), base.join("logs").join("chat"));
        assert_eq!(
            paths.event_notifications(),
            base.join("event_notification_log.csv")
        );
        assert_eq!(
            paths.watchlist_detections(),
            base.join("watchlist_detections.log")
        );
        assert_eq!(
            paths.legacy_access_token(),
            Some(base.join("access_token.txt").as_path())
        );
        // Flat compatibility does not write profile diagnostics.
        assert!(paths.api_diagnostics().is_none());
    }

    #[test]
    fn profile_layout_resolves_below_account_key_and_enables_diagnostics() {
        let temp = tempfile::tempdir().unwrap();
        let root = StorageRoot::from_path(temp.path()).unwrap();
        let key = AccountKey::generate();
        let account_paths = AccountPaths::new(root, key);
        let paths = AccountPersistencePaths::for_profile(&account_paths).unwrap();
        let profile_root = temp.path().join("accounts").join(key.to_string());

        assert_eq!(paths.account_data(), profile_root.join("account_data.json"));
        assert_eq!(
            paths.char_list_cache(),
            profile_root.join("cache").join("char_list.xml")
        );
        assert_eq!(
            paths.loot_database(),
            profile_root.join("databases").join("loot_history.db")
        );
        assert_eq!(
            paths.api_diagnostics(),
            Some(profile_root.join("diagnostics").join("api").as_path())
        );
        // Profile mode never exposes the legacy flat token path.
        assert!(paths.legacy_access_token().is_none());
    }

    #[test]
    fn two_profiles_never_share_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = StorageRoot::from_path(temp.path()).unwrap();
        let first = AccountPaths::new(root.clone(), AccountKey::generate());
        let second = AccountPaths::new(root, AccountKey::generate());
        let a = AccountPersistencePaths::for_profile(&first).unwrap();
        let b = AccountPersistencePaths::for_profile(&second).unwrap();

        assert_ne!(a.account_data(), b.account_data());
        assert_ne!(a.loot_database(), b.loot_database());
        assert_ne!(a.combat_database(), b.combat_database());
        assert_ne!(a.chat_logs(), b.chat_logs());
    }
}

//! Typed, exhaustive inventory of the legacy flat-layout sources.
//!
//! The inventory is normative: it encodes the exact known source paths from
//! `design.md` section 9.1 together with their classification and the validator
//! used to prove a durable copy. Nothing here resolves a real
//! `%LOCALAPPDATA%`/`%APPDATA%`; every path is relative to an explicit local
//! [`StorageRoot`](crate::storage::StorageRoot) or an explicit Roaming root the
//! caller supplies.

use std::path::PathBuf;

/// Where a legacy source physically lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceLocation {
    /// Relative to the explicit local storage root.
    Local,
    /// Relative to the explicit Roaming RealmHound root, when one is provided.
    Roaming,
}

/// Classification of a legacy source, driving its migration action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceClass {
    /// Installation-wide data left untouched by migration.
    Global,
    /// Account-scoped data assigned to the known account or quarantined.
    Attributable,
    /// Legacy duplicate data imported only by documented precedence.
    Ambiguous,
    /// Diagnostics or orphan files preserved in the migration backup only.
    Obsolete,
    /// Live SQLite database migrated by the checkpoint/backup algorithm.
    ActiveDatabase,
    /// The plaintext access token: secure-import then discard, never copied.
    Credential,
}

/// Validator that must prove a durable copy before an original is moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Validator {
    /// Parse as JSON.
    Json,
    /// Parse `char_list.xml` syntactically.
    Xml,
    /// Parse a RealmShark `dungeon.stats` document syntactically.
    RealmShark,
    /// Checkpoint + backup + schema/quick_check validation.
    Sqlite,
    /// Byte-for-byte hash equality of a single opaque file.
    OpaqueFile,
    /// Recursive tree-manifest equality of an opaque directory.
    OpaqueTree,
    /// No copy is produced (global untouched or credential).
    None,
}

/// A single typed legacy source with its exact relative path and action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryItem {
    /// Stable identifier used as the journal per-item key.
    pub id: &'static str,
    /// Physical location root for [`Self::source`].
    pub location: SourceLocation,
    /// Exact relative source path below its location root.
    pub source: PathBuf,
    /// Classification driving the migration action.
    pub class: SourceClass,
    /// Validator that proves a durable copy.
    pub validator: Validator,
}

impl InventoryItem {
    fn local(id: &'static str, source: &str, class: SourceClass, validator: Validator) -> Self {
        Self {
            id,
            location: SourceLocation::Local,
            source: relative(source),
            class,
            validator,
        }
    }

    /// Whether this source is copied into the known profile or quarantine.
    pub fn is_copied(&self) -> bool {
        matches!(
            self.class,
            SourceClass::Attributable | SourceClass::Ambiguous | SourceClass::ActiveDatabase
        )
    }

    /// Whether this source is only preserved in the timestamped backup.
    pub fn is_backup_only(&self) -> bool {
        matches!(self.class, SourceClass::Obsolete)
    }
}

fn relative(source: &str) -> PathBuf {
    source.split('/').fold(PathBuf::new(), |mut acc, part| {
        acc.push(part);
        acc
    })
}

/// Return the exhaustive, ordered flat-layout inventory.
///
/// The Roaming `characters_cache.json` entry is included only when the caller
/// supplied an explicit Roaming root.
pub fn flat_layout_inventory(has_roaming_root: bool) -> Vec<InventoryItem> {
    let mut items = vec![
        InventoryItem::local(
            "account_data.json",
            "account_data.json",
            SourceClass::Attributable,
            Validator::Json,
        ),
        InventoryItem::local(
            "characters_cache.json",
            "characters_cache.json",
            SourceClass::Ambiguous,
            Validator::Json,
        ),
    ];

    if has_roaming_root {
        items.push(InventoryItem {
            id: "roaming_characters_cache.json",
            location: SourceLocation::Roaming,
            source: relative("characters_cache.json"),
            class: SourceClass::Ambiguous,
            validator: Validator::Json,
        });
    }

    items.extend([
        InventoryItem::local(
            "live_vault.json",
            "live_vault.json",
            SourceClass::Ambiguous,
            Validator::Json,
        ),
        InventoryItem::local(
            "loot_history.db",
            "loot_history.db",
            SourceClass::ActiveDatabase,
            Validator::Sqlite,
        ),
        InventoryItem::local(
            "combat_history.db",
            "combat_history.db",
            SourceClass::ActiveDatabase,
            Validator::Sqlite,
        ),
        InventoryItem::local(
            "quests.json",
            "quests.json",
            SourceClass::Attributable,
            Validator::Json,
        ),
        InventoryItem::local(
            "char_list.xml",
            "char_list.xml",
            SourceClass::Attributable,
            Validator::Xml,
        ),
        InventoryItem::local(
            "imports/dungeon.stats",
            "imports/dungeon.stats",
            SourceClass::Attributable,
            Validator::RealmShark,
        ),
        InventoryItem::local(
            "logs/chat",
            "logs/chat",
            SourceClass::Attributable,
            Validator::OpaqueTree,
        ),
        InventoryItem::local(
            "event_notification_log.csv",
            "event_notification_log.csv",
            SourceClass::Attributable,
            Validator::OpaqueFile,
        ),
        InventoryItem::local(
            "watchlist_detections.log",
            "watchlist_detections.log",
            SourceClass::Attributable,
            Validator::OpaqueFile,
        ),
        InventoryItem::local(
            "access_token.txt",
            "access_token.txt",
            SourceClass::Credential,
            Validator::None,
        ),
        // Obsolete diagnostics/orphans: preserved in the migration backup only.
        InventoryItem::local(
            "party_sightings.json",
            "party_sightings.json",
            SourceClass::Obsolete,
            Validator::OpaqueFile,
        ),
        InventoryItem::local(
            "party_snapshot.json",
            "party_snapshot.json",
            SourceClass::Obsolete,
            Validator::OpaqueFile,
        ),
        InventoryItem::local(
            "debug",
            "debug",
            SourceClass::Obsolete,
            Validator::OpaqueTree,
        ),
        // Root API snapshots other than char_list.xml: backup-only.
        InventoryItem::local(
            "account_listPowerUpStats.xml",
            "account_listPowerUpStats.xml",
            SourceClass::Obsolete,
            Validator::OpaqueFile,
        ),
        InventoryItem::local(
            "missions_getClientSeasons.xml",
            "missions_getClientSeasons.xml",
            SourceClass::Obsolete,
            Validator::OpaqueFile,
        ),
        InventoryItem::local(
            "missions_getPlayerMissions.xml",
            "missions_getPlayerMissions.xml",
            SourceClass::Obsolete,
            Validator::OpaqueFile,
        ),
        // Obsolete manual export and ignored-test asset output: carved out of the
        // broad global `assets/` tree and preserved in the migration backup only.
        InventoryItem::local(
            "assets/enchantments16x16_export.png",
            "assets/enchantments16x16_export.png",
            SourceClass::Obsolete,
            Validator::OpaqueFile,
        ),
        InventoryItem::local(
            "assets/debug_sprites",
            "assets/debug_sprites",
            SourceClass::Obsolete,
            Validator::OpaqueTree,
        ),
        InventoryItem::local(
            "assets/debug_mapobjects_subatlases",
            "assets/debug_mapobjects_subatlases",
            SourceClass::Obsolete,
            Validator::OpaqueTree,
        ),
        // Global installation-wide data left untouched.
        InventoryItem::local(
            "settings.json",
            "settings.json",
            SourceClass::Global,
            Validator::None,
        ),
        InventoryItem::local(
            "watchlist.txt",
            "watchlist.txt",
            SourceClass::Global,
            Validator::None,
        ),
        InventoryItem::local("logs", "logs", SourceClass::Global, Validator::None),
        InventoryItem::local("captures", "captures", SourceClass::Global, Validator::None),
        InventoryItem::local("assets", "assets", SourceClass::Global, Validator::None),
        InventoryItem::local(
            "sounds/custom",
            "sounds/custom",
            SourceClass::Global,
            Validator::None,
        ),
    ]);

    items
}

/// Return the inventory item with the given id, if any.
pub fn item(id: &str, has_roaming_root: bool) -> Option<InventoryItem> {
    flat_layout_inventory(has_roaming_root)
        .into_iter()
        .find(|item| item.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_ids_are_unique() {
        let items = flat_layout_inventory(true);
        let mut ids: Vec<&str> = items.iter().map(|item| item.id).collect();
        ids.sort_unstable();
        let unique = ids.len();
        ids.dedup();
        assert_eq!(unique, ids.len());
    }

    #[test]
    fn roaming_entry_present_only_with_roaming_root() {
        assert!(item("roaming_characters_cache.json", false).is_none());
        assert!(item("roaming_characters_cache.json", true).is_some());
    }

    #[test]
    fn access_token_is_credential_and_never_copied() {
        let token = item("access_token.txt", false).unwrap();
        assert_eq!(token.class, SourceClass::Credential);
        assert!(!token.is_copied());
        assert!(!token.is_backup_only());
    }

    #[test]
    fn databases_use_sqlite_validator() {
        for id in ["loot_history.db", "combat_history.db"] {
            let db = item(id, false).unwrap();
            assert_eq!(db.class, SourceClass::ActiveDatabase);
            assert_eq!(db.validator, Validator::Sqlite);
        }
    }

    #[test]
    fn global_sources_are_untouched() {
        for id in [
            "settings.json",
            "watchlist.txt",
            "logs",
            "captures",
            "assets",
            "sounds/custom",
        ] {
            let global = item(id, false).unwrap();
            assert_eq!(global.class, SourceClass::Global);
            assert!(!global.is_copied());
            assert!(!global.is_backup_only());
        }
    }

    #[test]
    fn chat_logs_move_independently_from_root_logs() {
        let chat = item("logs/chat", false).unwrap();
        assert_eq!(chat.class, SourceClass::Attributable);
        let logs = item("logs", false).unwrap();
        assert_eq!(logs.class, SourceClass::Global);
    }

    #[test]
    fn obsolete_orphans_are_backup_only() {
        for id in [
            "party_sightings.json",
            "party_snapshot.json",
            "debug",
            "account_listPowerUpStats.xml",
            "missions_getClientSeasons.xml",
            "missions_getPlayerMissions.xml",
            "assets/enchantments16x16_export.png",
            "assets/debug_sprites",
            "assets/debug_mapobjects_subatlases",
        ] {
            let obsolete = item(id, false).unwrap();
            assert!(obsolete.is_backup_only(), "{id} should be backup-only");
            assert!(!obsolete.is_copied(), "{id} must not be copied");
        }
    }

    #[test]
    fn broad_assets_stays_global_while_carved_paths_are_backup_only() {
        // The broad assets directory remains global and untouched.
        let assets = item("assets", false).unwrap();
        assert_eq!(assets.class, SourceClass::Global);
        // Specific obsolete asset paths are carved out for backup-only handling.
        for id in [
            "assets/enchantments16x16_export.png",
            "assets/debug_sprites",
            "assets/debug_mapobjects_subatlases",
        ] {
            let carved = item(id, false).unwrap();
            assert_eq!(carved.class, SourceClass::Obsolete);
        }
    }

    #[test]
    fn inventory_is_the_exact_expected_set() {
        let mut ids: Vec<&str> = flat_layout_inventory(false)
            .iter()
            .map(|item| item.id)
            .collect();
        ids.sort_unstable();
        let mut expected = vec![
            "account_data.json",
            "characters_cache.json",
            "live_vault.json",
            "loot_history.db",
            "combat_history.db",
            "quests.json",
            "char_list.xml",
            "imports/dungeon.stats",
            "logs/chat",
            "event_notification_log.csv",
            "watchlist_detections.log",
            "access_token.txt",
            "party_sightings.json",
            "party_snapshot.json",
            "debug",
            "account_listPowerUpStats.xml",
            "missions_getClientSeasons.xml",
            "missions_getPlayerMissions.xml",
            "assets/enchantments16x16_export.png",
            "assets/debug_sprites",
            "assets/debug_mapobjects_subatlases",
            "settings.json",
            "watchlist.txt",
            "logs",
            "captures",
            "assets",
            "sounds/custom",
        ];
        expected.sort_unstable();
        assert_eq!(ids, expected);

        // With a Roaming root the only difference is the Roaming cache entry.
        let mut with_roaming: Vec<&str> = flat_layout_inventory(true)
            .iter()
            .map(|item| item.id)
            .collect();
        with_roaming.sort_unstable();
        expected.push("roaming_characters_cache.json");
        expected.sort_unstable();
        assert_eq!(with_roaming, expected);
    }
}

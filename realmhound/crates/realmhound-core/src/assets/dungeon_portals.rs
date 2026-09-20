//! Dungeon to portal mapping for sprite display.
//!
//! Maps dungeon names (as stored in loot records) to portal object IDs
//! for rendering dungeon portal sprites next to loot drops.

use std::collections::HashMap;
use std::sync::OnceLock;

/// Global dungeon portal mapping.
static DUNGEON_PORTALS: OnceLock<DungeonPortalMap> = OnceLock::new();

/// Base object id for the 8 legacy dungeon portals whose original sprites are
/// missing from the game's current assets and are instead shipped as embedded
/// PNGs in the UI binary. `get_portal_id` returns `BASE + index` for these so
/// the renderer can detect them via [`legacy_embed_portal_index`] and draw the
/// embedded art. The base is far above any real object id so it never collides.
pub const LEGACY_EMBED_PORTAL_BASE: i32 = 0x4C50_0000;

/// Number of embedded legacy portals (see [`LEGACY_EMBED_PORTAL_BASE`]).
pub const LEGACY_EMBED_PORTAL_COUNT: i32 = 8;

/// If `id` is an embedded legacy portal sentinel, return its 0-based index
/// (`0..LEGACY_EMBED_PORTAL_COUNT`); otherwise `None`.
pub fn legacy_embed_portal_index(id: i32) -> Option<usize> {
    if (LEGACY_EMBED_PORTAL_BASE..LEGACY_EMBED_PORTAL_BASE + LEGACY_EMBED_PORTAL_COUNT)
        .contains(&id)
    {
        Some((id - LEGACY_EMBED_PORTAL_BASE) as usize)
    } else {
        None
    }
}

/// Get the global dungeon portal mapping.
pub fn get_dungeon_portal_map() -> &'static DungeonPortalMap {
    DUNGEON_PORTALS.get_or_init(DungeonPortalMap::new)
}

/// Mapping from dungeon names to portal object IDs.
pub struct DungeonPortalMap {
    /// Map from normalized dungeon name to portal object ID
    name_to_portal: HashMap<String, i32>,
    /// Map from localization keys to display names
    localization_map: HashMap<String, String>,
}

impl DungeonPortalMap {
    /// Create a new dungeon portal map with all known mappings.
    pub fn new() -> Self {
        let mut name_to_portal = HashMap::new();
        let mut localization_map = HashMap::new();

        // Localization key mappings
        localization_map.insert("{s.rotmg}".to_string(), "Realm".to_string());
        localization_map.insert("{s.oryx_s_castle}".to_string(), "Oryx's Castle".to_string());
        localization_map.insert("{s.vault}".to_string(), "Vault".to_string());
        localization_map.insert("{s.wine_cellar}".to_string(), "Wine Cellar".to_string());
        localization_map.insert("{s.nexus}".to_string(), "Nexus".to_string());

        // Portal ID mappings (dungeon name -> portal object ID)
        // These IDs are from ObjectID.list Portal class objects

        // Special locations that may not have Portal objects in game data
        // or need localization key handling
        name_to_portal.insert("vault".to_string(), 1824);
        name_to_portal.insert("realm".to_string(), 1796); // Realm portal (gray arch)
        name_to_portal.insert("the realm".to_string(), 1796);

        // Oryx locations (may have special naming)
        name_to_portal.insert("oryx's castle".to_string(), 3465);
        name_to_portal.insert("oryx's chamber".to_string(), 1588);
        name_to_portal.insert("wine cellar".to_string(), 578);
        // Hardcoded because the dynamic Portal-class lookup can resolve to a
        // different, larger-looking "Oryx Sanctuary" portal object depending
        // on locally extracted game assets; this id is the small icon used
        // in the in-game Dungeon Completions page.
        name_to_portal.insert("oryx's sanctuary".to_string(), 6218);

        // Legacy/removed dungeon portals that still have original art in the
        // game's Portal-class objects (map display name -> object id).
        name_to_portal.insert("legacy forest maze".to_string(), 24372); // Forest Maze Portal (0x5f34)
        name_to_portal.insert("legacy lair of shaitan".to_string(), 8853); // Old Lair of Shaitan Portal (0x2295)
        name_to_portal.insert("bilgewater's grotto".to_string(), 28811); // Bilgewater's Grotto Portal (0x708b)
        name_to_portal.insert("ivory wyvern portal".to_string(), 30014); // Old Lair of Draconis Portal (0x753e)
                                                                         // Aliases for the internal <DungeonName> values used by mission cond
                                                                         // targets (which differ from the map display names above).
        name_to_portal.insert("legacy bilgewater's grotto".to_string(), 28811);
        name_to_portal.insert("the ivory wyvern".to_string(), 30014);
        // Legacy Deadwater Docks shares the Bilgewater's Grotto portal art.
        name_to_portal.insert("legacy deadwater docks".to_string(), 28811);
        // Combined display names (ALL_DUNGEONS) fold each of the above into one
        // completion counter; resolve them to the same portal art.
        name_to_portal.insert("legacy deadwater docks & grotto".to_string(), 28811);
        name_to_portal.insert("legacy lair of draconis & ivory".to_string(), 30014);

        // Legacy portals whose original sprites are missing from current game
        // assets: route to embedded PNGs via sentinel ids. Index order must
        // match the UI's `EmbeddedIcon::legacy_portal` mapping.
        name_to_portal.insert("legacy pirate cave".to_string(), LEGACY_EMBED_PORTAL_BASE);
        name_to_portal.insert(
            "legacy spider den".to_string(),
            LEGACY_EMBED_PORTAL_BASE + 1,
        );
        name_to_portal.insert(
            "legacy sprite world".to_string(),
            LEGACY_EMBED_PORTAL_BASE + 2,
        );
        name_to_portal.insert(
            "legacy undead lair".to_string(),
            LEGACY_EMBED_PORTAL_BASE + 3,
        );
        name_to_portal.insert(
            "legacy abyss of demons".to_string(),
            LEGACY_EMBED_PORTAL_BASE + 4,
        );
        name_to_portal.insert(
            "legacy the crawling depths".to_string(),
            LEGACY_EMBED_PORTAL_BASE + 5,
        );
        name_to_portal.insert(
            "legacy woodland labyrinth".to_string(),
            LEGACY_EMBED_PORTAL_BASE + 6,
        );
        name_to_portal.insert(
            "legacy the shatters".to_string(),
            LEGACY_EMBED_PORTAL_BASE + 7,
        );

        // Note: Most dungeons are now looked up dynamically from game assets
        // using find_portal_for_dungeon() - no need to hardcode them here.

        Self {
            name_to_portal,
            localization_map,
        }
    }

    /// Normalize a dungeon name for lookup.
    /// Handles localization keys and case-insensitive matching.
    pub fn normalize_dungeon_name(&self, dungeon: &str) -> String {
        // Check for localization key first
        if let Some(display_name) = self.localization_map.get(dungeon) {
            return display_name.clone();
        }

        // Return as-is (will be lowercased for portal lookup)
        dungeon.to_string()
    }

    /// Get the portal object ID for a dungeon name.
    /// First tries dynamic lookup from loaded game assets, then falls back to hardcoded map.
    /// Returns None if no portal mapping exists.
    pub fn get_portal_id(&self, dungeon: &str) -> Option<i32> {
        // First normalize (resolve localization keys)
        let normalized = self.normalize_dungeon_name(dungeon);

        // Try hardcoded lookup first (for special cases like localization keys)
        if let Some(&id) = self.name_to_portal.get(&normalized.to_lowercase()) {
            return Some(id);
        }

        // Fall back to dynamic lookup from loaded game assets
        use super::get_asset_manager;
        get_asset_manager().find_portal_for_dungeon(&normalized)
    }

    /// Check if a dungeon name represents the Nexus.
    pub fn is_nexus(&self, dungeon: &str) -> bool {
        dungeon == "{s.nexus}" || dungeon.eq_ignore_ascii_case("nexus")
    }

    /// Check if a dungeon name represents the Realm (godlands).
    pub fn is_realm(&self, dungeon: &str) -> bool {
        dungeon == "{s.rotmg}"
            || dungeon.eq_ignore_ascii_case("realm")
            || dungeon.eq_ignore_ascii_case("realm of the mad god")
    }
}

impl Default for DungeonPortalMap {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_localization_normalization() {
        let map = DungeonPortalMap::new();

        assert_eq!(map.normalize_dungeon_name("{s.rotmg}"), "Realm");
        assert_eq!(map.normalize_dungeon_name("{s.vault}"), "Vault");
        assert_eq!(map.normalize_dungeon_name("{s.wine_cellar}"), "Wine Cellar");
        assert_eq!(
            map.normalize_dungeon_name("{s.oryx_s_castle}"),
            "Oryx's Castle"
        );
    }

    #[test]
    fn test_portal_lookup() {
        let map = DungeonPortalMap::new();

        // Test static entries (localization keys and special cases)
        assert_eq!(map.get_portal_id("{s.vault}"), Some(1824));
        assert_eq!(map.get_portal_id("{s.wine_cellar}"), Some(578));
        assert_eq!(map.get_portal_id("vault"), Some(1824));
        assert_eq!(map.get_portal_id("Vault"), Some(1824));
        assert_eq!(map.get_portal_id("realm"), Some(1796));

        // Note: Most dungeons now use dynamic lookup from game assets,
        // which requires the asset manager to be initialized.
        // Those lookups are tested in integration tests.
    }

    #[test]
    fn test_legacy_portal_mappings() {
        let map = DungeonPortalMap::new();

        // Game-asset portals resolve to their real object ids (case-insensitive).
        assert_eq!(map.get_portal_id("Legacy Forest Maze"), Some(24372));
        assert_eq!(map.get_portal_id("Legacy Lair of Shaitan"), Some(8853));
        assert_eq!(map.get_portal_id("Bilgewater's Grotto"), Some(28811));
        assert_eq!(map.get_portal_id("Ivory Wyvern Portal"), Some(30014));
        // Internal DungeonName targets (missions) and folded siblings resolve to
        // the same portal art.
        assert_eq!(map.get_portal_id("Legacy Bilgewater's Grotto"), Some(28811));
        assert_eq!(map.get_portal_id("Legacy Deadwater Docks"), Some(28811));
        assert_eq!(map.get_portal_id("The Ivory Wyvern"), Some(30014));
        // Combined ALL_DUNGEONS display names.
        assert_eq!(
            map.get_portal_id("Legacy Deadwater Docks & Grotto"),
            Some(28811)
        );
        assert_eq!(
            map.get_portal_id("Legacy Lair of Draconis & Ivory"),
            Some(30014)
        );

        // Embedded portals resolve to sentinel ids in index order; the index
        // must round-trip so the UI selects the matching embedded PNG.
        let embedded = [
            ("Legacy Pirate Cave", 0usize),
            ("Legacy Spider Den", 1),
            ("Legacy Sprite World", 2),
            ("Legacy Undead Lair", 3),
            ("Legacy Abyss of Demons", 4),
            ("Legacy The Crawling Depths", 5),
            ("Legacy Woodland Labyrinth", 6),
            ("Legacy The Shatters", 7),
        ];
        for (name, idx) in embedded {
            let id = map.get_portal_id(name).unwrap();
            assert_eq!(id, LEGACY_EMBED_PORTAL_BASE + idx as i32, "{name}");
            assert_eq!(legacy_embed_portal_index(id), Some(idx), "{name}");
        }

        // Ordinary object ids are not treated as embedded portals.
        assert_eq!(legacy_embed_portal_index(28811), None);
    }

    #[test]
    fn test_nexus_detection() {
        let map = DungeonPortalMap::new();

        assert!(map.is_nexus("{s.nexus}"));
        assert!(map.is_nexus("Nexus"));
        assert!(map.is_nexus("nexus"));
        assert!(!map.is_nexus("Vault"));
    }

    #[test]
    fn test_realm_detection() {
        let map = DungeonPortalMap::new();

        assert!(map.is_realm("{s.rotmg}"));
        assert!(map.is_realm("Realm"));
        assert!(!map.is_realm("Lost Halls"));
    }
}

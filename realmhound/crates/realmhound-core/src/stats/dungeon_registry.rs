//! Dungeon registry: canonical name resolution, ORG_ code mapping, and difficulty lookup.
//!
//! Provides a unified layer for correlating dungeon identities across:
//! - Loot DB `dungeon` column strings
//! - `ALL_DUNGEONS` display names (PCStats)
//! - Key object names (for difficulty)
//! - `ORG_` labels on items (for item→dungeon mapping)

use std::collections::HashMap;
use std::sync::OnceLock;

use super::collections::ALL_DUNGEONS;

// ---------------------------------------------------------------------------
// ORG_ code → canonical dungeon name mapping
// ---------------------------------------------------------------------------

/// Static mapping from `ORG_` label codes to canonical dungeon display names.
/// These must match the names used in `ALL_DUNGEONS` and the loot DB.
static ORG_TO_DUNGEON: &[(&str, &str)] = &[
    ("3D", "The Third Dimension"),
    ("ABYSS", "Abyss of Demons"),
    ("CASTLE", "Oryx's Castle"),
    ("CDEPTHS", "The Crawling Depths"),
    ("CEM", "Haunted Cemetery"),
    ("CLAND", "Candyland Hunting Grounds"),
    ("CRYSTAL", "Crystal Cavern"),
    ("CRYSTAL_S", "Crystal Cavern"),
    ("CULT", "Cultist Hideout"),
    ("DAVY", "Davy Jones' Locker"),
    ("DDOCKS", "Deadwater Docks"),
    ("ENCORE", "Puppet Master's Encore"),
    ("FMAZE", "Forest Maze"),
    ("FUNGAL", "Fungal Cavern"),
    ("HIVE", "The Hive"),
    ("HTT", "High Tech Terror"),
    ("ICECAVE", "Ice Tomb"),
    ("JUNGLE", "Forbidden Jungle"),
    ("KOGBOLD", "Kogbold Steamworks"),
    ("KOGBOLD_CORE", "Kogbold Steamworks"),
    ("KOGBOLD_CORE_SHINY", "Kogbold Steamworks"),
    ("LAB", "Mad Lab"),
    ("LH", "Lost Halls"),
    ("LIBRARY", "Cursed Library"),
    ("LOD", "Lair of Draconis"),
    ("MANOR", "Manor of the Immortals"),
    ("MOONLIGHT", "Moonlight Village"),
    ("MOONLIGHT_CORE", "Moonlight Village"),
    ("MTEMPLE", "Mountain Temple"),
    ("MWOODS", "Magic Woods"),
    ("NEST", "The Nest"),
    ("ORYXMAS", "Santa's Workshop"),
    ("OSANC", "Oryx's Sanctuary"),
    ("OSANC_S", "Oryx's Sanctuary"),
    ("OTRENCH", "Ocean Trench"),
    ("PARASITE", "Parasite Chambers"),
    ("PCAVE", "Pirate Cave"),
    ("PUPPET", "Puppet Master's Theatre"),
    ("REEF", "Cnidarian Reef"),
    ("RUINS", "Ancient Ruins"),
    ("SETPIECE", "Realm"),
    ("SEWER", "Toxic Sewers"),
    ("SHAITAN", "Lair of Shaitan"),
    ("SHTT", "The Shatters"),
    ("SHTT_S", "The Shatters"),
    ("SPECTRAL", "Spectral Penitentiary"),
    ("SPECTRAL_CORE", "Spectral Penitentiary"),
    ("SPECTRAL_CORE_SHINY", "Spectral Penitentiary"),
    ("SPIDER", "Spider Den"),
    ("SPIT", "Snake Pit"),
    ("SPRITE", "Sprite World"),
    ("TAVERN", "The Tavern"),
    ("THICKET", "Secluded Thicket"),
    ("TOMB", "Tomb of the Ancients"),
    ("UDL", "Undead Lair"),
    ("UMI", "Kitsune Umi"),
    ("VOID", "The Void"),
    ("WETLANDS", "Sulfurous Wetlands"),
    ("WLAB", "Woodland Labyrinth"),
];

/// Key name suffixes to strip when deriving dungeon name from key item names.
static KEY_SUFFIXES: &[&str] = &[
    " Solo Key 2",
    " Solo Key",
    " Lucky Guild Key",
    " Guild Key",
    " Augmented Key",
    " Enchanted Key",
    " PermaKey",
    " Key Encore",
    " Key 2",
    " Key",
];

// ---------------------------------------------------------------------------
// DungeonRegistry
// ---------------------------------------------------------------------------

static REGISTRY: OnceLock<DungeonRegistry> = OnceLock::new();

/// Get the global dungeon registry.
pub fn get_dungeon_registry() -> &'static DungeonRegistry {
    REGISTRY.get_or_init(DungeonRegistry::new)
}

/// Unified dungeon identity resolver.
pub struct DungeonRegistry {
    /// ORG code → canonical name
    org_to_name: HashMap<&'static str, &'static str>,
    /// Lowercase alias → canonical name (for fuzzy matching across systems)
    aliases: HashMap<String, String>,
}

impl DungeonRegistry {
    fn new() -> Self {
        let org_to_name: HashMap<&'static str, &'static str> =
            ORG_TO_DUNGEON.iter().copied().collect();

        let mut aliases: HashMap<String, String> = HashMap::new();

        // Build aliases from ORG_TO_DUNGEON canonical names
        for &(_, canonical) in ORG_TO_DUNGEON {
            aliases.insert(canonical.to_lowercase(), canonical.to_string());
        }

        // Also register ALL_DUNGEONS names (catches entries not in ORG_TO_DUNGEON)
        for d in ALL_DUNGEONS.iter() {
            aliases
                .entry(d.name.to_lowercase())
                .or_insert_with(|| d.name.to_string());
        }

        // Add known alternate forms that appear in loot DB or key names
        let extra_aliases: &[(&str, &str)] = &[
            ("oryx's castle", "Oryx's Castle"),
            ("oryx's chamber", "Oryx's Chamber"),
            ("wine cellar", "Wine Cellar"),
            ("ice cave", "Ice Tomb"),
            ("davy jones' locker", "Davy Jones' Locker"),
            ("belladonna's garden", "Belladonna's Garden"),
            ("santa's workshop", "Santa's Workshop"),
            ("santa workshop", "Santa's Workshop"),
            ("shatters", "The Shatters"),
            ("nest", "The Nest"),
            ("void", "The Void"),
            ("hive", "The Hive"),
            ("crawling depths", "The Crawling Depths"),
            ("third dimension", "The Third Dimension"),
            ("machine", "The Machine"),
            ("tavern", "The Tavern"),
            ("bella's", "Belladonna's Garden"),
            ("davy's", "Davy Jones' Locker"),
            ("shaitan's", "Lair of Shaitan"),
            ("reef", "Cnidarian Reef"),
            ("lab", "Mad Lab"),
            ("candy", "Candyland Hunting Grounds"),
            ("cemetery", "Haunted Cemetery"),
            ("manor", "Manor of the Immortals"),
            ("theatre", "Puppet Master's Theatre"),
            ("totem", "Forbidden Jungle"),
            ("treasure map", "Cave of a Thousand Treasures"),
            ("trials of cronus", "The Trials of Cronus"),
            ("the realm", "Realm"),
            // Legacy variants folded into a single combined completion row:
            // both the "Legacy ..." internal DungeonName and the old display
            // name normalize to the combined ALL_DUNGEONS name.
            ("bilgewater's grotto", "Legacy Deadwater Docks & Grotto"),
            (
                "legacy bilgewater's grotto",
                "Legacy Deadwater Docks & Grotto",
            ),
            ("legacy deadwater docks", "Legacy Deadwater Docks & Grotto"),
            ("ivory wyvern portal", "Legacy Lair of Draconis & Ivory"),
            ("the ivory wyvern", "Legacy Lair of Draconis & Ivory"),
            ("legacy lair of draconis", "Legacy Lair of Draconis & Ivory"),
        ];
        for &(alias, canonical) in extra_aliases {
            aliases.insert(alias.to_lowercase(), canonical.to_string());
        }

        Self {
            org_to_name,
            aliases,
        }
    }

    /// Resolve an ORG_ code (without the "ORG_" prefix) to a canonical dungeon name.
    pub fn resolve_org_code(&self, code: &str) -> Option<&'static str> {
        self.org_to_name.get(code).copied()
    }

    /// Normalize a dungeon name to its canonical form.
    /// Handles case differences, known aliases, and localization keys.
    pub fn normalize(&self, name: &str) -> String {
        // Handle localization keys
        if name.starts_with("{s.") {
            let inner = &name[3..name.len().saturating_sub(1)];
            let cleaned = inner.replace('_', " ");
            return self.normalize(&cleaned);
        }

        let lower = name.to_lowercase();
        if let Some(canonical) = self.aliases.get(&lower) {
            return canonical.clone();
        }

        // Return as-is if no alias found (preserves original casing)
        name.to_string()
    }

    /// Derive a dungeon name from a key item name.
    /// Strips known suffixes like " Key", " Augmented Key", etc.
    /// Falls back to alias lookup for non-standard key names (e.g. "Treasure Map").
    pub fn dungeon_from_key_name(&self, key_name: &str) -> Option<String> {
        for suffix in KEY_SUFFIXES {
            if let Some(base) = key_name.strip_suffix(suffix) {
                let base = base.strip_prefix("Cursed ").unwrap_or(base);
                return Some(self.normalize(base));
            }
        }
        // Fallback: try alias lookup for non-standard names
        let lower = key_name.to_lowercase();
        self.aliases.get(&lower).cloned()
    }

    /// Extract ORG_ codes from a comma-separated labels string.
    /// Returns the resolved canonical dungeon names.
    pub fn dungeons_from_labels(&self, labels: &str) -> Vec<&'static str> {
        labels
            .split(',')
            .filter_map(|label| {
                label
                    .strip_prefix("ORG_")
                    .and_then(|code| self.resolve_org_code(code))
            })
            .collect()
    }
}

impl std::fmt::Debug for DungeonRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DungeonRegistry")
            .field("org_codes", &self.org_to_name.len())
            .field("aliases", &self.aliases.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_org_code_resolution() {
        let reg = get_dungeon_registry();
        assert_eq!(reg.resolve_org_code("PCAVE"), Some("Pirate Cave"));
        assert_eq!(reg.resolve_org_code("OSANC"), Some("Oryx's Sanctuary"));
        assert_eq!(reg.resolve_org_code("SHTT"), Some("The Shatters"));
        assert_eq!(reg.resolve_org_code("UNKNOWN"), None);
    }

    #[test]
    fn test_name_normalization() {
        let reg = get_dungeon_registry();
        assert_eq!(reg.normalize("pirate cave"), "Pirate Cave");
        assert_eq!(reg.normalize("THE SHATTERS"), "The Shatters");
        assert_eq!(reg.normalize("ice cave"), "Ice Tomb");
    }

    #[test]
    fn test_legacy_sibling_merge() {
        let reg = get_dungeon_registry();
        // Both the internal DungeonName and the folded sibling normalize to the
        // combined ALL_DUNGEONS completion row so drops + completions merge.
        for n in [
            "Legacy Bilgewater's Grotto",
            "Legacy Deadwater Docks",
            "Bilgewater's Grotto",
        ] {
            assert_eq!(reg.normalize(n), "Legacy Deadwater Docks & Grotto", "{n}");
        }
        for n in [
            "Legacy Lair of Draconis",
            "The Ivory Wyvern",
            "Ivory Wyvern Portal",
        ] {
            assert_eq!(reg.normalize(n), "Legacy Lair of Draconis & Ivory", "{n}");
        }
        // The modern (non-legacy) Deadwater Docks must not be folded.
        assert_eq!(reg.normalize("Deadwater Docks"), "Deadwater Docks");
    }

    #[test]
    fn test_dungeon_from_key_name() {
        let reg = get_dungeon_registry();
        assert_eq!(
            reg.dungeon_from_key_name("Pirate Cave Key"),
            Some("Pirate Cave".to_string())
        );
        assert_eq!(
            reg.dungeon_from_key_name("Shatters Augmented Key"),
            Some("The Shatters".to_string())
        );
        assert_eq!(
            reg.dungeon_from_key_name("Cursed Snake Pit Key"),
            Some("Snake Pit".to_string())
        );
    }

    #[test]
    fn test_dungeons_from_labels() {
        let reg = get_dungeon_registry();
        let labels =
            "EQUIPMENT,WEAPON,SWORD,UT,TAB_UT,POWERTIER_B,TRADEABLE,XPBONUS,ORG_ABYSS,DEMON_BLADE";
        let dungeons = reg.dungeons_from_labels(labels);
        assert_eq!(dungeons, vec!["Abyss of Demons"]);
    }

    #[test]
    fn test_multiple_org_labels() {
        let reg = get_dungeon_registry();
        let labels = "ORG_PCAVE,ORG_ABYSS";
        let dungeons = reg.dungeons_from_labels(labels);
        assert_eq!(dungeons.len(), 2);
        assert!(dungeons.contains(&"Pirate Cave"));
        assert!(dungeons.contains(&"Abyss of Demons"));
    }
}

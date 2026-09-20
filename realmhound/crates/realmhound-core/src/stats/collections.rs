//! Dungeon collection definitions for fame bonuses.
//!
//! Based on RealmEye fame bonuses: https://www.realmeye.com/wiki/fame-bonuses#dungeon

use crate::api::DungeonStats;

/// Type alias for dungeon accessor function.
pub type DungeonAccessor = fn(&DungeonStats) -> i32;

/// A dungeon with display name and stat accessor.
#[derive(Debug, Clone, Copy)]
pub struct Dungeon {
    /// Display name of the dungeon.
    pub name: &'static str,
    /// Function to get completion count from stats.
    pub get_count: DungeonAccessor,
}

/// Master list of all dungeons (single source of truth).
/// Used by both UI dungeon lists and collection definitions.
pub static ALL_DUNGEONS: &[Dungeon] = &[
    Dungeon {
        name: "Abyss of Demons",
        get_count: |s| s.abyss_of_demons,
    },
    Dungeon {
        name: "Advanced Kogbold Steamworks",
        get_count: |s| s.advanced_kogbold_steamworks,
    },
    Dungeon {
        name: "Ancient Ruins",
        get_count: |s| s.ancient_ruins,
    },
    Dungeon {
        name: "Battle for the Nexus",
        get_count: |s| s.battle_for_the_nexus,
    },
    Dungeon {
        name: "Beachzone",
        get_count: |s| s.beachzone,
    },
    Dungeon {
        name: "Belladonna's Garden",
        get_count: |s| s.belladonnas_garden,
    },
    Dungeon {
        name: "Candyland Hunting Grounds",
        get_count: |s| s.candyland_hunting_grounds,
    },
    Dungeon {
        name: "Cave of a Thousand Treasures",
        get_count: |s| s.cave_of_thousand_treasures,
    },
    Dungeon {
        name: "Cnidarian Reef",
        get_count: |s| s.cnidarian_reef,
    },
    Dungeon {
        name: "Crystal Cavern",
        get_count: |s| s.crystal_cavern,
    },
    Dungeon {
        name: "Cultist Hideout",
        get_count: |s| s.cultist_hideout,
    },
    Dungeon {
        name: "Cursed Library",
        get_count: |s| s.cursed_library,
    },
    Dungeon {
        name: "Davy Jones' Locker",
        get_count: |s| s.davy_jones_locker,
    },
    Dungeon {
        name: "Deadwater Docks",
        get_count: |s| s.deadwater_docks,
    },
    Dungeon {
        name: "Forax",
        get_count: |s| s.forax,
    },
    Dungeon {
        name: "Forbidden Jungle",
        get_count: |s| s.forbidden_jungle,
    },
    Dungeon {
        name: "Forest Maze",
        get_count: |s| s.forest_maze,
    },
    Dungeon {
        name: "Fungal Cavern",
        get_count: |s| s.fungal_cavern,
    },
    Dungeon {
        name: "Haunted Cemetery",
        get_count: |s| s.haunted_cemetery,
    },
    Dungeon {
        name: "Heroic Undead Lair",
        get_count: |s| s.undead_lair_heroic,
    },
    Dungeon {
        name: "Hidden Interregnum",
        get_count: |s| s.hidden_interregnum,
    },
    Dungeon {
        name: "High Tech Terror",
        get_count: |s| s.high_tech_terror,
    },
    Dungeon {
        name: "Ice Citadel",
        get_count: |s| s.ice_citadel,
    },
    Dungeon {
        name: "Ice Tomb",
        get_count: |s| s.ice_tomb,
    },
    Dungeon {
        name: "Infernal Abyss of Demons",
        get_count: |s| s.infernal_abyss_of_demons,
    },
    Dungeon {
        name: "Katalund",
        get_count: |s| s.katalund,
    },
    Dungeon {
        name: "Kogbold Steamworks",
        get_count: |s| s.kogbold_steamworks,
    },
    Dungeon {
        name: "Lair of Draconis",
        get_count: |s| s.lair_of_draconis,
    },
    Dungeon {
        name: "Lair of Shaitan",
        get_count: |s| s.lair_of_shaitan,
    },
    // In-game DUNGEONS list order follows Deca's internal `DungeonName` sort,
    // not the display name. This entry (DungeonName "Legacy Bilgewater's
    // Grotto") therefore sits inside the Legacy block, and the Ivory Wyvern
    // entry (DungeonName "The Ivory Wyvern") sorts under "The I" below. The
    // display names below fold in an untracked sibling dungeon that shares the
    // same completion counter (Legacy Deadwater Docks, Legacy Lair of Draconis).
    Dungeon {
        name: "Legacy Abyss of Demons",
        get_count: |s| s.legacy_abyss_of_demons,
    },
    Dungeon {
        name: "Legacy Deadwater Docks & Grotto",
        get_count: |s| s.bilgewaters_grotto,
    },
    Dungeon {
        name: "Legacy Forest Maze",
        get_count: |s| s.legacy_forest_maze,
    },
    Dungeon {
        name: "Legacy Heroic Abyss of Demons",
        get_count: |s| s.legacy_heroic_abyss_of_demons,
    },
    Dungeon {
        name: "Legacy Heroic Undead Lair",
        get_count: |s| s.legacy_heroic_undead_lair,
    },
    Dungeon {
        name: "Legacy Lair of Shaitan",
        get_count: |s| s.legacy_lair_of_shaitan,
    },
    Dungeon {
        name: "Legacy Pirate Cave",
        get_count: |s| s.legacy_pirate_cave,
    },
    Dungeon {
        name: "Legacy Spider Den",
        get_count: |s| s.legacy_spider_den,
    },
    Dungeon {
        name: "Legacy Sprite World",
        get_count: |s| s.legacy_sprite_world,
    },
    Dungeon {
        name: "Legacy The Crawling Depths",
        get_count: |s| s.legacy_the_crawling_depths,
    },
    Dungeon {
        name: "Legacy The Shatters",
        get_count: |s| s.legacy_the_shatters,
    },
    Dungeon {
        name: "Legacy Undead Lair",
        get_count: |s| s.legacy_undead_lair,
    },
    Dungeon {
        name: "Legacy Woodland Labyrinth",
        get_count: |s| s.legacy_woodland_labyrinth,
    },
    Dungeon {
        name: "Lost Halls",
        get_count: |s| s.lost_halls,
    },
    Dungeon {
        name: "Mad Lab",
        get_count: |s| s.mad_lab,
    },
    Dungeon {
        name: "Magic Woods",
        get_count: |s| s.magic_woods,
    },
    Dungeon {
        name: "Malogia",
        get_count: |s| s.malogia,
    },
    Dungeon {
        name: "Manor of the Immortals",
        get_count: |s| s.manor_of_the_immortals,
    },
    Dungeon {
        name: "The Trials of Cronus",
        get_count: |s| s.the_trials_of_cronus,
    },
    Dungeon {
        name: "Moonlight Village",
        get_count: |s| s.moonlight_village,
    },
    Dungeon {
        name: "Mountain Temple",
        get_count: |s| s.mountain_temple,
    },
    Dungeon {
        name: "Neo Forax",
        get_count: |s| s.neo_forax,
    },
    Dungeon {
        name: "Neo Katalund",
        get_count: |s| s.neo_katalund,
    },
    Dungeon {
        name: "Neo Malogia",
        get_count: |s| s.neo_malogia,
    },
    Dungeon {
        name: "Neo Untaris",
        get_count: |s| s.neo_untaris,
    },
    Dungeon {
        name: "Ocean Trench",
        get_count: |s| s.ocean_trench,
    },
    Dungeon {
        name: "Mad God Mayhem",
        get_count: |s| s.mad_god_mayhem,
    },
    Dungeon {
        name: "Oryx's Castle",
        get_count: |s| s.oryxs_castle,
    },
    Dungeon {
        name: "Oryx's Chamber",
        get_count: |s| s.oryxs_chamber,
    },
    Dungeon {
        name: "Oryx's Sanctuary",
        get_count: |s| s.oryxs_sanctuary,
    },
    Dungeon {
        name: "Parasite Chambers",
        get_count: |s| s.parasite_chambers,
    },
    Dungeon {
        name: "Pirate Cave",
        get_count: |s| s.pirate_cave,
    },
    Dungeon {
        name: "Plagued Nest",
        get_count: |s| s.plagued_nest,
    },
    Dungeon {
        name: "Puppet Master's Encore",
        get_count: |s| s.puppet_masters_encore,
    },
    Dungeon {
        name: "Puppet Master's Theatre",
        get_count: |s| s.puppet_masters_theatre,
    },
    Dungeon {
        name: "Queen Bunny Chamber",
        get_count: |s| s.queen_bunny_chamber,
    },
    Dungeon {
        name: "Rainbow Road",
        get_count: |s| s.rainbow_road,
    },
    Dungeon {
        name: "Santa's Workshop",
        get_count: |s| s.santas_workshop,
    },
    Dungeon {
        name: "Secluded Thicket",
        get_count: |s| s.secluded_thicket,
    },
    Dungeon {
        name: "Snake Pit",
        get_count: |s| s.snake_pit,
    },
    Dungeon {
        name: "Spectral Penitentiary",
        get_count: |s| s.spectral_penitentiary,
    },
    Dungeon {
        name: "Spider Den",
        get_count: |s| s.spider_den,
    },
    Dungeon {
        name: "Sprite World",
        get_count: |s| s.sprite_world,
    },
    // Stromwell's Rift completions are not yet exposed in PCStats (Deca only
    // added the counters this season and none of these dungeons were available),
    // so they always report 0. They are shown untracked in the UI until we can
    // map their PCStats bits. See `dungeon_tracked`.
    Dungeon {
        name: "Stromwell's Rift I",
        get_count: |_| 0,
    },
    Dungeon {
        name: "Stromwell's Rift II",
        get_count: |_| 0,
    },
    Dungeon {
        name: "Stromwell's Rift III",
        get_count: |_| 0,
    },
    Dungeon {
        name: "Sulfurous Wetlands",
        get_count: |s| s.sulfurous_wetlands,
    },
    Dungeon {
        name: "The Crawling Depths",
        get_count: |s| s.the_crawling_depths,
    },
    Dungeon {
        name: "The Hive",
        get_count: |s| s.the_hive,
    },
    Dungeon {
        name: "Legacy Lair of Draconis & Ivory",
        get_count: |s| s.ivory_wyvern_portal,
    },
    Dungeon {
        name: "The Machine",
        get_count: |s| s.the_machine,
    },
    Dungeon {
        name: "The Nest",
        get_count: |s| s.the_nest,
    },
    Dungeon {
        name: "The Shatters",
        get_count: |s| s.the_shatters,
    },
    Dungeon {
        name: "The Tavern",
        get_count: |s| s.the_tavern,
    },
    Dungeon {
        name: "The Third Dimension",
        get_count: |s| s.the_third_dimension,
    },
    Dungeon {
        name: "The Void",
        get_count: |s| s.the_void,
    },
    Dungeon {
        name: "Tomb of the Ancients",
        get_count: |s| s.tomb_of_the_ancients,
    },
    Dungeon {
        name: "Toxic Sewers",
        get_count: |s| s.toxic_sewers,
    },
    Dungeon {
        name: "Undead Lair",
        get_count: |s| s.undead_lair,
    },
    Dungeon {
        name: "Untaris",
        get_count: |s| s.untaris,
    },
    Dungeon {
        name: "White Snake Invasion I",
        get_count: |s| s.white_snake_invasion_i,
    },
    Dungeon {
        name: "White Snake Invasion II",
        get_count: |s| s.white_snake_invasion_ii,
    },
    Dungeon {
        name: "White Snake Invasion III",
        get_count: |s| s.white_snake_invasion_iii,
    },
    Dungeon {
        name: "Wine Cellar",
        get_count: |s| s.wine_cellar,
    },
    Dungeon {
        name: "Woodland Labyrinth",
        get_count: |s| s.woodland_labyrinth,
    },
];

/// Get a dungeon list as (name, count) pairs for UI display.
pub fn get_dungeon_list(stats: &DungeonStats) -> Vec<(&'static str, i32)> {
    ALL_DUNGEONS
        .iter()
        .map(|d| (d.name, (d.get_count)(stats)))
        .collect()
}

/// Tooltip shown for dungeons whose completions RealmHound cannot yet read.
pub const UNTRACKED_DUNGEON_TOOLTIP: &str =
    "This dungeon's completions are not tracked yet by RealmHound";

/// Whether RealmHound can read completion counts for this dungeon from PCStats.
///
/// Stromwell's Rift I/II/III are not yet exposed by PCStats, so they always
/// report 0 completions and are flagged untracked in the UI.
pub fn dungeon_tracked(name: &str) -> bool {
    !name.starts_with("Stromwell's Rift")
}

// ============================================================================
// Dungeon references by accessor (compile-time checked)
// ============================================================================

const PIRATE_CAVE: &Dungeon = &Dungeon {
    name: "Pirate Cave",
    get_count: |s| s.pirate_cave,
};
const FORBIDDEN_JUNGLE: &Dungeon = &Dungeon {
    name: "Forbidden Jungle",
    get_count: |s| s.forbidden_jungle,
};
const SPIDER_DEN: &Dungeon = &Dungeon {
    name: "Spider Den",
    get_count: |s| s.spider_den,
};
const SNAKE_PIT: &Dungeon = &Dungeon {
    name: "Snake Pit",
    get_count: |s| s.snake_pit,
};
const UNDEAD_LAIR: &Dungeon = &Dungeon {
    name: "Undead Lair",
    get_count: |s| s.undead_lair,
};
const ABYSS_OF_DEMONS: &Dungeon = &Dungeon {
    name: "Abyss of Demons",
    get_count: |s| s.abyss_of_demons,
};
const MANOR_OF_THE_IMMORTALS: &Dungeon = &Dungeon {
    name: "Manor of the Immortals",
    get_count: |s| s.manor_of_the_immortals,
};
const OCEAN_TRENCH: &Dungeon = &Dungeon {
    name: "Ocean Trench",
    get_count: |s| s.ocean_trench,
};
const TOMB_OF_THE_ANCIENTS: &Dungeon = &Dungeon {
    name: "Tomb of the Ancients",
    get_count: |s| s.tomb_of_the_ancients,
};
const ORYXS_CASTLE: &Dungeon = &Dungeon {
    name: "Oryx's Castle",
    get_count: |s| s.oryxs_castle,
};
const ORYXS_CHAMBER: &Dungeon = &Dungeon {
    name: "Oryx's Chamber",
    get_count: |s| s.oryxs_chamber,
};
const WINE_CELLAR: &Dungeon = &Dungeon {
    name: "Wine Cellar",
    get_count: |s| s.wine_cellar,
};
const DAVY_JONES_LOCKER: &Dungeon = &Dungeon {
    name: "Davy Jones' Locker",
    get_count: |s| s.davy_jones_locker,
};
const MAD_LAB: &Dungeon = &Dungeon {
    name: "Mad Lab",
    get_count: |s| s.mad_lab,
};
const CANDYLAND_HUNTING_GROUNDS: &Dungeon = &Dungeon {
    name: "Candyland Hunting Grounds",
    get_count: |s| s.candyland_hunting_grounds,
};
const HAUNTED_CEMETERY: &Dungeon = &Dungeon {
    name: "Haunted Cemetery",
    get_count: |s| s.haunted_cemetery,
};
const CAVE_OF_A_THOUSAND_TREASURES: &Dungeon = &Dungeon {
    name: "Cave of a Thousand Treasures",
    get_count: |s| s.cave_of_thousand_treasures,
};
const LAIR_OF_DRACONIS: &Dungeon = &Dungeon {
    name: "Lair of Draconis",
    get_count: |s| s.lair_of_draconis,
};
const DEADWATER_DOCKS: &Dungeon = &Dungeon {
    name: "Deadwater Docks",
    get_count: |s| s.deadwater_docks,
};
const WOODLAND_LABYRINTH: &Dungeon = &Dungeon {
    name: "Woodland Labyrinth",
    get_count: |s| s.woodland_labyrinth,
};
const THE_CRAWLING_DEPTHS: &Dungeon = &Dungeon {
    name: "The Crawling Depths",
    get_count: |s| s.the_crawling_depths,
};
const THE_SHATTERS: &Dungeon = &Dungeon {
    name: "The Shatters",
    get_count: |s| s.the_shatters,
};
const LAIR_OF_SHAITAN: &Dungeon = &Dungeon {
    name: "Lair of Shaitan",
    get_count: |s| s.lair_of_shaitan,
};
const PUPPET_MASTERS_THEATRE: &Dungeon = &Dungeon {
    name: "Puppet Master's Theatre",
    get_count: |s| s.puppet_masters_theatre,
};
const ICE_CITADEL: &Dungeon = &Dungeon {
    name: "Ice Citadel",
    get_count: |s| s.ice_citadel,
};
const PUPPET_MASTERS_ENCORE: &Dungeon = &Dungeon {
    name: "Puppet Master's Encore",
    get_count: |s| s.puppet_masters_encore,
};
const THE_HIVE: &Dungeon = &Dungeon {
    name: "The Hive",
    get_count: |s| s.the_hive,
};
const TOXIC_SEWERS: &Dungeon = &Dungeon {
    name: "Toxic Sewers",
    get_count: |s| s.toxic_sewers,
};
const MOUNTAIN_TEMPLE: &Dungeon = &Dungeon {
    name: "Mountain Temple",
    get_count: |s| s.mountain_temple,
};
const THE_THIRD_DIMENSION: &Dungeon = &Dungeon {
    name: "The Third Dimension",
    get_count: |s| s.the_third_dimension,
};
const THE_NEST: &Dungeon = &Dungeon {
    name: "The Nest",
    get_count: |s| s.the_nest,
};
const LOST_HALLS: &Dungeon = &Dungeon {
    name: "Lost Halls",
    get_count: |s| s.lost_halls,
};
const CULTIST_HIDEOUT: &Dungeon = &Dungeon {
    name: "Cultist Hideout",
    get_count: |s| s.cultist_hideout,
};
const THE_VOID: &Dungeon = &Dungeon {
    name: "The Void",
    get_count: |s| s.the_void,
};
const CNIDARIAN_REEF: &Dungeon = &Dungeon {
    name: "Cnidarian Reef",
    get_count: |s| s.cnidarian_reef,
};
const PARASITE_CHAMBERS: &Dungeon = &Dungeon {
    name: "Parasite Chambers",
    get_count: |s| s.parasite_chambers,
};
const MAGIC_WOODS: &Dungeon = &Dungeon {
    name: "Magic Woods",
    get_count: |s| s.magic_woods,
};
const SECLUDED_THICKET: &Dungeon = &Dungeon {
    name: "Secluded Thicket",
    get_count: |s| s.secluded_thicket,
};
const CURSED_LIBRARY: &Dungeon = &Dungeon {
    name: "Cursed Library",
    get_count: |s| s.cursed_library,
};
const ORYXS_SANCTUARY: &Dungeon = &Dungeon {
    name: "Oryx's Sanctuary",
    get_count: |s| s.oryxs_sanctuary,
};
const ANCIENT_RUINS: &Dungeon = &Dungeon {
    name: "Ancient Ruins",
    get_count: |s| s.ancient_ruins,
};
const HIGH_TECH_TERROR: &Dungeon = &Dungeon {
    name: "High Tech Terror",
    get_count: |s| s.high_tech_terror,
};
const SULFUROUS_WETLANDS: &Dungeon = &Dungeon {
    name: "Sulfurous Wetlands",
    get_count: |s| s.sulfurous_wetlands,
};
const SPECTRAL_PENITENTIARY: &Dungeon = &Dungeon {
    name: "Spectral Penitentiary",
    get_count: |s| s.spectral_penitentiary,
};
const FOREST_MAZE: &Dungeon = &Dungeon {
    name: "Forest Maze",
    get_count: |s| s.forest_maze,
};
const SPRITE_WORLD: &Dungeon = &Dungeon {
    name: "Sprite World",
    get_count: |s| s.sprite_world,
};
const FUNGAL_CAVERN: &Dungeon = &Dungeon {
    name: "Fungal Cavern",
    get_count: |s| s.fungal_cavern,
};
const CRYSTAL_CAVERN: &Dungeon = &Dungeon {
    name: "Crystal Cavern",
    get_count: |s| s.crystal_cavern,
};
const KOGBOLD_STEAMWORKS: &Dungeon = &Dungeon {
    name: "Kogbold Steamworks",
    get_count: |s| s.kogbold_steamworks,
};
const BELLADONNAS_GARDEN: &Dungeon = &Dungeon {
    name: "Belladonna's Garden",
    get_count: |s| s.belladonnas_garden,
};
const ICE_TOMB: &Dungeon = &Dungeon {
    name: "Ice Tomb",
    get_count: |s| s.ice_tomb,
};
const MAD_GOD_MAYHEM: &Dungeon = &Dungeon {
    name: "Mad God Mayhem",
    get_count: |s| s.mad_god_mayhem,
};
const BATTLE_FOR_THE_NEXUS: &Dungeon = &Dungeon {
    name: "Battle for the Nexus",
    get_count: |s| s.battle_for_the_nexus,
};
const SANTAS_WORKSHOP: &Dungeon = &Dungeon {
    name: "Santa's Workshop",
    get_count: |s| s.santas_workshop,
};
const THE_MACHINE: &Dungeon = &Dungeon {
    name: "The Machine",
    get_count: |s| s.the_machine,
};
const MALOGIA: &Dungeon = &Dungeon {
    name: "Malogia",
    get_count: |s| s.malogia,
};
const UNTARIS: &Dungeon = &Dungeon {
    name: "Untaris",
    get_count: |s| s.untaris,
};
const FORAX: &Dungeon = &Dungeon {
    name: "Forax",
    get_count: |s| s.forax,
};
const KATALUND: &Dungeon = &Dungeon {
    name: "Katalund",
    get_count: |s| s.katalund,
};
const NEO_FORAX: &Dungeon = &Dungeon {
    name: "Neo Forax",
    get_count: |s| s.neo_forax,
};
const NEO_KATALUND: &Dungeon = &Dungeon {
    name: "Neo Katalund",
    get_count: |s| s.neo_katalund,
};
const NEO_MALOGIA: &Dungeon = &Dungeon {
    name: "Neo Malogia",
    get_count: |s| s.neo_malogia,
};
const NEO_UNTARIS: &Dungeon = &Dungeon {
    name: "Neo Untaris",
    get_count: |s| s.neo_untaris,
};
const RAINBOW_ROAD: &Dungeon = &Dungeon {
    name: "Rainbow Road",
    get_count: |s| s.rainbow_road,
};
const BEACHZONE: &Dungeon = &Dungeon {
    name: "Beachzone",
    get_count: |s| s.beachzone,
};

/// A dungeon collection fame bonus.
#[derive(Debug, Clone)]
pub struct DungeonCollection {
    /// Display name of the collection.
    pub name: &'static str,
    /// List of dungeons required for the collection.
    pub dungeons: &'static [&'static Dungeon],
    /// Fame bonus percentage (e.g., 7.5 for +7.5%).
    pub fame_percent: f32,
    /// Flat fame bonus.
    pub fame_flat: i32,
}

impl DungeonCollection {
    /// Calculate completion for this collection given dungeon stats.
    /// Returns (completed_count, total_count, missing_dungeons).
    pub fn calculate_completion(
        &self,
        stats: &DungeonStats,
    ) -> (usize, usize, Vec<&'static Dungeon>) {
        let mut completed = 0;
        let mut missing = Vec::new();

        for &dungeon in self.dungeons {
            if (dungeon.get_count)(stats) > 0 {
                completed += 1;
            } else {
                missing.push(dungeon);
            }
        }

        (completed, self.dungeons.len(), missing)
    }

    /// Check if the collection is complete.
    pub fn is_complete(&self, stats: &DungeonStats) -> bool {
        self.dungeons
            .iter()
            .all(|&dungeon| (dungeon.get_count)(stats) > 0)
    }
}

/// All dungeon collections in display order (matching RealmEye).
pub static DUNGEON_COLLECTIONS: &[DungeonCollection] = &[
    // Tunnel Rat - Wild Shadow era dungeons
    DungeonCollection {
        name: "Tunnel Rat",
        dungeons: &[
            PIRATE_CAVE,
            FORBIDDEN_JUNGLE,
            SPIDER_DEN,
            SNAKE_PIT,
            UNDEAD_LAIR,
            ABYSS_OF_DEMONS,
            MANOR_OF_THE_IMMORTALS,
            OCEAN_TRENCH,
            TOMB_OF_THE_ANCIENTS,
            ORYXS_CASTLE,
            ORYXS_CHAMBER,
            WINE_CELLAR,
        ],
        fame_percent: 7.5,
        fame_flat: 3000,
    },
    // Explosive Journey - Kabam era dungeons
    DungeonCollection {
        name: "Explosive Journey",
        dungeons: &[
            DAVY_JONES_LOCKER,
            MAD_LAB,
            CANDYLAND_HUNTING_GROUNDS,
            HAUNTED_CEMETERY,
            CAVE_OF_A_THOUSAND_TREASURES,
            LAIR_OF_DRACONIS,
            DEADWATER_DOCKS,
            WOODLAND_LABYRINTH,
            THE_CRAWLING_DEPTHS,
            THE_SHATTERS,
            LAIR_OF_SHAITAN,
            PUPPET_MASTERS_THEATRE,
            ICE_CITADEL,
        ],
        fame_percent: 7.5,
        fame_flat: 3000,
    },
    // Travel of the Decade - Deca era dungeons
    DungeonCollection {
        name: "Travel of the Decade",
        dungeons: &[
            PUPPET_MASTERS_ENCORE,
            THE_HIVE,
            TOXIC_SEWERS,
            MOUNTAIN_TEMPLE,
            THE_THIRD_DIMENSION,
            THE_NEST,
            LOST_HALLS,
            CULTIST_HIDEOUT,
            THE_VOID,
            CNIDARIAN_REEF,
            PARASITE_CHAMBERS,
            MAGIC_WOODS,
            SECLUDED_THICKET,
            CURSED_LIBRARY,
            ORYXS_SANCTUARY,
            ANCIENT_RUINS,
            HIGH_TECH_TERROR,
            SULFUROUS_WETLANDS,
            SPECTRAL_PENITENTIARY,
        ],
        fame_percent: 10.0,
        fame_flat: 5000,
    },
    // First Steps - Early game dungeons
    DungeonCollection {
        name: "First Steps",
        dungeons: &[
            PIRATE_CAVE,
            FOREST_MAZE,
            FORBIDDEN_JUNGLE,
            SPIDER_DEN,
            THE_HIVE,
        ],
        fame_percent: 2.5,
        fame_flat: 100,
    },
    // King of the Mountains - Mountain dungeons
    DungeonCollection {
        name: "King of the Mountains",
        dungeons: &[
            SNAKE_PIT,
            SPRITE_WORLD,
            ABYSS_OF_DEMONS,
            TOXIC_SEWERS,
            MAD_LAB,
            MAGIC_WOODS,
            PUPPET_MASTERS_THEATRE,
            HAUNTED_CEMETERY,
            CURSED_LIBRARY,
            ANCIENT_RUINS,
            SULFUROUS_WETLANDS,
            SPECTRAL_PENITENTIARY,
        ],
        fame_percent: 5.0,
        fame_flat: 1000,
    },
    // Conqueror of the Realm - Encounter dungeons
    DungeonCollection {
        name: "Conqueror of the Realm",
        dungeons: &[
            DAVY_JONES_LOCKER,
            ICE_CITADEL,
            LAIR_OF_DRACONIS,
            MOUNTAIN_TEMPLE,
            THE_THIRD_DIMENSION,
            OCEAN_TRENCH,
            TOMB_OF_THE_ANCIENTS,
            THE_SHATTERS,
            THE_NEST,
            FUNGAL_CAVERN,
            CRYSTAL_CAVERN,
            LOST_HALLS,
            KOGBOLD_STEAMWORKS,
        ],
        fame_percent: 10.0,
        fame_flat: 4000,
    },
    // Enemy of the Court - Court dungeons
    DungeonCollection {
        name: "Enemy of the Court",
        dungeons: &[
            LAIR_OF_SHAITAN,
            PUPPET_MASTERS_ENCORE,
            CNIDARIAN_REEF,
            SECLUDED_THICKET,
            HIGH_TECH_TERROR,
        ],
        fame_percent: 7.5,
        fame_flat: 3000,
    },
    // Epic Battles - Epic dungeons
    DungeonCollection {
        name: "Epic Battles",
        dungeons: &[
            DEADWATER_DOCKS,
            WOODLAND_LABYRINTH,
            THE_CRAWLING_DEPTHS,
            THE_NEST,
            SECLUDED_THICKET,
        ],
        fame_percent: 7.5,
        fame_flat: 2000,
    },
    // Far Out - Alien dungeons
    DungeonCollection {
        name: "Far Out",
        dungeons: &[MALOGIA, UNTARIS, FORAX, KATALUND],
        fame_percent: 5.0,
        fame_flat: 2000,
    },
    // Farther Out - Neo alien wormholes
    DungeonCollection {
        name: "Farther Out",
        dungeons: &[NEO_MALOGIA, NEO_UNTARIS, NEO_FORAX, NEO_KATALUND],
        fame_percent: 7.5,
        fame_flat: 3000,
    },
    // Hero of the Nexus - All standard dungeons
    DungeonCollection {
        name: "Hero of the Nexus",
        dungeons: &[
            PIRATE_CAVE,
            FOREST_MAZE,
            SPIDER_DEN,
            SNAKE_PIT,
            FORBIDDEN_JUNGLE,
            THE_HIVE,
            ANCIENT_RUINS,
            MAGIC_WOODS,
            SPRITE_WORLD,
            CANDYLAND_HUNTING_GROUNDS,
            CAVE_OF_A_THOUSAND_TREASURES,
            UNDEAD_LAIR,
            ABYSS_OF_DEMONS,
            MANOR_OF_THE_IMMORTALS,
            PUPPET_MASTERS_THEATRE,
            TOXIC_SEWERS,
            CURSED_LIBRARY,
            HAUNTED_CEMETERY,
            MAD_LAB,
            PARASITE_CHAMBERS,
            DAVY_JONES_LOCKER,
            MOUNTAIN_TEMPLE,
            THE_THIRD_DIMENSION,
            LAIR_OF_DRACONIS,
            DEADWATER_DOCKS,
            WOODLAND_LABYRINTH,
            THE_CRAWLING_DEPTHS,
            OCEAN_TRENCH,
            ICE_CITADEL,
            TOMB_OF_THE_ANCIENTS,
            FUNGAL_CAVERN,
            CRYSTAL_CAVERN,
            THE_NEST,
            THE_SHATTERS,
            LOST_HALLS,
            CULTIST_HIDEOUT,
            THE_VOID,
            SULFUROUS_WETLANDS,
            KOGBOLD_STEAMWORKS,
            ORYXS_CASTLE,
            LAIR_OF_SHAITAN,
            PUPPET_MASTERS_ENCORE,
            CNIDARIAN_REEF,
            SECLUDED_THICKET,
            HIGH_TECH_TERROR,
            ORYXS_CHAMBER,
            WINE_CELLAR,
            ORYXS_SANCTUARY,
        ],
        fame_percent: 12.5,
        fame_flat: 5000,
    },
    // Season's Beatins - Seasonal dungeons
    DungeonCollection {
        name: "Season's Beatins",
        dungeons: &[
            BELLADONNAS_GARDEN,
            ICE_TOMB,
            MAD_GOD_MAYHEM,
            BATTLE_FOR_THE_NEXUS,
            SANTAS_WORKSHOP,
            THE_MACHINE,
            MALOGIA,
            UNTARIS,
            FORAX,
            KATALUND,
            NEO_MALOGIA,
            NEO_UNTARIS,
            NEO_FORAX,
            NEO_KATALUND,
            RAINBOW_ROAD,
            BEACHZONE,
        ],
        fame_percent: 12.5,
        fame_flat: 5000,
    },
    // Realm of the Mad God - Every dungeon (comprehensive)
    DungeonCollection {
        name: "Realm of the Mad God",
        dungeons: &[
            // All standard dungeons
            PIRATE_CAVE,
            FOREST_MAZE,
            SPIDER_DEN,
            SNAKE_PIT,
            FORBIDDEN_JUNGLE,
            THE_HIVE,
            ANCIENT_RUINS,
            MAGIC_WOODS,
            SPRITE_WORLD,
            CANDYLAND_HUNTING_GROUNDS,
            CAVE_OF_A_THOUSAND_TREASURES,
            UNDEAD_LAIR,
            ABYSS_OF_DEMONS,
            MANOR_OF_THE_IMMORTALS,
            PUPPET_MASTERS_THEATRE,
            TOXIC_SEWERS,
            CURSED_LIBRARY,
            HAUNTED_CEMETERY,
            MAD_LAB,
            PARASITE_CHAMBERS,
            DAVY_JONES_LOCKER,
            MOUNTAIN_TEMPLE,
            THE_THIRD_DIMENSION,
            LAIR_OF_DRACONIS,
            DEADWATER_DOCKS,
            WOODLAND_LABYRINTH,
            THE_CRAWLING_DEPTHS,
            OCEAN_TRENCH,
            ICE_CITADEL,
            TOMB_OF_THE_ANCIENTS,
            FUNGAL_CAVERN,
            CRYSTAL_CAVERN,
            THE_NEST,
            THE_SHATTERS,
            LOST_HALLS,
            CULTIST_HIDEOUT,
            THE_VOID,
            SULFUROUS_WETLANDS,
            KOGBOLD_STEAMWORKS,
            ORYXS_CASTLE,
            LAIR_OF_SHAITAN,
            PUPPET_MASTERS_ENCORE,
            CNIDARIAN_REEF,
            SECLUDED_THICKET,
            HIGH_TECH_TERROR,
            ORYXS_CHAMBER,
            WINE_CELLAR,
            ORYXS_SANCTUARY,
            // Seasonal dungeons
            BELLADONNAS_GARDEN,
            ICE_TOMB,
            MAD_GOD_MAYHEM,
            BATTLE_FOR_THE_NEXUS,
            SANTAS_WORKSHOP,
            THE_MACHINE,
            MALOGIA,
            UNTARIS,
            FORAX,
            KATALUND,
            RAINBOW_ROAD,
            BEACHZONE,
        ],
        fame_percent: 25.0,
        fame_flat: 10000,
    },
];

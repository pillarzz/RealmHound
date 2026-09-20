//! PCStats decoder - decodes the 6-bit encoded character statistics.
//!
//! The PCStats field in the char/list API response contains detailed
//! character statistics encoded in a custom 6-bit format (similar to Base64
//! but with different character mappings).
//!
//! ## Encoding Format
//! 1. First 4 bytes: flag (little-endian i32)
//! 2. Next 16 or 32 bytes: bit array indicating which stats have values
//! 3. Remaining bytes: compressed integers for each set bit, IN THE REFERENCE READ ORDER
//!
//! IMPORTANT: Values are stored in a hardcoded read order, NOT in bit order!
//!
//! NOTE: A 2026 game update grew the bit array from 128 to 256 bits. The current
//! format uses a 32-byte bitfield, but PCStats strings frozen on characters that
//! died before the patch (e.g. graveyard chars that can no longer be re-fetched)
//! still use the legacy 16-byte bitfield. The bitfield size is auto-detected per
//! string so both formats decode correctly. See [`detect_bitfield_bytes`].

use std::collections::HashMap;

/// Regular character statistics (kills, shots, etc.)
#[derive(Debug, Clone, Default)]
pub struct CharacterStats {
    pub shots_fired: i32,
    pub hits: i32,
    pub ability_used: i32,
    pub tiles_discovered: i32,
    pub teleports: i32,
    pub potions_drunk: i32,
    pub kills: i32,
    pub assists: i32,
    pub party_level_ups: i32,
    pub lesser_gods_kills: i32,
    pub encounter_kills: i32,
    pub hero_kills: i32,
    pub critter_kills: i32,
    pub beast_kills: i32,
    pub humanoid_kills: i32,
    pub undead_kills: i32,
    pub nature_kills: i32,
    pub construct_kills: i32,
    pub grotesque_kills: i32,
    pub structure_kills: i32,
    pub god_kills: i32,
    pub assists_against_gods: i32,
    pub cube_kills: i32,
    pub oryx_kills: i32,
    pub quests_completed: i32,
    pub minutes_active: i32,
    pub dungeon_types_completed: i32,
    pub stat_potions_consumed: i32,
    /// Biome enemy kill counts
    pub biomes: BiomeStats,
    /// Dungeon completion counts
    pub dungeons: DungeonStats,
}

/// Biome enemy kill statistics.
#[derive(Debug, Clone, Default)]
pub struct BiomeStats {
    pub ruins_enemy_kills: i32,
    pub beach_enemy_kills: i32,
    pub undead_forest_enemy_kills: i32,
    pub forest_enemy_kills: i32,
    pub plains_enemy_kills: i32,
    pub wither_enemy_kills: i32,
    pub dark_forest_enemy_kills: i32,
    pub desert_enemy_kills: i32,
    pub coral_reefs_enemy_kills: i32,
    pub sprite_forest_enemy_kills: i32,
    pub haunted_hallows_enemy_kills: i32,
    pub shipwreck_cove_enemy_kills: i32,
    pub dead_church_enemy_kills: i32,
    pub risen_hells_enemy_kills: i32,
    pub abandoned_city_enemy_kills: i32,
    pub deep_sea_abyss_enemy_kills: i32,
    pub carboniferous_enemy_kills: i32,
    pub floral_escape_enemy_kills: i32,
    pub sanguine_forest_enemy_kills: i32,
    pub runic_tundra_kills: i32,
}

/// Dungeon completion statistics.
#[derive(Debug, Clone, Default)]
pub struct DungeonStats {
    pub abyss_of_demons: i32,
    pub advanced_kogbold_steamworks: i32,
    pub ancient_ruins: i32,
    pub battle_for_the_nexus: i32,
    pub beachzone: i32,
    pub belladonnas_garden: i32,
    pub bilgewaters_grotto: i32,
    pub candyland_hunting_grounds: i32,
    pub cave_of_thousand_treasures: i32,
    pub cnidarian_reef: i32,
    pub crystal_cavern: i32,
    pub cultist_hideout: i32,
    pub cursed_library: i32,
    pub davy_jones_locker: i32,
    pub deadwater_docks: i32,
    pub forax: i32,
    pub forbidden_jungle: i32,
    pub forest_maze: i32,
    pub fungal_cavern: i32,
    pub haunted_cemetery: i32,
    pub hidden_interregnum: i32,
    pub high_tech_terror: i32,
    pub ice_citadel: i32,
    pub ice_tomb: i32,
    pub infernal_abyss_of_demons: i32,
    pub ivory_wyvern_portal: i32,
    pub katalund: i32,
    pub kogbold_steamworks: i32,
    pub lair_of_draconis: i32,
    pub lair_of_shaitan: i32,
    pub legacy_abyss_of_demons: i32,
    pub legacy_forest_maze: i32,
    pub legacy_heroic_abyss_of_demons: i32,
    pub legacy_heroic_undead_lair: i32,
    pub legacy_lair_of_shaitan: i32,
    pub legacy_pirate_cave: i32,
    pub legacy_spider_den: i32,
    pub legacy_sprite_world: i32,
    pub legacy_the_crawling_depths: i32,
    pub legacy_the_shatters: i32,
    pub legacy_undead_lair: i32,
    pub legacy_woodland_labyrinth: i32,
    pub lost_halls: i32,
    pub mad_lab: i32,
    pub magic_woods: i32,
    pub malogia: i32,
    pub manor_of_the_immortals: i32,
    pub moonlight_village: i32,
    pub mountain_temple: i32,
    pub neo_forax: i32,
    pub neo_katalund: i32,
    pub neo_malogia: i32,
    pub neo_untaris: i32,
    pub ocean_trench: i32,
    pub mad_god_mayhem: i32,
    pub oryxs_castle: i32,
    pub oryxs_chamber: i32,
    pub oryxs_sanctuary: i32,
    pub parasite_chambers: i32,
    pub pirate_cave: i32,
    pub plagued_nest: i32,
    pub puppet_masters_encore: i32,
    pub puppet_masters_theatre: i32,
    pub queen_bunny_chamber: i32,
    pub rainbow_road: i32,
    pub santas_workshop: i32,
    pub secluded_thicket: i32,
    pub snake_pit: i32,
    pub spider_den: i32,
    pub spectral_penitentiary: i32,
    pub sprite_world: i32,
    pub sulfurous_wetlands: i32,
    pub the_crawling_depths: i32,
    pub the_hive: i32,
    pub the_machine: i32,
    pub the_nest: i32,
    pub the_shatters: i32,
    pub the_tavern: i32,
    pub the_third_dimension: i32,
    pub the_trials_of_cronus: i32,
    pub the_void: i32,
    pub tomb_of_the_ancients: i32,
    pub toxic_sewers: i32,
    pub undead_lair: i32,
    pub undead_lair_heroic: i32,
    pub untaris: i32,
    pub white_snake_invasion_i: i32,
    pub white_snake_invasion_ii: i32,
    pub white_snake_invasion_iii: i32,
    pub wine_cellar: i32,
    pub woodland_labyrinth: i32,
}

/// Macro to define PCStats bit mappings in one place.
/// Macro to define PCStats bit mappings with explicit read order.
///
/// Two sections:
/// - `mappings { bit => stat, ... }` - defines which bit maps to which stat (order doesn't matter)
/// - `read_order [ bit1, bit2, ... ]` - defines the exact order values are read from the stream
///
/// This separation makes it clear:
/// 1. What each bit represents (mappings)
/// 2. What order values are read in (read_order)
macro_rules! define_pcstats {
    (
        mappings {
            $( $bit:expr => $($path:ident).+ ),* $(,)?
        }
        read_order [ $( $order_bit:expr ),* $(,)? ]
    ) => {
        /// Bit index to stat name mapping for known stats.
        const BIT_MAPPINGS: &[(usize, &str)] = &[
            $(($bit, stringify!($($path).+)),)*
        ];

        /// The read order - bits are read in THIS order, not numerical order.
        const READ_ORDER: &[usize] = &[$($order_bit,)*];

        /// Parse stat values in the fixed read order, storing into a HashMap.
        fn read_values_in_order(
            reader: &mut ByteReader,
            bits: &[bool],
            debug: bool,
        ) -> Option<HashMap<usize, i32>> {
            // Build a map from bit to stat name for debug output
            let bit_to_name: HashMap<usize, &str> = BIT_MAPPINGS.iter().copied().collect();

            let mut values = HashMap::new();
            let mut idx = 0;

            for &bit in READ_ORDER {
                if bits.get(bit).copied().unwrap_or(false) {
                    let val = reader.read_compressed_int()?;
                    values.insert(bit, val);
                    if debug {
                        let name = bit_to_name.get(&bit).copied().unwrap_or("???");
                        tracing::debug!(
                            "[PCSTATS DEBUG] #{}: bit[{}] {} = {}",
                            idx, bit, name, val
                        );
                    }
                    idx += 1;
                }
            }

            Some(values)
        }

        /// Map the bit values to the CharacterStats struct fields.
        fn map_values_to_stats(values: &HashMap<usize, i32>) -> CharacterStats {
            let mut stats = CharacterStats::default();
            let get = |bit: usize| values.get(&bit).copied().unwrap_or(0);

            $(stats.$($path).+ = get($bit);)*

            stats
        }
    };
}

// ============================================================================
// PCSTATS BIT MAPPINGS AND READ ORDER
//
// mappings: Defines which bit index maps to which stat field (order irrelevant)
// read_order: Defines the exact order values are read (ORDER IS CRITICAL!)
// ============================================================================
define_pcstats! {
    mappings {
        // =====================================================================
        // GENERAL STATS
        // =====================================================================
        24 => shots_fired,
        25 => hits,
        26 => ability_used,
        27 => tiles_discovered,
        28 => teleports,
        29 => potions_drunk,
        30 => kills,
        31 => assists,
        16 => god_kills,
        17 => assists_against_gods,
        18 => cube_kills,
        19 => oryx_kills,
        20 => quests_completed,
        12 => minutes_active,
        36 => dungeon_types_completed,
        85 => party_level_ups,
        86 => lesser_gods_kills,
        87 => encounter_kills,
        72 => hero_kills,
        74 => critter_kills,
        75 => beast_kills,
        76 => humanoid_kills,
        77 => undead_kills,
        78 => nature_kills,
        79 => construct_kills,
        64 => grotesque_kills,
        65 => structure_kills,
        124 => stat_potions_consumed,

        // =====================================================================
        // DUNGEONS (alphabetical for easy lookup)
        // =====================================================================
        23 => dungeons.abyss_of_demons,
        120 => dungeons.advanced_kogbold_steamworks,
        35 => dungeons.ancient_ruins,
        62 => dungeons.battle_for_the_nexus,
        67 => dungeons.beachzone,
        48 => dungeons.belladonnas_garden,
        3 => dungeons.candyland_hunting_grounds,
        5 => dungeons.cave_of_thousand_treasures,
        46 => dungeons.cnidarian_reef,
        34 => dungeons.crystal_cavern,
        40 => dungeons.cultist_hideout,
        32 => dungeons.cursed_library,
        7 => dungeons.davy_jones_locker,
        59 => dungeons.deadwater_docks,
        15 => dungeons.forbidden_jungle,
        1 => dungeons.forest_maze,
        37 => dungeons.forax,
        33 => dungeons.fungal_cavern,
        4 => dungeons.haunted_cemetery,
        68 => dungeons.hidden_interregnum,
        88 => dungeons.high_tech_terror,
        58 => dungeons.ice_citadel,
        89 => dungeons.ice_tomb,
        103 => dungeons.infernal_abyss_of_demons,
        90 => dungeons.katalund,
        70 => dungeons.kogbold_steamworks,
        2 => dungeons.lair_of_draconis,
        43 => dungeons.lair_of_shaitan,
        38 => dungeons.legacy_heroic_abyss_of_demons,
        39 => dungeons.legacy_heroic_undead_lair,
        55 => dungeons.lost_halls,
        91 => dungeons.mad_god_mayhem,
        6 => dungeons.mad_lab,
        45 => dungeons.magic_woods,
        92 => dungeons.malogia,
        0 => dungeons.manor_of_the_immortals,
        71 => dungeons.moonlight_village,
        52 => dungeons.mountain_temple,
        14 => dungeons.ocean_trench,
        93 => dungeons.oryxs_castle,
        94 => dungeons.oryxs_chamber,
        95 => dungeons.oryxs_sanctuary,
        44 => dungeons.parasite_chambers,
        21 => dungeons.pirate_cave,
        121 => dungeons.plagued_nest,
        42 => dungeons.puppet_masters_encore,
        49 => dungeons.puppet_masters_theatre,
        123 => dungeons.queen_bunny_chamber,
        80 => dungeons.rainbow_road,
        81 => dungeons.santas_workshop,
        47 => dungeons.secluded_thicket,
        8 => dungeons.snake_pit,
        9 => dungeons.spider_den,
        125 => dungeons.spectral_penitentiary,
        10 => dungeons.sprite_world,
        69 => dungeons.sulfurous_wetlands,
        122 => dungeons.the_tavern,
        60 => dungeons.the_crawling_depths,
        51 => dungeons.the_hive,
        82 => dungeons.the_machine,
        53 => dungeons.the_nest,
        63 => dungeons.the_shatters,
        66 => dungeons.the_third_dimension,
        101 => dungeons.the_trials_of_cronus,
        41 => dungeons.the_void,
        13 => dungeons.tomb_of_the_ancients,
        50 => dungeons.toxic_sewers,
        22 => dungeons.undead_lair,
        102 => dungeons.undead_lair_heroic,
        83 => dungeons.untaris,
        155 => dungeons.neo_malogia,
        156 => dungeons.neo_untaris,
        157 => dungeons.neo_forax,
        158 => dungeons.neo_katalund,
        136 => dungeons.legacy_pirate_cave,
        137 => dungeons.legacy_abyss_of_demons,
        138 => dungeons.legacy_the_shatters,
        144 => dungeons.legacy_forest_maze,
        145 => dungeons.legacy_sprite_world,
        146 => dungeons.legacy_spider_den,
        147 => dungeons.legacy_undead_lair,
        148 => dungeons.bilgewaters_grotto,
        149 => dungeons.legacy_the_crawling_depths,
        150 => dungeons.legacy_woodland_labyrinth,
        151 => dungeons.ivory_wyvern_portal,
        159 => dungeons.legacy_lair_of_shaitan,
        98 => dungeons.white_snake_invasion_i,
        99 => dungeons.white_snake_invasion_ii,
        100 => dungeons.white_snake_invasion_iii,
        84 => dungeons.wine_cellar,
        61 => dungeons.woodland_labyrinth,

        // =====================================================================
        // BIOME ENEMY KILLS
        // =====================================================================
        96 => biomes.sanguine_forest_enemy_kills,
        97 => biomes.runic_tundra_kills,
        104 => biomes.haunted_hallows_enemy_kills,
        105 => biomes.shipwreck_cove_enemy_kills,
        106 => biomes.dead_church_enemy_kills,
        107 => biomes.risen_hells_enemy_kills,
        108 => biomes.abandoned_city_enemy_kills,
        109 => biomes.deep_sea_abyss_enemy_kills,
        110 => biomes.carboniferous_enemy_kills,
        111 => biomes.floral_escape_enemy_kills,
        112 => biomes.undead_forest_enemy_kills,
        113 => biomes.forest_enemy_kills,
        114 => biomes.plains_enemy_kills,
        115 => biomes.wither_enemy_kills,
        116 => biomes.dark_forest_enemy_kills,
        117 => biomes.desert_enemy_kills,
        118 => biomes.coral_reefs_enemy_kills,
        119 => biomes.sprite_forest_enemy_kills,
        126 => biomes.ruins_enemy_kills,
        127 => biomes.beach_enemy_kills
    }

    // =========================================================================
    // READ ORDER - This is the exact order values are read from the stream
    // DO NOT CHANGE unless you've verified the correct order!
    // =========================================================================
    read_order [
        // General stats
        24, 25, 26, 27, 28, 29, 30, 31,
        16, 17, 18, 19, 20,

        // Dungeons batch 1
        21, 22, 23,
        8, 9, 10,
        12,  // minutes_active
        13, 14, 15,
        0, 1, 2, 3, 4, 5, 6, 7,

        // Dungeons batch 2
        58, 59, 60, 61, 62, 63,
        48, 49, 50, 51, 52, 53,
        55,
        40, 41, 42, 43, 44, 45, 46, 47,
        32, 33, 34, 35,
        36,  // dungeon_types_completed
        37, 38, 39,

        // Dungeons batch 3
        88, 89, 90, 91, 92, 93, 94, 95,
        80, 81, 82, 83, 84,
        85, 86, 87,  // party_level_ups, lesser_gods_kills, encounter_kills
        72,
        74, 75, 76, 77, 78, 79,
        64, 65,
        66, 67, 68, 69, 70, 71,

        // Dungeons batch 4
        120, 121, 122, 123,
        124,  // stat_potions_consumed
        125,

        // Biome enemy kills (fixed read order)
        126, 127,  // ruins, beach
        112, 113, 114, 115, 116, 117, 118, 119,  // undead_forest through sprite_forest
        104, 105, 106, 107, 108, 109, 110, 111,  // haunted_hallows through floral_escape
        96, 97,  // sanguine_forest, runic_tundra

        // Season/event dungeons + new heroics (read last)
        98, 99, 100, 101,  // white_snake I/II/III, trials_of_cronus
        102, 103,  // undead_lair_heroic, infernal_abyss_of_demons

        // Neo alien wormholes
        155, 156, 157, 158,

        // Legacy/removed dungeons + Bilgewater's Grotto + Ivory Wyvern Portal.
        // Values are written in the game's enum/add order, NOT ascending bit
        // order: Legacy The Shatters (bit 138) was added after its low bit slot,
        // so its value is appended last. Reading 138 in numeric position would
        // shift every later legacy value by one, so it must be read last.
        136, 137, 144, 145, 146, 147, 148, 149, 150, 151, 159, 138,
    ]
}

/// Decode a PCStats string into character statistics.
///
/// The PCStats string uses a custom 6-bit encoding similar to Base64.
pub fn decode_pcstats(pc_stats: &str) -> Option<CharacterStats> {
    decode_pcstats_internal(pc_stats, false)
}

/// Decode PCStats with debug output to help reverse-engineer unknown stats.
pub fn decode_pcstats_debug(pc_stats: &str) -> Option<CharacterStats> {
    decode_pcstats_internal(pc_stats, true)
}

/// Get unmapped bit indices with their values from a PCStats string.
/// Returns a vector of (bit_index, value) tuples for any bits that are set
/// but not yet mapped to a known stat.
pub fn get_unmapped_pcstats(pc_stats: &str) -> Vec<(usize, i32)> {
    if pc_stats.is_empty() {
        return Vec::new();
    }

    let Some(bytes) = six_bit_string_to_bytes(pc_stats) else {
        return Vec::new();
    };
    if bytes.len() < 4 {
        return Vec::new();
    }

    let flag = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if flag == 0 {
        return Vec::new();
    }

    let mut reader = ByteReader::new(&bytes[4..]);
    let Some(bit_array) = parse_bits(&mut reader, detect_bitfield_bytes(&bytes[4..])) else {
        return Vec::new();
    };

    // Read known values first (in order)
    let _ = read_values_in_order(&mut reader, &bit_array, false);

    // Find unknown bits
    let known_bits: std::collections::HashSet<usize> = READ_ORDER.iter().copied().collect();

    let unknown_bits: Vec<usize> = bit_array
        .iter()
        .enumerate()
        .filter(|(i, &b)| b && !known_bits.contains(i))
        .map(|(i, _)| i)
        .collect();

    // Read remaining unknown values
    let mut result = Vec::new();
    for bit_idx in unknown_bits {
        if let Some(val) = reader.read_compressed_int() {
            result.push((bit_idx, val));
        }
    }

    result
}

/// Debug helper: Show which bits are set and correlate with our read order.
/// Returns a formatted string with detailed comparison to help identify misalignment.
pub fn debug_order_alignment(pc_stats: &str) -> String {
    use std::fmt::Write;
    let mut output = String::new();

    if pc_stats.is_empty() {
        return "Empty PCStats string".to_string();
    }

    let Some(bytes) = six_bit_string_to_bytes(pc_stats) else {
        return "Failed to decode PCStats string".to_string();
    };
    if bytes.len() < 4 {
        return "PCStats too short".to_string();
    }

    let flag = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if flag == 0 {
        return "Flag is 0, no stats".to_string();
    }

    let mut reader = ByteReader::new(&bytes[4..]);
    let Some(bit_array) = parse_bits(&mut reader, detect_bitfield_bytes(&bytes[4..])) else {
        return "Failed to parse bit array".to_string();
    };

    // Get all set bits
    let set_bits: Vec<usize> = bit_array
        .iter()
        .enumerate()
        .filter(|(_, &b)| b)
        .map(|(i, _)| i)
        .collect();

    let _ = writeln!(output, "PCSTATS DEBUG - {} bits set", set_bits.len());
    let _ = writeln!(output, "Set bits: {:?}", set_bits);

    // Show our read order for the set bits
    let known_bits: std::collections::HashSet<usize> = READ_ORDER.iter().copied().collect();

    // Find unknown bits
    let unknown: Vec<usize> = set_bits
        .iter()
        .filter(|b| !known_bits.contains(b))
        .copied()
        .collect();

    if !unknown.is_empty() {
        let _ = writeln!(output, "\n⚠️ UNKNOWN BITS: {:?}", unknown);
    }

    let _ = writeln!(output, "\nIdx | Bit | Value | Stat");
    let _ = writeln!(output, "----|-----|-------|-----");

    let mut idx = 0;
    for &bit in READ_ORDER {
        if bit_array.get(bit).copied().unwrap_or(false) {
            if let Some(val) = reader.read_compressed_int() {
                let name = BIT_MAPPINGS
                    .iter()
                    .find(|(b, _)| *b == bit)
                    .map(|(_, n)| *n)
                    .unwrap_or("???");
                let _ = writeln!(output, "{:>3} | {:>3} | {:>5} | {}", idx, bit, val, name);
                idx += 1;
            }
        }
    }

    // Read any remaining unknown values
    for &bit in &unknown {
        if let Some(val) = reader.read_compressed_int() {
            let _ = writeln!(output, "{:>3} | {:>3} | {:>5} | ⚠️ UNKNOWN", idx, bit, val);
            idx += 1;
        }
    }

    if reader.pos < reader.data.len() {
        let _ = writeln!(
            output,
            "\n⚠️ {} bytes remain unread!",
            reader.data.len() - reader.pos
        );
    }

    output
}

fn decode_pcstats_internal(pc_stats: &str, debug: bool) -> Option<CharacterStats> {
    if pc_stats.is_empty() {
        return Some(CharacterStats::default());
    }

    let bytes = six_bit_string_to_bytes(pc_stats)?;
    if bytes.len() < 4 {
        return Some(CharacterStats::default());
    }

    // Read flag (first 4 bytes, little-endian)
    let flag = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if flag == 0 {
        return Some(CharacterStats::default());
    }

    if debug {
        tracing::info!("[PCSTATS DEBUG] Flag: {}", flag);
        tracing::info!(
            "[PCSTATS DEBUG] Total bytes after flag: {}",
            bytes.len() - 4
        );
    }

    let mut reader = ByteReader::new(&bytes[4..]);

    // Parse the bit flags indicating which stats are present
    let bit_array = parse_bits(&mut reader, detect_bitfield_bytes(&bytes[4..]))?;

    if debug {
        let set_bits: Vec<usize> = bit_array
            .iter()
            .enumerate()
            .filter(|(_, &b)| b)
            .map(|(i, _)| i)
            .collect();
        tracing::info!(
            "[PCSTATS DEBUG] Bits set ({} total): {:?}",
            set_bits.len(),
            set_bits
        );
    }

    // Read values in the fixed order (generated by macro)
    let values = read_values_in_order(&mut reader, &bit_array, debug)?;

    if debug {
        // Check for unknown bits
        let known_bits: std::collections::HashSet<usize> = READ_ORDER.iter().copied().collect();

        let unknown_bits: Vec<usize> = bit_array
            .iter()
            .enumerate()
            .filter(|(i, &b)| b && !known_bits.contains(i))
            .map(|(i, _)| i)
            .collect();

        if !unknown_bits.is_empty() {
            tracing::warn!("[PCSTATS DEBUG] ⚠️ UNKNOWN BITS SET: {:?}", unknown_bits);
            // Try to read their values
            for &bit_idx in &unknown_bits {
                if let Some(val) = reader.read_compressed_int() {
                    tracing::warn!(
                        "[PCSTATS DEBUG]   bit[{}] = {} (UNKNOWN - needs mapping!)",
                        bit_idx,
                        val
                    );
                }
            }
        }

        if reader.pos < reader.data.len() {
            tracing::warn!(
                "[PCSTATS DEBUG] ⚠️ {} bytes of data still remain unread!",
                reader.data.len() - reader.pos
            );
        } else {
            tracing::info!("[PCSTATS DEBUG] ✓ All data consumed successfully");
        }
    }

    // Map values to struct fields (generated by macro)
    let stats = map_values_to_stats(&values);

    Some(stats)
}

/// Convert a 6-bit encoded string to bytes.
fn six_bit_string_to_bytes(s: &str) -> Option<Vec<u8>> {
    let chars: Vec<char> = s.chars().collect();
    let index_padding = s.find('=').unwrap_or(s.len());
    let padding = s.len() - index_padding;

    if s.len() % 4 != 0 {
        return None;
    }

    let output_len = (s.len() / 4) * 3 - padding;
    let mut output = vec![0u8; output_len];
    let mut o = 0;

    for i in (0..s.len()).step_by(4) {
        let v1 = char_value(chars[i])?;
        let v2 = char_value(chars[i + 1])?;
        let c3 = chars[i + 2];
        let c4 = chars[i + 3];
        let v3 = char_value(c3)?;
        let v4 = char_value(c4)?;

        output[o] = ((v1 << 2) | (v2 >> 4)) as u8;
        if c3 != '=' && o + 1 < output_len {
            output[o + 1] = (((v2 & 0x0F) << 4) | (v3 >> 2)) as u8;
            if c4 != '=' && o + 2 < output_len {
                output[o + 2] = (((v3 & 0x03) << 6) | v4) as u8;
            }
        }
        o += 3;
    }

    Some(output)
}

/// Get the numeric value of a 6-bit encoded character.
fn char_value(c: char) -> Option<u8> {
    match c {
        'A'..='Z' => Some(c as u8 - b'A'),      // 0-25
        'a'..='z' => Some(c as u8 - b'a' + 26), // 26-51
        '0'..='9' => Some(c as u8 - b'0' + 52), // 52-61
        '-' => Some(62),
        '_' => Some(63),
        '=' => Some(0), // padding
        _ => None,
    }
}

/// Simple byte reader for parsing PCStats data.
struct ByteReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> ByteReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn read_byte(&mut self) -> Option<u8> {
        if self.pos < self.data.len() {
            let b = self.data[self.pos];
            self.pos += 1;
            Some(b)
        } else {
            None
        }
    }

    /// Read a compressed integer (variable-length encoding).
    ///
    /// An i32 varint is at most 5 bytes; the continuation loop is bounded and
    /// uses `wrapping_shl` so that misaligned/garbage input (which can occur
    /// while probing the wrong bitfield size during format detection) cannot
    /// panic via shift overflow. Valid input never reaches those bounds, so
    /// decoded values are unchanged.
    fn read_compressed_int(&mut self) -> Option<i32> {
        let first_byte = self.read_byte()? as u32;
        let is_negative = (first_byte & 64) != 0;
        let mut shift: u32 = 6;
        let mut value = (first_byte & 63) as i32;

        let mut current_byte = first_byte;
        let mut bytes_read = 1;
        while (current_byte & 128) != 0 && bytes_read < 5 {
            current_byte = self.read_byte()? as u32;
            value |= ((current_byte & 127) as i32).wrapping_shl(shift);
            shift += 7;
            bytes_read += 1;
        }

        if is_negative {
            Some(-value)
        } else {
            Some(value)
        }
    }
}

/// Number of bytes in the current PCStats presence bitfield.
///
/// A 2026 game update expanded the stat bitfield from 16 bytes (128 bits) to
/// 32 bytes (256 bits) to make room for new stats (e.g. "Summon Power").
const PCSTATS_BITFIELD_BYTES: usize = 32;

/// Number of bytes in the legacy PCStats presence bitfield (pre-2026 patch).
///
/// PCStats strings frozen on characters that died before the patch still use
/// this size and cannot be re-fetched, so the decoder must support both.
const LEGACY_PCSTATS_BITFIELD_BYTES: usize = 16;

/// Detect whether a PCStats payload uses the legacy 16-byte bitfield or the
/// current 32-byte bitfield.
///
/// `payload` is the bytes *after* the 4-byte flag. Older (pre-patch) strings,
/// such as those frozen on dead/graveyard characters, use 16 bytes; current
/// strings use 32. Reading the wrong size desyncs the value stream and corrupts
/// every stat, so we probe both sizes and pick the one whose value stream parses
/// cleanly.
///
/// Each candidate size is scored by `(leftover_bytes, unknown_bits)`, lower is
/// better:
/// - `leftover_bytes`: bytes left unread after consuming every known value
///   (or [`usize::MAX`] if the stream ran out mid-read). The correct size
///   consumes the stream exactly. This is the primary signal so that a future
///   stat mapped above bit 127 does not flip a valid 32-byte string to legacy.
/// - `unknown_bits`: count of set bits not in `READ_ORDER` (secondary signal).
///
/// Ties prefer the current 32-byte format for forward-compatibility.
fn detect_bitfield_bytes(payload: &[u8]) -> usize {
    let score = |n: usize| -> Option<(usize, usize)> {
        if payload.len() < n {
            return None;
        }
        let mut reader = ByteReader::new(payload);
        let bits = parse_bits(&mut reader, n)?;
        let read_ok = read_values_in_order(&mut reader, &bits, false).is_some();
        let leftover = if read_ok {
            reader.data.len() - reader.pos
        } else {
            usize::MAX
        };
        let known: std::collections::HashSet<usize> = READ_ORDER.iter().copied().collect();
        let unknown = bits
            .iter()
            .enumerate()
            .filter(|(i, &b)| b && !known.contains(i))
            .count();
        Some((leftover, unknown))
    };

    match (
        score(PCSTATS_BITFIELD_BYTES),
        score(LEGACY_PCSTATS_BITFIELD_BYTES),
    ) {
        (Some(current), Some(legacy)) => {
            if legacy < current {
                LEGACY_PCSTATS_BITFIELD_BYTES
            } else {
                PCSTATS_BITFIELD_BYTES
            }
        }
        (Some(_), None) => PCSTATS_BITFIELD_BYTES,
        (None, Some(_)) => LEGACY_PCSTATS_BITFIELD_BYTES,
        (None, None) => PCSTATS_BITFIELD_BYTES,
    }
}

/// Parse the bit flags from the PCStats data.
/// Reads exactly `bitfield_bytes` bytes to create the presence bit array.
fn parse_bits(reader: &mut ByteReader, bitfield_bytes: usize) -> Option<Vec<bool>> {
    let mut bits = vec![false; bitfield_bytes * 8];

    for i in 0..bitfield_bytes {
        let byte = reader.read_byte()?;
        let j = i * 8;
        bits[j] = (byte & 0x01) != 0;
        bits[j + 1] = (byte & 0x02) != 0;
        bits[j + 2] = (byte & 0x04) != 0;
        bits[j + 3] = (byte & 0x08) != 0;
        bits[j + 4] = (byte & 0x10) != 0;
        bits[j + 5] = (byte & 0x20) != 0;
        bits[j + 6] = (byte & 0x40) != 0;
        bits[j + 7] = (byte & 0x80) != 0;
    }

    Some(bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_char_value() {
        assert_eq!(char_value('A'), Some(0));
        assert_eq!(char_value('Z'), Some(25));
        assert_eq!(char_value('a'), Some(26));
        assert_eq!(char_value('z'), Some(51));
        assert_eq!(char_value('0'), Some(52));
        assert_eq!(char_value('9'), Some(61));
        assert_eq!(char_value('-'), Some(62));
        assert_eq!(char_value('_'), Some(63));
        assert_eq!(char_value('='), Some(0));
    }

    #[test]
    fn test_six_bit_decode_simple() {
        let result = six_bit_string_to_bytes("AAAA");
        assert!(result.is_some());
        let bytes = result.unwrap();
        assert_eq!(bytes.len(), 3);
        assert_eq!(bytes, vec![0, 0, 0]);
    }

    #[test]
    fn test_decode_empty_pcstats() {
        let stats = decode_pcstats("");
        assert!(stats.is_some());
        let stats = stats.unwrap();
        assert_eq!(stats.kills, 0);
    }

    #[test]
    fn test_compressed_int_reader() {
        // Test positive number
        let data = vec![0x05]; // 5 (no continuation, positive)
        let mut reader = ByteReader::new(&data);
        assert_eq!(reader.read_compressed_int(), Some(5));

        // Test negative number
        let data = vec![0x45]; // -5 (no continuation, negative bit set)
        let mut reader = ByteReader::new(&data);
        assert_eq!(reader.read_compressed_int(), Some(-5));
    }

    #[test]
    fn test_bit_mappings_unique() {
        // Verify all mappings are unique
        let unique: std::collections::HashSet<usize> = READ_ORDER.iter().copied().collect();
        assert_eq!(
            READ_ORDER.len(),
            unique.len(),
            "Duplicate bit mappings found!"
        );
    }

    #[test]
    fn test_decode_256_bit_bitfield() {
        // Regression for the 2026 game update that grew the stat bitfield from
        // 128 to 256 bits. This blob uses a 32-byte bitfield with
        // bit0=manor(300), bit24=shots_fired(1000), bit30=kills(50). Decoding it
        // with a 16-byte bitfield desyncs the stream and corrupts every value.
        let s = "AQAAAAEAAEEAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAqA8yrAQ=";
        let stats = decode_pcstats(s).expect("should decode");
        assert_eq!(stats.dungeons.manor_of_the_immortals, 300);
        assert_eq!(stats.shots_fired, 1000);
        assert_eq!(stats.kills, 50);
    }

    /// Encode bytes using the URL-safe 6-bit alphabet (inverse of
    /// `six_bit_string_to_bytes`) so tests can build raw PCStats blobs.
    fn encode_six_bit(bytes: &[u8]) -> String {
        const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = *chunk.get(1).unwrap_or(&0) as u32;
            let b2 = *chunk.get(2).unwrap_or(&0) as u32;
            let n = (b0 << 16) | (b1 << 8) | b2;
            out.push(ALPHA[((n >> 18) & 63) as usize] as char);
            out.push(ALPHA[((n >> 12) & 63) as usize] as char);
            out.push(if chunk.len() > 1 {
                ALPHA[((n >> 6) & 63) as usize] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                ALPHA[(n & 63) as usize] as char
            } else {
                '='
            });
        }
        out
    }

    /// Encode a value as a 2-byte compressed int (valid for 64..=4095).
    fn compressed_2byte(v: u32) -> [u8; 2] {
        [((v & 63) | 0x80) as u8, ((v >> 6) & 0x7F) as u8]
    }

    #[test]
    fn test_decode_legacy_16_byte_bitfield() {
        // Regression: characters that died before the 2026 patch
        // keep a 16-byte (128-bit) bitfield frozen in the API and can no longer
        // be re-fetched (e.g. graveyard chars). The decoder must auto-detect the
        // legacy size instead of forcing the 32-byte format, which corrupts
        // every stat. flag=1; bits manor(0), shots_fired(24), kills(30);
        // values (READ_ORDER): shots=1000, kills=50, manor=300.
        let mut bytes = vec![1u8, 0, 0, 0];
        let mut bitfield = vec![0u8; 16];
        bitfield[0] = 0x01; // bit 0 = manor
        bitfield[3] = 0x41; // bit 24 = shots_fired, bit 30 = kills
        bytes.extend_from_slice(&bitfield);
        bytes.extend_from_slice(&[0xA8, 0x0F, 0x32, 0xAC, 0x04]);

        let s = encode_six_bit(&bytes);
        let stats = decode_pcstats(&s).expect("should decode legacy 16-byte format");
        assert_eq!(stats.dungeons.manor_of_the_immortals, 300);
        assert_eq!(stats.shots_fired, 1000);
        assert_eq!(stats.kills, 50);
    }

    #[test]
    fn test_detect_prefers_legacy_when_32_byte_read_desyncs() {
        // A legacy blob long enough that a 32-byte bitfield read is also
        // length-valid, so detection must rely on stream consumption (not just
        // length) to choose 16. 10 general stats with 2-byte values => 20 value
        // bytes, giving a 36-byte payload. bits 24..=31 and 16,17 are set.
        let mut bytes = vec![1u8, 0, 0, 0];
        let mut bitfield = vec![0u8; 16];
        bitfield[2] = 0x03; // bits 16, 17
        bitfield[3] = 0xFF; // bits 24..=31
        bytes.extend_from_slice(&bitfield);
        // READ_ORDER for these bits: 24,25,26,27,28,29,30,31,16,17.
        for v in [301u32, 302, 303, 304, 305, 306, 307, 308, 309, 310] {
            bytes.extend_from_slice(&compressed_2byte(v));
        }
        assert!(bytes.len() - 4 >= PCSTATS_BITFIELD_BYTES);

        let s = encode_six_bit(&bytes);
        let stats = decode_pcstats(&s).expect("should decode as legacy");
        assert_eq!(stats.shots_fired, 301); // bit 24, first in read order
        assert_eq!(stats.assists, 308); // bit 31
        assert_eq!(stats.god_kills, 309); // bit 16
        assert_eq!(stats.assists_against_gods, 310); // bit 17, last
    }

    #[test]
    fn test_detect_prefers_current_32_byte_format() {
        // The existing 32-byte blob must still resolve to the current format
        // even though a 16-byte read is length-valid for it.
        let s = "AQAAAAEAAEEAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAqA8yrAQ=";
        let bytes = six_bit_string_to_bytes(s).unwrap();
        assert_eq!(detect_bitfield_bytes(&bytes[4..]), PCSTATS_BITFIELD_BYTES);
    }
}

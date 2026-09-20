//! RotMG character data structures.

use std::collections::HashMap;

/// RotMG character class types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum CharacterClass {
    Rogue = 768,
    Archer = 775,
    Wizard = 782,
    Priest = 784,
    Warrior = 797,
    Knight = 798,
    Paladin = 799,
    Assassin = 800,
    Necromancer = 801,
    Huntress = 802,
    Mystic = 803,
    Trickster = 804,
    Sorcerer = 805,
    Ninja = 806,
    Samurai = 785,
    Bard = 796,
    Summoner = 817,
    Kensei = 818,
    Druid = 819,
    Unknown = 0,
}

impl CharacterClass {
    /// Get the class from its numeric ID.
    pub fn from_id(id: u16) -> Self {
        match id {
            768 => Self::Rogue,
            775 => Self::Archer,
            782 => Self::Wizard,
            784 => Self::Priest,
            797 => Self::Warrior,
            798 => Self::Knight,
            799 => Self::Paladin,
            800 => Self::Assassin,
            801 => Self::Necromancer,
            802 => Self::Huntress,
            803 => Self::Mystic,
            804 => Self::Trickster,
            805 => Self::Sorcerer,
            806 => Self::Ninja,
            785 => Self::Samurai,
            796 => Self::Bard,
            817 => Self::Summoner,
            818 => Self::Kensei,
            819 => Self::Druid,
            _ => Self::Unknown,
        }
    }

    /// Get the display name of the class.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Rogue => "Rogue",
            Self::Archer => "Archer",
            Self::Wizard => "Wizard",
            Self::Priest => "Priest",
            Self::Warrior => "Warrior",
            Self::Knight => "Knight",
            Self::Paladin => "Paladin",
            Self::Assassin => "Assassin",
            Self::Necromancer => "Necromancer",
            Self::Huntress => "Huntress",
            Self::Mystic => "Mystic",
            Self::Trickster => "Trickster",
            Self::Sorcerer => "Sorcerer",
            Self::Ninja => "Ninja",
            Self::Samurai => "Samurai",
            Self::Bard => "Bard",
            Self::Summoner => "Summoner",
            Self::Kensei => "Kensei",
            Self::Druid => "Druid",
            Self::Unknown => "Unknown",
        }
    }
}

/// Exaltation progress for a single class.
/// Raw values represent dungeon completion counts toward exaltations.
/// Use `exalt_level()` to convert raw values to 0-5 exalt levels.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClassExaltation {
    /// Class type ID (e.g., 768 for Rogue)
    pub class_type: i32,
    /// Raw attack exaltation progress
    pub attack: u32,
    /// Raw defense exaltation progress
    pub defense: u32,
    /// Raw speed exaltation progress
    pub speed: u32,
    /// Raw dexterity exaltation progress
    pub dexterity: u32,
    /// Raw vitality exaltation progress
    pub vitality: u32,
    /// Raw wisdom exaltation progress
    pub wisdom: u32,
    /// Raw HP exaltation progress
    pub hp: u32,
    /// Raw MP exaltation progress
    pub mp: u32,
}

impl ClassExaltation {
    /// Convert a raw exaltation value to exalt level (0-5).
    /// Thresholds: 0-4=0, 5-14=1, 15-29=2, 30-49=3, 50-74=4, 75+=5
    pub fn raw_to_exalt_level(raw: u32) -> u8 {
        match raw {
            0..=4 => 0,
            5..=14 => 1,
            15..=29 => 2,
            30..=49 => 3,
            50..=74 => 4,
            _ => 5,
        }
    }

    /// Get attack exalt level (0-5).
    pub fn attack_level(&self) -> u8 {
        Self::raw_to_exalt_level(self.attack)
    }

    /// Get defense exalt level (0-5).
    pub fn defense_level(&self) -> u8 {
        Self::raw_to_exalt_level(self.defense)
    }

    /// Get speed exalt level (0-5).
    pub fn speed_level(&self) -> u8 {
        Self::raw_to_exalt_level(self.speed)
    }

    /// Get dexterity exalt level (0-5).
    pub fn dexterity_level(&self) -> u8 {
        Self::raw_to_exalt_level(self.dexterity)
    }

    /// Get vitality exalt level (0-5).
    pub fn vitality_level(&self) -> u8 {
        Self::raw_to_exalt_level(self.vitality)
    }

    /// Get wisdom exalt level (0-5).
    pub fn wisdom_level(&self) -> u8 {
        Self::raw_to_exalt_level(self.wisdom)
    }

    /// Get HP exalt level (0-5).
    pub fn hp_level(&self) -> u8 {
        Self::raw_to_exalt_level(self.hp)
    }

    /// Get MP exalt level (0-5).
    pub fn mp_level(&self) -> u8 {
        Self::raw_to_exalt_level(self.mp)
    }

    /// Get total exalt levels across all stats (0-40 max).
    pub fn total_levels(&self) -> u8 {
        self.attack_level()
            + self.defense_level()
            + self.speed_level()
            + self.dexterity_level()
            + self.vitality_level()
            + self.wisdom_level()
            + self.hp_level()
            + self.mp_level()
    }

    /// Effective stat bonus granted by exaltations: +1 per level for the six
    /// primary stats and +5 per level for HP and MP. Returned as
    /// `(hp, mp, attack, defense, speed, dexterity, vitality, wisdom)`.
    pub fn stat_bonuses(&self) -> (i32, i32, i32, i32, i32, i32, i32, i32) {
        (
            self.hp_level() as i32 * 5,
            self.mp_level() as i32 * 5,
            self.attack_level() as i32,
            self.defense_level() as i32,
            self.speed_level() as i32,
            self.dexterity_level() as i32,
            self.vitality_level() as i32,
            self.wisdom_level() as i32,
        )
    }

    /// Parse from comma-separated string "DEX,SPD,VIT,WIS,DEF,ATT,MP,HP".
    pub fn from_csv(class_type: i32, csv: &str) -> Option<Self> {
        let parts: Vec<&str> = csv.split(',').collect();
        if parts.len() != 8 {
            return None;
        }
        Some(Self {
            class_type,
            dexterity: parts[0].parse().unwrap_or(0),
            speed: parts[1].parse().unwrap_or(0),
            vitality: parts[2].parse().unwrap_or(0),
            wisdom: parts[3].parse().unwrap_or(0),
            defense: parts[4].parse().unwrap_or(0),
            attack: parts[5].parse().unwrap_or(0),
            mp: parts[6].parse().unwrap_or(0),
            hp: parts[7].parse().unwrap_or(0),
        })
    }

    /// Create from ExaltationUpdatePacket fields.
    /// Packet order: DEX, SPD, VIT, WIS, DEF, ATT, MP, HP (same as CSV).
    pub fn from_packet(
        class_type: i16,
        dexterity: i32,
        speed: i32,
        vitality: i32,
        wisdom: i32,
        defense: i32,
        attack: i32,
        mana: i32,
        health: i32,
    ) -> Self {
        Self {
            class_type: class_type as i32,
            dexterity: dexterity as u32,
            speed: speed as u32,
            vitality: vitality as u32,
            wisdom: wisdom as u32,
            defense: defense as u32,
            attack: attack as u32,
            mp: mana as u32,
            hp: health as u32,
        }
    }
}

/// A pet ability with type and power level.
#[derive(Debug, Clone, Default)]
pub struct PetAbility {
    /// Ability type ID (406=Electric, 407=Heal, 408=MagicHeal, etc.)
    pub ability_type: i32,
    /// Power/level (1-100)
    pub power: i32,
}

impl PetAbility {
    /// Get the display name for this ability type.
    pub fn name(&self) -> &'static str {
        pet_ability_name(self.ability_type)
    }
}

/// Get the display name for a pet ability type.
pub fn pet_ability_name(ability_type: i32) -> &'static str {
    match ability_type {
        402 => "Atk Close",
        404 => "Atk Mid",
        405 => "Atk Far",
        406 => "Electric",
        407 => "Heal",
        408 => "M.Heal",
        409 => "Savage",
        410 => "Decoy",
        411 => "Rising Fury",
        _ => "Unknown",
    }
}

/// An item in pet inventory.
#[derive(Debug, Clone, Default)]
pub struct PetInventoryItem {
    /// Item type ID (-1 = empty slot)
    pub item_id: i32,
    /// Unique ID for enchanted items (from item_id#unique_id format)
    pub unique_id: Option<u64>,
    /// Stack count for stackable items (1 = single, >1 = stacked)
    pub stack_count: u8,
}

impl PetInventoryItem {
    /// Create an empty slot.
    pub fn empty() -> Self {
        Self {
            item_id: -1,
            unique_id: None,
            stack_count: 0,
        }
    }

    /// Check if this slot is empty.
    pub fn is_empty(&self) -> bool {
        self.item_id < 0
    }
}

/// Pet data for a character.
#[derive(Debug, Clone, Default)]
pub struct Pet {
    /// Pet name
    pub name: String,
    /// When the pet was created
    pub created_on: String,
    /// Pet skin ID (used for sprite rendering)
    pub skin: i32,
    /// Pet type ID
    pub pet_type: i32,
    /// Instance ID
    pub instance_id: i32,
    /// Maximum ability power
    pub max_ability_power: i32,
    /// Pet rarity (0=Common, 1=Uncommon, 2=Rare, 3=Legendary, 4=Divine)
    pub rarity: i32,
    /// Number of inventory slots available
    pub inventory_slots: i32,
    /// Pet inventory items (up to 8 slots)
    pub inventory: Vec<PetInventoryItem>,
    /// Pet abilities (up to 3)
    pub abilities: Vec<PetAbility>,
}

impl Pet {
    /// Get the rarity name.
    pub fn rarity_name(&self) -> &'static str {
        match self.rarity {
            0 => "Common",
            1 => "Uncommon",
            2 => "Rare",
            3 => "Legendary",
            4 => "Divine",
            _ => "Unknown",
        }
    }
}

/// A RotMG character with all its data.
#[derive(Debug, Clone)]
pub struct RealmCharacter {
    /// Character ID
    pub char_id: i32,
    /// Class type ID
    pub class_id: u16,
    /// Class enum
    pub class: CharacterClass,
    /// Character level (1-20)
    pub level: i32,
    /// Skin ID
    pub skin: i32,
    /// Clothing dye/cloth texture (Tex1 from API)
    pub tex1: u32,
    /// Accessory dye/cloth texture (Tex2 from API)
    pub tex2: u32,
    /// Total experience points
    pub exp: i64,
    /// Current fame
    pub fame: i64,
    /// Whether this is a seasonal character
    pub seasonal: bool,
    /// Whether crucible is active for this character (non-empty = active)
    pub crucible_active: bool,
    /// Whether character has a backpack
    pub has_backpack: bool,
    /// Number of backpack slots (0, 8, or 16)
    pub backpack_slots: i32,
    /// Whether character has 3 quickslots
    pub has_3_quickslots: bool,
    /// Equipment item IDs
    pub equipment: Vec<i32>,
    /// Equipment quickslot data
    pub equip_qs: Vec<String>,
    /// Character creation date
    pub creation_date: String,
    /// Max HP stat
    pub max_hp: i32,
    /// Max MP stat
    pub max_mp: i32,
    /// Attack stat
    pub attack: i32,
    /// Defense stat
    pub defense: i32,
    /// Speed stat
    pub speed: i32,
    /// Dexterity stat
    pub dexterity: i32,
    /// Vitality stat
    pub vitality: i32,
    /// Wisdom stat
    pub wisdom: i32,
    /// Raw PCStats string (encoded)
    pub pc_stats_raw: String,
    /// Decoded character statistics
    pub stats: Option<super::pcstats::CharacterStats>,
    /// Pet data (if equipped)
    pub pet: Option<Pet>,
    /// Unique item info (item_id -> ordered list of base64 encoded data with enchantments)
    /// Multiple entries for the same item_id are stored in order of appearance.
    pub unique_item_info: HashMap<i32, Vec<String>>,
    /// Remaining loot-drop-boost seconds for this character (`<LDTimer>`),
    /// when a boost is active. `None`/`0` means no active boost.
    pub loot_boost_secs: Option<u32>,
}

impl Default for RealmCharacter {
    fn default() -> Self {
        Self {
            char_id: 0,
            class_id: 0,
            class: CharacterClass::Unknown,
            level: 1,
            skin: 0,
            tex1: 0,
            tex2: 0,
            exp: 0,
            fame: 0,
            seasonal: false,
            crucible_active: false,
            has_backpack: false,
            backpack_slots: 0,
            has_3_quickslots: false,
            equipment: Vec::new(),
            equip_qs: Vec::new(),
            creation_date: String::new(),
            max_hp: 0,
            max_mp: 0,
            attack: 0,
            defense: 0,
            speed: 0,
            dexterity: 0,
            vitality: 0,
            wisdom: 0,
            pc_stats_raw: String::new(),
            stats: None,
            pet: None,
            unique_item_info: HashMap::new(),
            loot_boost_secs: None,
        }
    }
}

impl RealmCharacter {
    /// Get the class name as a string.
    pub fn class_name(&self) -> &'static str {
        self.class.name()
    }

    /// Check if this is a maxed character (level 20).
    pub fn is_maxed_level(&self) -> bool {
        self.level >= 20
    }

    /// Get total stat points (sum of all 8 stats).
    pub fn total_stats(&self) -> i32 {
        self.max_hp
            + self.max_mp
            + self.attack
            + self.defense
            + self.speed
            + self.dexterity
            + self.vitality
            + self.wisdom
    }
}

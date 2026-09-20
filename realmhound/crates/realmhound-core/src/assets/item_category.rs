//! Item categorization for the Treasury tab.
//!
//! Maps game objects (`ObjectAsset`) to category/sub-category pairs using asset
//! metadata (`class`, `group`, `labels`, `Projectile.slot_type`).
//!
//! The categorizer is built once from the full `ObjectList` and cached for O(1)
//! lookups by item ID.

use std::collections::HashMap;
use std::fmt;

use super::object_list::{ObjectAsset, ObjectList};

// ---------------------------------------------------------------------------
// Category enums
// ---------------------------------------------------------------------------

/// Top-level item category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemCategory {
    Weapons,
    Armors,
    Abilities,
    Accessories,
    Misc,
    Consumables,
    UtilityItems,
    Other,
}

impl ItemCategory {
    /// Display name for UI.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Weapons => "Weapons",
            Self::Armors => "Armors",
            Self::Abilities => "Abilities",
            Self::Accessories => "Accessories",
            Self::Misc => "Forge Tokens",
            Self::Consumables => "Consumables",
            Self::UtilityItems => "Utility Items",
            Self::Other => "Other",
        }
    }

    /// All categories in default display order.
    pub fn all() -> &'static [ItemCategory] {
        &[
            Self::Weapons,
            Self::Armors,
            Self::Abilities,
            Self::Accessories,
            Self::Misc,
            Self::Consumables,
            Self::UtilityItems,
            Self::Other,
        ]
    }

    /// Stable string key for settings persistence.
    pub fn to_key(&self) -> &'static str {
        match self {
            Self::Weapons => "weapons",
            Self::Armors => "armors",
            Self::Abilities => "abilities",
            Self::Accessories => "accessories",
            Self::Misc => "misc",
            Self::Consumables => "consumables",
            Self::UtilityItems => "utility_items",
            Self::Other => "other",
        }
    }

    /// Parse from a settings key string.
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "weapons" => Some(Self::Weapons),
            "armors" => Some(Self::Armors),
            "abilities" => Some(Self::Abilities),
            "accessories" => Some(Self::Accessories),
            "misc" => Some(Self::Misc),
            "consumables" => Some(Self::Consumables),
            "utility_items" => Some(Self::UtilityItems),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

impl fmt::Display for ItemCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.display_name())
    }
}

/// Sub-category within an `ItemCategory`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemSubCategory {
    // -- Weapons --
    Daggers,
    DualBlades,
    Bows,
    Longbows,
    Staves,
    Spellblades,
    Swords,
    Flails,
    Wands,
    MorningStars,
    Katanas,
    Tachis,

    // -- Armors --
    HeavyArmors,
    LeatherArmors,
    Robes,

    // -- Abilities --
    Cloaks,
    Prisms,
    Poisons,
    Quivers,
    Traps,
    Lutes,
    Spells,
    Orbs,
    Skulls,
    Shields,
    Seals,
    Helmets,
    Tomes,
    Scepters,
    Maces,
    Stars,
    Wakis,
    Sheaths,
    Sigils,

    // -- Accessories --
    Rings,

    // -- Misc --
    MiscEquipment,

    // -- Consumables --
    Potions,
    PotionsSB,
    GreaterPotions,
    Candies,
    Keys,
    DyesCloths,
    Skins,
    PetSkins,
    Unlockers,
    Boosters,
    HelpfulConsumables,
    PetEggs,
    Crates,
    Blueprints,
    Titles,
    Emotes,
    EventSpecial,

    // -- Utility Items --
    EnchantmentArtifacts,
    Ores,
    PetFood,
    Marks,
    VanityItems,
    EventTokens,

    // -- Other --
    Uncategorized,
}

impl ItemSubCategory {
    /// Display name for UI.
    pub fn display_name(&self) -> &'static str {
        match self {
            // Weapons
            Self::Daggers => "Daggers",
            Self::DualBlades => "Dual Blades",
            Self::Bows => "Bows",
            Self::Longbows => "Longbows",
            Self::Staves => "Staves",
            Self::Spellblades => "Spellblades",
            Self::Swords => "Swords",
            Self::Flails => "Flails",
            Self::Wands => "Wands",
            Self::MorningStars => "Morning Stars",
            Self::Katanas => "Katanas",
            Self::Tachis => "Tachis",
            // Armors
            Self::HeavyArmors => "Heavy Armors",
            Self::LeatherArmors => "Leather Armors",
            Self::Robes => "Robes",
            // Abilities
            Self::Cloaks => "Cloaks",
            Self::Prisms => "Prisms",
            Self::Poisons => "Poisons",
            Self::Quivers => "Quivers",
            Self::Traps => "Traps",
            Self::Lutes => "Lutes",
            Self::Spells => "Spells",
            Self::Orbs => "Orbs",
            Self::Skulls => "Skulls",
            Self::Shields => "Shields",
            Self::Seals => "Seals",
            Self::Helmets => "Helmets",
            Self::Tomes => "Tomes",
            Self::Scepters => "Scepters",
            Self::Maces => "Maces",
            Self::Stars => "Stars",
            Self::Wakis => "Wakis",
            Self::Sheaths => "Sheaths",
            Self::Sigils => "Sigils",
            // Accessories
            Self::Rings => "Rings",
            // Misc
            Self::MiscEquipment => "Misc Equipment",
            // Consumables
            Self::Potions => "Potions",
            Self::PotionsSB => "Potions (SB)",
            Self::GreaterPotions => "Greater Potions",
            Self::Candies => "Stat Candies",
            Self::Keys => "Keys",
            Self::DyesCloths => "Dyes & Cloths",
            Self::Skins => "Skins",
            Self::PetSkins => "Pet Skins",
            Self::Unlockers => "Unlockers",
            Self::Boosters => "Boosters",
            Self::EventTokens => "Shards",
            Self::HelpfulConsumables => "Restoratives",
            Self::PetEggs => "Pet Eggs",
            Self::Crates => "Crates",
            Self::Blueprints => "Blueprints",
            Self::Titles => "Titles",
            Self::Emotes => "Emotes",
            Self::EventSpecial => "Event Special",
            // Utility Items
            Self::EnchantmentArtifacts => "Enchantment Artifacts",
            Self::Ores => "Ores",
            Self::PetFood => "Pet Food",
            Self::Marks => "Marks",
            Self::VanityItems => "Vanity Items",
            // Other
            Self::Uncategorized => "Uncategorized",
        }
    }

    /// Get all sub-categories belonging to a parent category.
    pub fn for_category(category: ItemCategory) -> &'static [ItemSubCategory] {
        match category {
            ItemCategory::Weapons => &[
                Self::Daggers,
                Self::DualBlades,
                Self::Bows,
                Self::Longbows,
                Self::Staves,
                Self::Spellblades,
                Self::Swords,
                Self::Flails,
                Self::Wands,
                Self::MorningStars,
                Self::Katanas,
                Self::Tachis,
            ],
            ItemCategory::Armors => &[Self::HeavyArmors, Self::LeatherArmors, Self::Robes],
            ItemCategory::Abilities => &[
                Self::Cloaks,
                Self::Prisms,
                Self::Poisons,
                Self::Quivers,
                Self::Traps,
                Self::Lutes,
                Self::Spells,
                Self::Orbs,
                Self::Skulls,
                Self::Shields,
                Self::Seals,
                Self::Helmets,
                Self::Tomes,
                Self::Scepters,
                Self::Maces,
                Self::Stars,
                Self::Wakis,
                Self::Sheaths,
                Self::Sigils,
            ],
            ItemCategory::Accessories => &[Self::Rings],
            ItemCategory::Misc => &[Self::MiscEquipment],
            ItemCategory::Consumables => &[
                Self::Potions,
                Self::PotionsSB,
                Self::GreaterPotions,
                Self::Candies,
                Self::Keys,
                Self::DyesCloths,
                Self::Skins,
                Self::PetSkins,
                Self::Unlockers,
                Self::Boosters,
                Self::HelpfulConsumables,
                Self::PetEggs,
                Self::Crates,
                Self::Blueprints,
                Self::Titles,
                Self::Emotes,
                Self::EventSpecial,
            ],
            ItemCategory::UtilityItems => &[
                Self::EnchantmentArtifacts,
                Self::Ores,
                Self::PetFood,
                Self::Marks,
                Self::VanityItems,
                Self::EventTokens,
            ],
            ItemCategory::Other => &[Self::Uncategorized],
        }
    }
}

impl fmt::Display for ItemSubCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.display_name())
    }
}

// ---------------------------------------------------------------------------
// Weapon slot type constants (from RotMG game data)
// ---------------------------------------------------------------------------

/// RotMG slot types for equipment classification.
///
/// These values come from the `SlotType` element in game XML data.
/// Verified against extracted ObjectID.list from game assets.
///
/// Note: Newer weapon variants share slot types with classic weapons:
///   1 = Sword + Flail
///   2 = Dagger + Dual Blade
///   3 = Bow + Longbow
///   8 = Wand + Morning Star
///  17 = Staff + Spellblade
///  24 = Katana + Tachi
/// Name-based disambiguation is used for these shared slots.
mod slot_types {
    // -- Weapons (classic) --
    /// Also used by Flails
    pub const SWORD: i32 = 1;
    /// Also used by Dual Blades
    pub const DAGGER: i32 = 2;
    /// Also used by Longbows
    pub const BOW: i32 = 3;
    /// Also used by Morning Stars
    pub const WAND: i32 = 8;
    /// Also used by Spellblades
    pub const STAFF: i32 = 17;
    /// Also used by Tachis
    pub const KATANA: i32 = 24;

    // -- Armors --
    pub const LIGHT_ARMOR: i32 = 6;
    pub const HEAVY_ARMOR: i32 = 7;
    pub const ROBE: i32 = 14;

    // -- Abilities --
    pub const TOME: i32 = 4;
    pub const SHIELD: i32 = 5;
    pub const SPELL: i32 = 11;
    pub const SEAL: i32 = 12;
    pub const CLOAK: i32 = 13;
    pub const QUIVER: i32 = 15;
    pub const HELM: i32 = 16;
    pub const POISON: i32 = 18;
    pub const SKULL: i32 = 19;
    pub const TRAP: i32 = 20;
    pub const ORB: i32 = 21;
    pub const PRISM: i32 = 22;
    pub const SCEPTER: i32 = 23;
    pub const STAR: i32 = 25;
    pub const WAKIZASHI: i32 = 27;
    pub const LUTE: i32 = 28;
    pub const MACE: i32 = 29;
    pub const SHEATH: i32 = 30;

    // -- Accessories --
    pub const RING: i32 = 9;

    // -- Other --
    pub const CONSUMABLE: i32 = 10;
    pub const EGG: i32 = 26;
}

/// Resolve a game `SlotType` name (as it appears in mission `wornRestriction`
/// entries, e.g. `"ORB"`, `"SKULL"`, `"KATANA"`) to its numeric slot type.
/// Case-insensitive. Returns `None` for unknown names so callers can degrade
/// gracefully rather than guess. Kept in sync with the [`slot_types`] table.
pub fn slot_type_from_name(name: &str) -> Option<i32> {
    use slot_types::*;
    Some(match name.trim().to_ascii_uppercase().as_str() {
        "SWORD" => SWORD,
        "DAGGER" => DAGGER,
        "BOW" => BOW,
        "WAND" => WAND,
        "STAFF" => STAFF,
        "KATANA" => KATANA,
        "LIGHT_ARMOR" | "LEATHER" | "LEATHER_ARMOR" => LIGHT_ARMOR,
        "HEAVY_ARMOR" | "HEAVY" => HEAVY_ARMOR,
        "ROBE" => ROBE,
        "TOME" => TOME,
        "SHIELD" => SHIELD,
        "SPELL" => SPELL,
        "SEAL" => SEAL,
        "CLOAK" => CLOAK,
        "QUIVER" => QUIVER,
        "HELM" | "HELMET" => HELM,
        "POISON" => POISON,
        "SKULL" => SKULL,
        "TRAP" => TRAP,
        "ORB" => ORB,
        "PRISM" => PRISM,
        "SCEPTER" => SCEPTER,
        "STAR" => STAR,
        "WAKIZASHI" | "WAKI" => WAKIZASHI,
        "LUTE" => LUTE,
        "MACE" => MACE,
        "SHEATH" => SHEATH,
        "RING" => RING,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// ItemCategorizer
// ---------------------------------------------------------------------------

/// Cached categorization of all game items.
///
/// Built once from the full `ObjectList` and provides O(1) lookups by item ID.
pub struct ItemCategorizer {
    /// Map from item ID to (category, sub-category).
    cache: HashMap<i32, (ItemCategory, ItemSubCategory)>,
}

impl ItemCategorizer {
    /// Build the categorizer from the full object list.
    pub fn from_object_list(objects: &ObjectList) -> Self {
        let mut cache = HashMap::new();
        for asset in objects.iter() {
            let (cat, sub) = Self::categorize(asset);
            cache.insert(asset.id, (cat, sub));
        }
        Self { cache }
    }

    /// Look up the category for an item ID.
    pub fn get(&self, item_id: i32) -> (ItemCategory, ItemSubCategory) {
        self.cache
            .get(&item_id)
            .copied()
            .unwrap_or((ItemCategory::Other, ItemSubCategory::Uncategorized))
    }

    /// Get only the top-level category for an item ID.
    pub fn category(&self, item_id: i32) -> ItemCategory {
        self.get(item_id).0
    }

    /// Get only the sub-category for an item ID.
    pub fn sub_category(&self, item_id: i32) -> ItemSubCategory {
        self.get(item_id).1
    }

    /// Categorize a single object asset.
    pub fn categorize(asset: &ObjectAsset) -> (ItemCategory, ItemSubCategory) {
        if let Some(result) = Self::special_override(asset) {
            return result;
        }

        if asset.slot_type != 0 {
            return Self::categorize_by_slot_type(asset);
        }

        if asset.class == "Equipment" {
            return Self::categorize_equipment(asset);
        }

        Self::categorize_non_equipment(asset)
    }

    fn has_label(asset: &ObjectAsset, tag: &str) -> bool {
        asset.labels.split(',').any(|l| l == tag)
    }

    fn special_override(asset: &ObjectAsset) -> Option<(ItemCategory, ItemSubCategory)> {
        let name = asset.name().to_lowercase();
        let id = asset.id_name.to_lowercase();

        // --- Class / label based (cross-slot) ---
        // Pet skins are their own asset class.
        if asset.class == "PetSkin" {
            return Some((ItemCategory::Consumables, ItemSubCategory::PetSkins));
        }
        // Sigils use an unmapped slot_type (31); identify by label.
        if Self::has_label(asset, "SIGIL") {
            return Some((ItemCategory::Abilities, ItemSubCategory::Sigils));
        }
        // Beekeeper's Flamethrower is labeled WAND but is a Morning Star.
        if name.contains("beekeeper's flamethrower") {
            return Some((ItemCategory::Weapons, ItemSubCategory::MorningStars));
        }

        // --- Forge Tokens: ST set tokens/shards + upgrade cores ---
        if name.contains("set token")
            || name.contains("set shard")
            || name.contains("mythical st gem")
            || name.contains("mythical st shard")
            || name.contains("concentrated soul fire")
            || name.contains("kogbold enhancement core")
            || Self::has_label(asset, "ALIEN_ESSENCE")
            || name.contains("vial of soul extract")
        {
            return Some((ItemCategory::Misc, ItemSubCategory::MiscEquipment));
        }

        // --- Shards: mystery ST shards + named dungeon/gem craft materials ---
        if name.contains("mystery st shard")
            || name.contains("st skin shard")
            || name.contains("mystery skin token")
            || name.contains("amethyst shard")
            || name.contains("sapphire shard")
            || name.contains("topaz shard")
            || name.contains("ancient fossil")
            || name.contains("ancient ruby")
            || name.contains("ancient topaz")
            || name.contains("agents of oryx shard")
            || name.contains("celestial stone")
            || name.contains("crystal of fortune")
            || name.contains("crystal of extreme fortune")
            || name.contains("crystallized heart")
            || name.contains("nightmatter shard")
            || name.contains("vortex sigil")
            || name.contains("heart-explosion entrance shard")
            || name.contains("shard of the")
        {
            return Some((ItemCategory::UtilityItems, ItemSubCategory::EventTokens));
        }

        // --- Ores: broad-use forge materials only (ores + nilshards) ---
        if matches!(
            name.as_str(),
            "common ore" | "rare ore" | "epic ore" | "legendary ore"
        ) || matches!(
            id.as_str(),
            "basic ore" | "greater ore" | "superior ore" | "paramount ore"
        ) || Self::has_label(asset, "NILSHARD")
            || name.contains("nilshard")
        {
            return Some((ItemCategory::UtilityItems, ItemSubCategory::Ores));
        }

        // --- Enchantment Artifacts: technologies, engravings, cards ---
        if Self::has_label(asset, "ARTIFACT") || Self::has_label(asset, "ENGRAVING") {
            return Some((
                ItemCategory::UtilityItems,
                ItemSubCategory::EnchantmentArtifacts,
            ));
        }

        // --- Titles (label-driven; must precede the Unlockers rule) ---
        if Self::has_label(asset, "TITLE") {
            return Some((ItemCategory::Consumables, ItemSubCategory::Titles));
        }

        // --- Marks ---
        if Self::has_label(asset, "MARK") || name.contains("mark of") {
            return Some((ItemCategory::UtilityItems, ItemSubCategory::Marks));
        }

        // --- Oryx Runes (dungeon keys for Oryx's Sanctuary) ---
        if name == "sword rune" || name == "shield rune" || name == "helmet rune" {
            return Some((ItemCategory::Consumables, ItemSubCategory::Keys));
        }

        // --- Restoratives: nildrops ---
        if name.contains("nildrop") {
            return Some((
                ItemCategory::Consumables,
                ItemSubCategory::HelpfulConsumables,
            ));
        }

        // --- Boosters: max-level potions, stat potion drop, sulphurs ---
        if name.contains("potion of max level")
            || name.contains("stat potion drop")
            || name.contains("sulphur")
        {
            return Some((ItemCategory::Consumables, ItemSubCategory::Boosters));
        }

        // --- Dyes & Cloths (authoritative Dye class; must precede Pet Food so
        // "Chocolate Bar" cloths aren't caught by the pet-food name match) ---
        if asset.class == "Dye" || name.contains("chocolate dye") {
            return Some((ItemCategory::Consumables, ItemSubCategory::DyesCloths));
        }

        // --- Pet Food overrides ---
        if name.contains("candy apple")
            || name.contains("christmas turkey leg")
            || name.contains("thanksgiving turkey")
        {
            return Some((ItemCategory::UtilityItems, ItemSubCategory::PetFood));
        }

        // --- Crates ---
        if name.contains("interregnum treasure chest") {
            return Some((ItemCategory::Consumables, ItemSubCategory::Crates));
        }

        // --- Vanity: journals / lore collectibles ---
        if id.contains("interregnumlore")
            || name.contains("vagrant's journal")
            || name.contains("ozuchi's vow")
            || name.contains("wanderer's journal")
            // The Valentine heart generator is a cosmetic pet spawner.
            || name == "valentine"
            || name == "energy signet"
        {
            return Some((ItemCategory::UtilityItems, ItemSubCategory::VanityItems));
        }

        // --- Event Special: snowman parts + candy of ominous spells ---
        // Gate on CONSUMABLE to avoid catching gear like "Coalbearing Quiver".
        // The Oryxmas Nexus snowman-accessory set (carrots, hats, scarves, coal,
        // tree branch) shares the "XN " id_name prefix; skip dev "test" items.
        if Self::has_label(asset, "CONSUMABLE")
            && (name.contains("snowman body")
                || name.contains("top hat")
                || name.contains("coal")
                // Jack Frost decorative carrots (Jungle/Spriteful/Holy/Frosty
                // /Sandy Carrot); excludes "Carrot Cake"/"Carrot Coin".
                || name.ends_with("carrot")
                || name.contains("candy of ominous spells")
                || (id.starts_with("xn ") && !id.contains("test")))
        {
            return Some((ItemCategory::Consumables, ItemSubCategory::EventSpecial));
        }

        // --- Restoratives: heals, wines, fishing food, temporary boosters ---
        // Runs last so specific overrides above (titles, marks, event special,
        // account boosters) win first. Most temporary consumables carry the
        // TAB_UT or APPLY_STAT_EFF label; tiered wines, fishing foods and basic
        // heal potions lack a distinguishing label and are matched by name.
        const WINES: &[&str] = &[
            "fire water",
            "cream spirit",
            "chardonnay",
            "melon liqueur",
            "cabernet",
            "vintage port",
            "sauvignon blanc",
            "muscat",
            "rice wine",
            "shiraz",
        ];
        const FISHING_FOOD: &[&str] = &[
            "akuma ramen",
            "barbecued shrimp",
            "grilled fish",
            "onigiri",
            "salmon roe",
            "seafood bento",
            "sizzled tentacle",
            "steamed clam",
            "sushi roll",
            "takoyaki",
            "tuna nigiri",
            "unadon",
        ];
        // Halloween Bottles, Halloween Treats and Christmas Stat Boosters carry only
        // CONSUMABLE + UT (same labels as account boosters), so match them by name.
        // Bottles use contains() to also catch the "Greater ..." variants. Gate on
        // CONSUMABLE to skip the Candy Cane emote and Santa's Sleigh event chest.
        const HALLOWEEN_BOTTLES: &[&str] = &[
            "suspicious milk bottle",
            "zombie bottle",
            "bat bottle",
            "ghost bottle",
            "pumpkin bottle",
            "frankenstein bottle",
            "mini oryx bottle",
        ];
        const HALLOWEEN_TREATS: &[&str] = &[
            "teal treat",
            "hazel treat",
            "azure treat",
            "crimson treat",
            "scarlet treat",
            "beryl treat",
            "onyx treat",
            "amaranth treat",
        ];
        const CHRISTMAS_BOOSTERS: &[&str] = &[
            "santa's sleigh",
            "egg nog",
            "fruitcake",
            "candy cane",
            "figgy pudding",
            "mistletoe",
            "frost cake",
            "blue ice candy",
        ];
        if Self::has_label(asset, "CONSUMABLE")
            && (HALLOWEEN_BOTTLES.iter().any(|b| name.contains(b))
                || HALLOWEEN_TREATS.contains(&name.as_str())
                || CHRISTMAS_BOOSTERS.contains(&name.as_str()))
        {
            return Some((
                ItemCategory::Consumables,
                ItemSubCategory::HelpfulConsumables,
            ));
        }
        if name.contains("health potion")
            || name.contains("magic potion")
            || name.contains("elixir of health")
            || name.contains("elixir of magic")
            || WINES.contains(&name.as_str())
            || FISHING_FOOD.contains(&name.as_str())
            // Santa's Reins companions buff stats; they carry COOLDOWN (no
            // CONSUMABLE label) and all end in "'s Reins".
            || name.ends_with("'s reins")
            // Sand/Ice Pails and Ice Castle (gate on CONSUMABLE to skip the
            // Ice Pail summon).
            || ((name == "sand pail" || name == "ice pail" || name == "ice castle")
                && Self::has_label(asset, "CONSUMABLE"))
            || (asset.class == "Equipment"
                && Self::has_label(asset, "CONSUMABLE")
                && (Self::has_label(asset, "TAB_UT")
                    || Self::has_label(asset, "APPLY_STAT_EFF")))
        {
            return Some((
                ItemCategory::Consumables,
                ItemSubCategory::HelpfulConsumables,
            ));
        }

        None
    }

    /// Authoritative weapon sub-category from game labels, if present.
    ///
    /// Reskins (Damnation, Water Pistol, Beach Umbrella, etc.) share slot_types
    /// with classic weapons and their names lack the classic keyword, but their
    /// labels always carry the true weapon type.
    fn weapon_sub_from_labels(asset: &ObjectAsset) -> Option<ItemSubCategory> {
        // Newer variants first (they can co-occur with the classic label).
        if Self::has_label(asset, "FLAIL") {
            Some(ItemSubCategory::Flails)
        } else if Self::has_label(asset, "DUALBLADE") {
            Some(ItemSubCategory::DualBlades)
        } else if Self::has_label(asset, "LONGBOW") {
            Some(ItemSubCategory::Longbows)
        } else if Self::has_label(asset, "MORNINGSTAR") {
            Some(ItemSubCategory::MorningStars)
        } else if Self::has_label(asset, "SPELLBLADE") {
            Some(ItemSubCategory::Spellblades)
        } else if Self::has_label(asset, "TACHI") {
            Some(ItemSubCategory::Tachis)
        } else if Self::has_label(asset, "SWORD") {
            Some(ItemSubCategory::Swords)
        } else if Self::has_label(asset, "DAGGER") {
            Some(ItemSubCategory::Daggers)
        } else if Self::has_label(asset, "BOW") {
            Some(ItemSubCategory::Bows)
        } else if Self::has_label(asset, "WAND") {
            Some(ItemSubCategory::Wands)
        } else if Self::has_label(asset, "STAFF") {
            Some(ItemSubCategory::Staves)
        } else if Self::has_label(asset, "KATANA") {
            Some(ItemSubCategory::Katanas)
        } else {
            None
        }
    }

    /// Categorize by the object-level `slot_type` field.
    /// This handles weapons, armors, abilities, rings, and consumables.
    fn categorize_by_slot_type(asset: &ObjectAsset) -> (ItemCategory, ItemSubCategory) {
        match asset.slot_type {
            // Weapons - shared slot types need name-based disambiguation
            slot_types::SWORD => (
                ItemCategory::Weapons,
                Self::disambiguate_weapon(
                    asset,
                    ItemSubCategory::Swords,
                    ItemSubCategory::Flails,
                    "flail",
                ),
            ),
            slot_types::DAGGER => (
                ItemCategory::Weapons,
                Self::disambiguate_weapon(
                    asset,
                    ItemSubCategory::Daggers,
                    ItemSubCategory::DualBlades,
                    "dual",
                ),
            ),
            slot_types::BOW => (
                ItemCategory::Weapons,
                Self::disambiguate_weapon(
                    asset,
                    ItemSubCategory::Bows,
                    ItemSubCategory::Longbows,
                    "longbow",
                ),
            ),
            slot_types::WAND => (
                ItemCategory::Weapons,
                Self::disambiguate_weapon(
                    asset,
                    ItemSubCategory::Wands,
                    ItemSubCategory::MorningStars,
                    "morning star",
                ),
            ),
            slot_types::STAFF => (
                ItemCategory::Weapons,
                Self::disambiguate_weapon(
                    asset,
                    ItemSubCategory::Staves,
                    ItemSubCategory::Spellblades,
                    "spellblade",
                ),
            ),
            slot_types::KATANA => (
                ItemCategory::Weapons,
                Self::disambiguate_weapon(
                    asset,
                    ItemSubCategory::Katanas,
                    ItemSubCategory::Tachis,
                    "tachi",
                ),
            ),
            slot_types::MACE => (ItemCategory::Abilities, ItemSubCategory::Maces),
            slot_types::SHEATH => (ItemCategory::Abilities, ItemSubCategory::Sheaths),

            // Armors
            slot_types::LIGHT_ARMOR => (ItemCategory::Armors, ItemSubCategory::LeatherArmors),
            slot_types::HEAVY_ARMOR => (ItemCategory::Armors, ItemSubCategory::HeavyArmors),
            slot_types::ROBE => (ItemCategory::Armors, ItemSubCategory::Robes),

            // Abilities
            slot_types::TOME => (ItemCategory::Abilities, ItemSubCategory::Tomes),
            slot_types::SHIELD => (ItemCategory::Abilities, ItemSubCategory::Shields),
            slot_types::SPELL => (ItemCategory::Abilities, ItemSubCategory::Spells),
            slot_types::SEAL => (ItemCategory::Abilities, ItemSubCategory::Seals),
            slot_types::CLOAK => (ItemCategory::Abilities, ItemSubCategory::Cloaks),
            slot_types::QUIVER => (ItemCategory::Abilities, ItemSubCategory::Quivers),
            slot_types::HELM => (ItemCategory::Abilities, ItemSubCategory::Helmets),
            slot_types::POISON => (ItemCategory::Abilities, ItemSubCategory::Poisons),
            slot_types::SKULL => (ItemCategory::Abilities, ItemSubCategory::Skulls),
            slot_types::TRAP => (ItemCategory::Abilities, ItemSubCategory::Traps),
            slot_types::ORB => (ItemCategory::Abilities, ItemSubCategory::Orbs),
            slot_types::PRISM => (ItemCategory::Abilities, ItemSubCategory::Prisms),
            slot_types::SCEPTER => (ItemCategory::Abilities, ItemSubCategory::Scepters),
            slot_types::STAR => (ItemCategory::Abilities, ItemSubCategory::Stars),
            slot_types::WAKIZASHI => (ItemCategory::Abilities, ItemSubCategory::Wakis),
            slot_types::LUTE => (ItemCategory::Abilities, ItemSubCategory::Lutes),

            // Accessories
            slot_types::RING => (ItemCategory::Accessories, ItemSubCategory::Rings),

            // Consumables / misc (slot_type 10, 26)
            slot_types::CONSUMABLE => Self::categorize_non_equipment(asset),
            slot_types::EGG => (ItemCategory::Consumables, ItemSubCategory::PetEggs),

            // Unknown slot type - try name-based fallback
            _ => {
                if asset.class == "Equipment" {
                    Self::categorize_equipment(asset)
                } else {
                    Self::categorize_non_equipment(asset)
                }
            }
        }
    }

    /// Disambiguate newer weapon variants that share a slot type with classic weapons.
    ///
    /// Checks if the item name contains the newer variant keyword; if so returns
    /// `newer`, otherwise returns `classic`.
    fn disambiguate_weapon(
        asset: &ObjectAsset,
        classic: ItemSubCategory,
        newer: ItemSubCategory,
        keyword: &str,
    ) -> ItemSubCategory {
        // Prefer authoritative weapon-type labels (fixes reskins).
        if let Some(sub) = Self::weapon_sub_from_labels(asset) {
            return sub;
        }
        let name = asset.name().to_lowercase();
        let id = asset.id_name.to_lowercase();
        let kw_no_space = keyword.replace(' ', "");
        if name.contains(keyword) || id.contains(&kw_no_space) {
            newer
        } else {
            classic
        }
    }

    /// Categorize an equipment item (class == "Equipment").
    /// Used as fallback when slot_type is 0.
    fn categorize_equipment(asset: &ObjectAsset) -> (ItemCategory, ItemSubCategory) {
        match asset.group.as_str() {
            "Weapon" => Self::categorize_weapon(asset),
            "Armor" => Self::categorize_armor(asset),
            "Ability" => Self::categorize_ability(asset),
            "Ring" => (ItemCategory::Accessories, ItemSubCategory::Rings),
            _ => {
                // Group is empty or unknown - try name-based detection across
                // all equipment types (weapons, armors, abilities, rings).
                Self::categorize_equipment_by_name(asset)
            }
        }
    }

    /// Name-based equipment categorization when both slot_type and group are
    /// unavailable. Tries weapon, armor, ability, and ring patterns.
    fn categorize_equipment_by_name(asset: &ObjectAsset) -> (ItemCategory, ItemSubCategory) {
        // Try weapon by name first
        let weapon_sub = Self::weapon_from_name(asset);
        if !matches!(weapon_sub, ItemSubCategory::Swords) || {
            let name = asset.name().to_lowercase();
            name.contains("sword")
        } {
            return (ItemCategory::Weapons, weapon_sub);
        }

        // Try armor by name
        let name = asset.name().to_lowercase();
        let id = asset.id_name.to_lowercase();
        if name.contains("robe") || id.contains("robe") {
            return (ItemCategory::Armors, ItemSubCategory::Robes);
        }
        if name.contains("armor")
            || name.contains("mail")
            || name.contains("plate")
            || id.contains("armor")
            || id.contains("heavyarmor")
        {
            return (ItemCategory::Armors, ItemSubCategory::HeavyArmors);
        }
        if name.contains("leather")
            || name.contains("hide")
            || name.contains("garment")
            || id.contains("leather")
            || id.contains("lightarmor")
            || id.contains("hide")
        {
            return (ItemCategory::Armors, ItemSubCategory::LeatherArmors);
        }

        // Try ability by name
        let (cat, sub) = Self::categorize_ability(asset);
        if !matches!(sub, ItemSubCategory::Uncategorized) {
            return (cat, sub);
        }

        // Try ring by name
        if name.contains("ring") || id.contains("ring") {
            return (ItemCategory::Accessories, ItemSubCategory::Rings);
        }

        // Fishing rods go to Accessories
        if name.contains("fishing rod") || id.contains("fishingrod") {
            return (ItemCategory::Accessories, ItemSubCategory::MiscEquipment);
        }

        // Pet skin stones (Equipment class but functionally pet skins)
        // Christmas Tree Skin belongs to Pet Skins
        if id.contains("pet stone")
            || id.contains("petstone")
            || name.contains("christmas tree skin")
            || (name.contains("pet skin") && !name.contains("unlocker"))
        {
            return (ItemCategory::Consumables, ItemSubCategory::PetSkins);
        }

        // Misc equipment: Spen Vial, Kog Core, etc.
        (ItemCategory::Misc, ItemSubCategory::MiscEquipment)
    }

    /// Categorize a weapon by its projectile slot_type or name.
    fn categorize_weapon(asset: &ObjectAsset) -> (ItemCategory, ItemSubCategory) {
        // Try projectile slot_type first (available in old-format ObjectID.list)
        let slot = asset.projectiles.first().map(|p| p.slot_type).unwrap_or(0);

        let sub = match slot {
            slot_types::SWORD => Self::disambiguate_weapon(
                asset,
                ItemSubCategory::Swords,
                ItemSubCategory::Flails,
                "flail",
            ),
            slot_types::DAGGER => Self::disambiguate_weapon(
                asset,
                ItemSubCategory::Daggers,
                ItemSubCategory::DualBlades,
                "dual",
            ),
            slot_types::BOW => Self::disambiguate_weapon(
                asset,
                ItemSubCategory::Bows,
                ItemSubCategory::Longbows,
                "longbow",
            ),
            slot_types::WAND => Self::disambiguate_weapon(
                asset,
                ItemSubCategory::Wands,
                ItemSubCategory::MorningStars,
                "morning star",
            ),
            slot_types::STAFF => Self::disambiguate_weapon(
                asset,
                ItemSubCategory::Staves,
                ItemSubCategory::Spellblades,
                "spellblade",
            ),
            slot_types::KATANA => Self::disambiguate_weapon(
                asset,
                ItemSubCategory::Katanas,
                ItemSubCategory::Tachis,
                "tachi",
            ),
            slot_types::MACE => ItemSubCategory::Maces,
            slot_types::SHEATH => ItemSubCategory::Sheaths,
            _ => {
                // Fallback: try name-based detection for weapons without projectile data
                Self::weapon_from_name(asset)
            }
        };

        (ItemCategory::Weapons, sub)
    }

    /// Fallback weapon sub-category from display name or id_name.
    fn weapon_from_name(asset: &ObjectAsset) -> ItemSubCategory {
        let name = asset.name().to_lowercase();
        let id = asset.id_name.to_lowercase();

        if name.contains("sword") || id.contains("sword") {
            ItemSubCategory::Swords
        } else if name.contains("dagger") || id.contains("dagger") || id.contains("dirk") {
            ItemSubCategory::Daggers
        } else if name.contains("longbow") || id.contains("longbow") {
            ItemSubCategory::Longbows
        } else if name.contains("bow") || id.contains("bow") {
            ItemSubCategory::Bows
        } else if name.contains("staff") || id.contains("staff") {
            ItemSubCategory::Staves
        } else if name.contains("wand") || id.contains("wand") {
            ItemSubCategory::Wands
        } else if name.contains("katana") || id.contains("katana") {
            ItemSubCategory::Katanas
        } else if name.contains("morning star") || id.contains("morningstar") {
            ItemSubCategory::MorningStars
        } else if name.contains("flail") || id.contains("flail") {
            ItemSubCategory::Flails
        } else if name.contains("spellblade") || id.contains("spellblade") {
            ItemSubCategory::Spellblades
        } else if name.contains("tachi") || id.contains("tachi") {
            ItemSubCategory::Tachis
        } else if name.contains("dual blade") || id.contains("dualblade") {
            ItemSubCategory::DualBlades
        } else {
            ItemSubCategory::Swords // default weapon
        }
    }

    /// Categorize armor by name pattern.
    fn categorize_armor(asset: &ObjectAsset) -> (ItemCategory, ItemSubCategory) {
        let name = asset.name().to_lowercase();
        let id = asset.id_name.to_lowercase();

        let sub = if name.contains("robe") || id.contains("robe") {
            ItemSubCategory::Robes
        } else if name.contains("leather") || id.contains("leather") || id.contains("hide") {
            ItemSubCategory::LeatherArmors
        } else {
            // Default equipment-category armors to heavy
            ItemSubCategory::HeavyArmors
        };

        (ItemCategory::Armors, sub)
    }

    /// Categorize ability by name/id_name pattern.
    fn categorize_ability(asset: &ObjectAsset) -> (ItemCategory, ItemSubCategory) {
        let name = asset.name().to_lowercase();
        let id = asset.id_name.to_lowercase();

        let sub = if name.contains("cloak") || id.contains("cloak") {
            ItemSubCategory::Cloaks
        } else if name.contains("prism") || id.contains("prism") {
            ItemSubCategory::Prisms
        } else if name.contains("poison") || id.contains("poison") {
            ItemSubCategory::Poisons
        } else if name.contains("quiver") || id.contains("quiver") {
            ItemSubCategory::Quivers
        } else if name.contains("trap") || id.contains("trap") {
            ItemSubCategory::Traps
        } else if name.contains("lute") || id.contains("lute") {
            ItemSubCategory::Lutes
        } else if name.contains("spell") || id.contains("spell") {
            ItemSubCategory::Spells
        } else if name.contains("orb") || id.contains("orb") {
            ItemSubCategory::Orbs
        } else if name.contains("skull") || id.contains("skull") {
            ItemSubCategory::Skulls
        } else if name.contains("shield") || id.contains("shield") {
            ItemSubCategory::Shields
        } else if name.contains("seal") || id.contains("seal") {
            ItemSubCategory::Seals
        } else if name.contains("helm") || id.contains("helm") {
            ItemSubCategory::Helmets
        } else if name.contains("tome") || id.contains("tome") {
            ItemSubCategory::Tomes
        } else if name.contains("scepter") || id.contains("scepter") {
            ItemSubCategory::Scepters
        } else if name.contains("mace") || id.contains("mace") {
            ItemSubCategory::Maces
        } else if name.contains("star") || id.contains("star") {
            ItemSubCategory::Stars
        } else if name.contains("waki") || id.contains("waki") || name.contains("wakizashi") {
            ItemSubCategory::Wakis
        } else if name.contains("sheath") || id.contains("sheath") {
            ItemSubCategory::Sheaths
        } else if name.contains("sigil") || id.contains("sigil") {
            ItemSubCategory::Sigils
        } else {
            // Unknown ability type
            ItemSubCategory::Uncategorized
        };

        if matches!(sub, ItemSubCategory::Uncategorized) {
            (ItemCategory::Other, sub)
        } else {
            (ItemCategory::Abilities, sub)
        }
    }

    /// Categorize non-equipment items (consumables, utility, etc.).
    fn categorize_non_equipment(asset: &ObjectAsset) -> (ItemCategory, ItemSubCategory) {
        let name = asset.name().to_lowercase();
        let id = asset.id_name.to_lowercase();
        let labels = asset.labels.to_lowercase();

        // =====================================================================
        // SPECIFIC ITEM OVERRIDES (checked before pattern matching)
        // =====================================================================

        // --- Stat Candies (only permanent stat increase items) ---
        // Apple of Extreme Maxening and Welcome Back Gift go here
        // Stat candies follow pattern "Candy of Extreme [Stat]"
        if name.contains("apple of extreme maxening")
            || name.contains("welcome back gift")
            || name.contains("seasonal jumpstart gift")
            || name.contains("candy of extreme")
        {
            return (ItemCategory::Consumables, ItemSubCategory::Candies);
        }

        // --- Pet Food (full realmeye wiki list) ---
        // Stackable pet food has an empty display name and an id_name like
        // "Cheese x3", so name() returns "cheese x3"; strip the " xN" suffix to
        // match the base name. Recipe items ("... Recipe", no CONSUMABLE label)
        // are naturally excluded by exact base-name matching. All ambrosia
        // variants are caught by the contains() check.
        let food_base = match name.rsplit_once(" x") {
            Some((head, tail)) if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) => {
                head
            }
            _ => name.as_str(),
        };
        const PET_FOOD: &[&str] = &[
            "apple pie",
            "asteralis root",
            "astral jelly",
            "atomic taco",
            "banana",
            "broccoli",
            "candy apple",
            "caramel apple",
            "carrot cake",
            "chaos cake",
            "cheese",
            "chocolate bar",
            "chocolate bonbon",
            "chocolate cream sandwich cookie",
            "christmas tree cupcake",
            "christmas turkey leg",
            "cornbread",
            "cosmic spice",
            "cranberries",
            "daeva elixir",
            "deadly dapperling",
            "double cheeseburger deluxe",
            "drowned shamrock",
            "ear of corn",
            "egg omelette",
            "energy potato",
            "enriched bee steak",
            "eyecicle",
            "french toast",
            "fries",
            "gingerbread house",
            "glazed apple",
            "glowing green goo",
            "gold bar",
            "grapes of heaven",
            "grapes of wrath",
            "great taco",
            "grilled honey comb",
            "heart-shaped chocolate box",
            "honeyed snake skewer",
            "hot cross buns",
            "hot sauce",
            "ice cream",
            "jack-o-lantern",
            "latte",
            "mashed potatoes",
            "mead",
            "mouthwatering melon",
            "napalm taco",
            "oryx cookie",
            "pecan pie",
            "picante taco",
            "power pizza",
            "protein shake",
            "pumpkin pie",
            "pumpkin spice latte",
            "rainbow gummy worm",
            "reindeer food",
            "ripe pumpkin",
            "roast biff",
            "royal marbled chocolate",
            "sliced yam",
            "snow cone",
            "soft drink",
            "solar cupcake",
            "solar energy drink",
            "solar potato",
            "space jam",
            "space ration",
            "steamed zombie steak",
            "strawberry milkshake",
            "sugar gummy worm",
            "superburger",
            "thanksgiving turkey",
            "the appetizer's spicy oryx cookie",
            "the appetizer's spicy power pizza",
            "the appetizer's spicy superburger",
            "turkey cake",
            "vamp steak flambe'",
            "void venom",
            "xeno heart",
        ];
        if PET_FOOD.contains(&food_base)
            || food_base.contains("ambrosia")
            || labels.contains("pet_food")
            || id.contains("petfood")
        {
            return (ItemCategory::UtilityItems, ItemSubCategory::PetFood);
        }

        // --- Crates (chests, selection chests, mystery boxes) ---
        // Selection Chests (Beginner/Intermediate/Advanced/Expert Weapon/Ability/Armor/Ring)
        if name.contains("selection chest")
            || name.contains("armaments")
            || name.contains("reliquary")
            || name.contains("armory")
            || name.contains("armor chest")
            // Tiered gear/dust chests and weapon caches (Beginner/Advanced/Expert
            // /Aspirant/Oryxmas/Easter Weapon/Ring/Ability/Armor/Dust chests).
            || name.ends_with("weapon chest")
            || name.ends_with("weapons chest")
            || name.ends_with("ring chest")
            || name.ends_with("ability chest")
            || name.ends_with("dust chest")
            || name.ends_with("arsenal chest")
            || name.contains("weapon cache")
            // Seasonal Character Rewards: "<Tier> <Weapon|Ability|Armor|Ring>"
            // (Beginner/Intermediate/Advanced/Expert), consumed into tiered gear.
            || ((name.starts_with("beginner ")
                || name.starts_with("intermediate ")
                || name.starts_with("advanced ")
                || name.starts_with("expert "))
                && (name.ends_with(" weapon")
                    || name.ends_with(" ability")
                    || name.ends_with(" armor")
                    || name.ends_with(" ring")))
            || name == "shards of destiny"
            || name.contains("music box")
            || name.contains("eerie music box")
            || name.contains("leprechaun's lucky hat")
            || name.contains("demon pumpkin")
            || name.contains("collection chest")
            || name.contains("pet takeout")
            || name.contains("stat potion choice")
            || name.contains("quest chest")
            || name.contains("enchantment's secrets")
            || name.contains("buried treasure")
            || name.contains("legion treasure")
            || name.contains("mystery") // catches Mystery skins/items → Crates
            || name == "gift of the void"
            || name == "gift of the daeva"
            || name == "silver urn"
            || id.contains("questchest")
            || id.contains("crate")
        {
            return (ItemCategory::Consumables, ItemSubCategory::Crates);
        }

        // --- Boosters (loot, xp, dust, artifact drop potions) ---
        if name.contains("loot drop potion")
            || name.contains("loot tier potion")
            || name.contains("dust shard")
            || name.contains("dust charm")
            || name.contains("dust essence")
            || name.contains("dust gem")
            || name.contains("dust volume")
            || name.contains("dust drop")
            || name.contains("lucky golden rat")
            || name.contains("lucky clover")
            || name.contains("lucky golden clover")
            || name.contains("artifact drop potion")
            || name.contains("booster")
            || name.contains("boost")
            || id.contains("lootdrop")
            || id.contains("loottier")
            || id.contains("xpboost")
            || id.contains("dustboost")
        {
            return (ItemCategory::Consumables, ItemSubCategory::Boosters);
        }

        // --- Unlockers (backpacks, slots, belts, coupons) ---
        if name.contains("adventurer's belt")
            || name.contains("santa's bag")
            || name.contains("chest coupon")
            || name.contains("char slot coupon")
            || name.contains("character slot coupon")
            || name.contains("unlocker")
            || name.contains("backpack")
            || id.contains("charslot")
            || id.contains("vaultchest")
        {
            return (ItemCategory::Consumables, ItemSubCategory::Unlockers);
        }

        // --- Portal Keys (runes, incantations, vials) ---
        if name.contains("wine cellar incantation")
            || name.contains("vial of pure darkness")
            || name.contains("treasure map")
            || id.contains("rune_")
            || id.contains("incantation")
            || id.contains("lhvial")
            || (name.contains("key") && !name.contains("turkey"))
        // exclude Turkey Key (vanity)
        {
            return (ItemCategory::Consumables, ItemSubCategory::Keys);
        }

        // --- Blueprints (including schematics) ---
        if name.contains("blueprint")
            || name.contains("schematic")
            || id.contains("blueprint")
            || id.contains("schematic")
        {
            return (ItemCategory::Consumables, ItemSubCategory::Blueprints);
        }

        // --- Enchantment Artifacts (cards, cores, engravings, tarot) ---
        if name.contains("premium silver card")
            || name.contains("premium gold card")
            || name.contains("amber honeycomb")
            || name.contains("ivory heart")
            || name.contains("adamantine ingot")
            || name.contains("nightmatter core")
            || name.contains("precision cog")
            || name.contains("ascension ankh")
            || name.contains("spectral arrowhead")
            || name.contains("freezing core")
            || id.contains("tarot")
            || id.contains("engraving")
            || labels.contains("enchantment")
        {
            return (
                ItemCategory::UtilityItems,
                ItemSubCategory::EnchantmentArtifacts,
            );
        }

        // --- Vanity Items (cosmetics, fun items, collectibles) ---
        if name.contains("valentine's egg")
            || name.contains("valentines egg")
            || name.contains("beach ball")
            || name.contains("valentine launcher")
            || name.contains("veteran of")
            || name.contains("exalt trophy")
            || name.contains("paddy's flying hat")
            || name.contains("rainbow clover")
            || name.contains("beer slurp")
            || name.contains("pet rock")
            || name.contains("snowman")  // snowman accessories
            || name.contains("jack frost")
            || name.contains("serpentine memory orb")
            || id.contains("lorebook")
            || id.contains("collectible")
        {
            return (ItemCategory::UtilityItems, ItemSubCategory::VanityItems);
        }

        // =====================================================================
        // GENERAL PATTERN MATCHING
        // =====================================================================

        // Marks (utility)
        if name.starts_with("mark of") || id.starts_with("mark_") || id.contains("completionmark") {
            return (ItemCategory::UtilityItems, ItemSubCategory::Marks);
        }

        // Pet eggs (before general egg check)
        if name.contains("egg")
            && (name.contains("pet") || id.contains("petegg") || labels.contains("pet_egg"))
        {
            return (ItemCategory::Consumables, ItemSubCategory::PetEggs);
        }

        // Pet skins (including Christmas Tree Skin)
        if (name.contains("pet skin")
            || id.contains("petskin")
            || name.contains("christmas tree skin"))
            && !name.contains("pet skin unlocker")
        {
            return (ItemCategory::Consumables, ItemSubCategory::PetSkins);
        }

        // Skins (character)
        if (name.contains("skin") && !name.contains("pet"))
            || id.contains("_skin_")
            || labels.contains("skin")
        {
            // Exclude pet skins (already handled) and pet skin unlockers
            if !id.contains("petskin") && !name.contains("pet skin") {
                return (ItemCategory::Consumables, ItemSubCategory::Skins);
            }
        }

        // Enchantment artifacts (tarot cards, engravings, dungeon artifacts)
        if labels.contains("enchantment")
            || id.contains("enchantment")
            || id.contains("tarot")
            || id.contains("engraving")
            || name.contains("artifact")
        {
            return (
                ItemCategory::UtilityItems,
                ItemSubCategory::EnchantmentArtifacts,
            );
        }

        // Ores and Nilshards (broad-use forge materials only; specific craft
        // materials are handled as Shards/Forge Tokens in special_override).
        if matches!(
            name.as_str(),
            "common ore" | "rare ore" | "epic ore" | "legendary ore"
        ) || name.contains("nilshard")
        {
            return (ItemCategory::UtilityItems, ItemSubCategory::Ores);
        }

        // Keys / portal keys
        if name.contains("key")
            || id.contains("_key")
            || id.contains("incantation")
            || id.contains("rune_")
            || id.contains("lhvial")
        {
            return (ItemCategory::Consumables, ItemSubCategory::Keys);
        }

        // Blueprints
        if name.contains("blueprint") || id.contains("blueprint") {
            return (ItemCategory::Consumables, ItemSubCategory::Blueprints);
        }

        // Titles
        if name.contains("title") || id.contains("title_") {
            return (ItemCategory::Consumables, ItemSubCategory::Titles);
        }

        // Emotes
        if name.contains("emote") || id.contains("emote_") {
            return (ItemCategory::Consumables, ItemSubCategory::Emotes);
        }

        // Dyes and cloths
        if name.contains("dye")
            || name.contains("cloth")
            || id.contains("dye_")
            || id.contains("cloth_")
        {
            return (ItemCategory::Consumables, ItemSubCategory::DyesCloths);
        }

        // Unlockers (backpacks, char slots, potion belts, vault chests)
        if name.contains("unlocker")
            || name.contains("backpack")
            || id.contains("unlocker")
            || id.contains("charslot")
            || id.contains("vaultchest")
        {
            return (ItemCategory::Consumables, ItemSubCategory::Unlockers);
        }

        // Boosters (loot, xp, dust, etc.)
        if name.contains("booster")
            || name.contains("boost")
            || id.contains("booster")
            || id.contains("lootdrop")
            || id.contains("xpboost")
        {
            return (ItemCategory::Consumables, ItemSubCategory::Boosters);
        }

        // Crates / mystery items / quest chests
        if name.contains("crate")
            || name.contains("mystery")
            || id.contains("crate")
            || id.contains("mystery")
            || id.contains("questchest")
        {
            return (ItemCategory::Consumables, ItemSubCategory::Crates);
        }

        // Event tokens / Shards (moved to Utility Items)
        // Golden Egg Token is an old event shard exception
        if name.contains("token")
            || id.contains("token")
            || labels.contains("event_token")
            || name.contains("golden egg token")
        {
            return (ItemCategory::UtilityItems, ItemSubCategory::EventTokens);
        }

        // Greater stat potions
        if (name.contains("greater potion") || name.contains("greater pot"))
            || (id.contains("greaterpot") || id.contains("greaterpotion"))
        {
            return (ItemCategory::Consumables, ItemSubCategory::GreaterPotions);
        }

        // Candies - candy-themed temp boosts go to Helpful Consumables
        if name.contains("candy") || id.contains("candy") {
            return (
                ItemCategory::Consumables,
                ItemSubCategory::HelpfulConsumables,
            );
        }

        // Helpful consumables (ichors, tinctures, nildrops, drake eggs, effusions)
        if name.contains("ichor")
            || name.contains("tincture")
            || name.contains("nildrop")
            || name.contains("drake egg")
            || name.contains("effusion")
            || id.contains("ichor")
            || id.contains("tincture")
            || id.contains("nildrop")
            || id.contains("drakeegg")
            || id.contains("effusion")
        {
            return (
                ItemCategory::Consumables,
                ItemSubCategory::HelpfulConsumables,
            );
        }

        // Stat potions (SB vs tradeable)
        if name.contains("potion of")
            || name.contains("pot of")
            || id.contains("statpot")
            || id.contains("potion_")
        {
            if labels.contains("soulbound") || labels.contains("sb") || name.contains("(sb)") {
                return (ItemCategory::Consumables, ItemSubCategory::PotionsSB);
            }
            return (ItemCategory::Consumables, ItemSubCategory::Potions);
        }

        // Vanity items (treasure, lore books, collectibles)
        if name.contains("treasure")
            || name.contains("lore")
            || id.contains("treasure")
            || id.contains("lorebook")
            || id.contains("collectible")
        {
            return (ItemCategory::UtilityItems, ItemSubCategory::VanityItems);
        }

        // Generic consumable class fallback
        if asset.class == "Consumable" || asset.class == "Potion" {
            return (ItemCategory::Consumables, ItemSubCategory::Potions);
        }

        (ItemCategory::Other, ItemSubCategory::Uncategorized)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a minimal ObjectAsset with given fields.
    fn make_asset(
        id: i32,
        id_name: &str,
        display_name: &str,
        class: &str,
        group: &str,
        labels: &str,
        slot_type: Option<i32>,
    ) -> ObjectAsset {
        let projectiles = if let Some(st) = slot_type {
            vec![super::super::object_list::Projectile {
                min_damage: 100,
                max_damage: 200,
                armor_piercing: false,
                slot_type: st,
                display_id: None,
                ignore_on_tooltip: false,
            }]
        } else {
            Vec::new()
        };

        ObjectAsset {
            id,
            id_name: id_name.to_string(),
            display_name: display_name.to_string(),
            class: class.to_string(),
            group: group.to_string(),
            labels: labels.to_string(),
            textures: Vec::new(),
            projectiles,
            mask: None,
            tex1: 0,
            tex2: 0,
            slot_type: slot_type.unwrap_or(0),
            defense: 0,
            hitbox_scale: 1.0,
            tier: None,
            feed_power: 0,
            fame_bonus: 0,
            bag_type: 0,
            set_name: None,
            lethal_strike: None,
            collection_icon: None,
            rate_of_fire: 1.0,
            num_projectiles: 1,
            stat_bonuses: Default::default(),
            weapon_procs: Vec::new(),
        }
    }

    // -- Weapon sub-categories by slot_type --

    #[test]
    fn test_sword_by_slot_type() {
        let a = make_asset(
            1,
            "T14_Sword",
            "Sword of the Colossus",
            "Equipment",
            "Weapon",
            "",
            Some(1),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Weapons, ItemSubCategory::Swords)
        );
    }

    #[test]
    fn test_dagger_by_slot_type() {
        let a = make_asset(
            2,
            "T14_Dagger",
            "Etherite Dagger",
            "Equipment",
            "Weapon",
            "",
            Some(2),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Weapons, ItemSubCategory::Daggers)
        );
    }

    #[test]
    fn test_bow_by_slot_type() {
        let a = make_asset(
            3,
            "T14_Bow",
            "Bow of Covert Havens",
            "Equipment",
            "Weapon",
            "",
            Some(3),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Weapons, ItemSubCategory::Bows)
        );
    }

    #[test]
    fn test_wand_by_slot_type() {
        let a = make_asset(
            4,
            "T14_Wand",
            "Wand of Recompense",
            "Equipment",
            "Weapon",
            "",
            Some(8),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Weapons, ItemSubCategory::Wands)
        );
    }

    #[test]
    fn test_katana_by_slot_type() {
        let a = make_asset(
            5,
            "T14_Katana",
            "Masamune",
            "Equipment",
            "Weapon",
            "",
            Some(24),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Weapons, ItemSubCategory::Katanas)
        );
    }

    #[test]
    fn test_staff_by_slot_type() {
        let a = make_asset(
            6,
            "T14_Staff",
            "Staff of the Cosmic Whole",
            "Equipment",
            "Weapon",
            "",
            Some(17),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Weapons, ItemSubCategory::Staves)
        );
    }

    #[test]
    fn test_morning_star_by_shared_slot() {
        // Morning Stars share slot_type 8 with Wands, disambiguated by name
        let a = make_asset(
            7,
            "MorningStar1",
            "Morning Star of Reckoning",
            "Equipment",
            "Weapon",
            "",
            Some(8),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Weapons, ItemSubCategory::MorningStars)
        );
    }

    #[test]
    fn test_flail_by_shared_slot() {
        // Flails share slot_type 1 with Swords, disambiguated by name
        let a = make_asset(
            8,
            "Flail1",
            "Interregnum Flail",
            "Equipment",
            "Weapon",
            "",
            Some(1),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Weapons, ItemSubCategory::Flails)
        );
    }

    #[test]
    fn test_longbow_by_shared_slot() {
        // Longbows share slot_type 3 with Bows, disambiguated by name
        let a = make_asset(
            9,
            "Longbow1",
            "Longbow of the Void",
            "Equipment",
            "Weapon",
            "",
            Some(3),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Weapons, ItemSubCategory::Longbows)
        );
    }

    #[test]
    fn test_spellblade_by_shared_slot() {
        // Spellblades share slot_type 17 with Staves, disambiguated by name
        let a = make_asset(
            10,
            "Spellblade1",
            "Spellblade of Decimation",
            "Equipment",
            "Weapon",
            "",
            Some(17),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Weapons, ItemSubCategory::Spellblades)
        );
    }

    #[test]
    fn test_tachi_by_shared_slot() {
        // Tachis share slot_type 24 with Katanas, disambiguated by name
        let a = make_asset(
            11,
            "Tachi1",
            "Cloudstrike Tachi",
            "Equipment",
            "Weapon",
            "",
            Some(24),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Weapons, ItemSubCategory::Tachis)
        );
    }

    #[test]
    fn test_dual_blade_by_shared_slot() {
        // Dual Blades share slot_type 2 with Daggers, disambiguated by name
        let a = make_asset(
            12,
            "DualBlade1",
            "Dual Pops",
            "Equipment",
            "Weapon",
            "",
            Some(2),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Weapons, ItemSubCategory::DualBlades)
        );
    }

    #[test]
    fn test_mace_by_slot_type() {
        let a = make_asset(
            13,
            "Mace1",
            "Crystal Mace",
            "Equipment",
            "Ability",
            "",
            Some(29),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Abilities, ItemSubCategory::Maces)
        );
    }

    #[test]
    fn test_sheath_by_slot_type() {
        let a = make_asset(
            14,
            "Sheath1",
            "Volcanic Sheath",
            "Equipment",
            "Ability",
            "",
            Some(30),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Abilities, ItemSubCategory::Sheaths)
        );
    }

    // -- Armor sub-categories --

    #[test]
    fn test_heavy_armor() {
        let a = make_asset(
            20,
            "T15_HeavyArmor",
            "Acropolis Armor",
            "Equipment",
            "Armor",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Armors, ItemSubCategory::HeavyArmors)
        );
    }

    #[test]
    fn test_leather_armor() {
        let a = make_asset(
            21,
            "T15_LeatherArmor",
            "Wyrmhide Leather",
            "Equipment",
            "Armor",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Armors, ItemSubCategory::LeatherArmors)
        );
    }

    #[test]
    fn test_robe() {
        let a = make_asset(
            22,
            "T15_Robe",
            "Robe of the Grand Sorcerer",
            "Equipment",
            "Armor",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Armors, ItemSubCategory::Robes)
        );
    }

    // -- Ability sub-categories --

    #[test]
    fn test_cloak() {
        let a = make_asset(
            30,
            "T7_Cloak",
            "Cloak of Ghostly Protection",
            "Equipment",
            "Ability",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Abilities, ItemSubCategory::Cloaks)
        );
    }

    #[test]
    fn test_shield() {
        let a = make_asset(
            31,
            "T7_Shield",
            "Shield of Ogmur",
            "Equipment",
            "Ability",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Abilities, ItemSubCategory::Shields)
        );
    }

    #[test]
    fn test_tome() {
        let a = make_asset(
            32,
            "T7_Tome",
            "Tome of Holy Protection",
            "Equipment",
            "Ability",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Abilities, ItemSubCategory::Tomes)
        );
    }

    #[test]
    fn test_quiver() {
        let a = make_asset(
            33,
            "T7_Quiver",
            "Quiver of Elvish Mastery",
            "Equipment",
            "Ability",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Abilities, ItemSubCategory::Quivers)
        );
    }

    #[test]
    fn test_helm() {
        let a = make_asset(
            34,
            "T7_Helm",
            "Helm of the Juggernaut",
            "Equipment",
            "Ability",
            "UT",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Abilities, ItemSubCategory::Helmets)
        );
    }

    #[test]
    fn test_seal() {
        let a = make_asset(
            35,
            "T7_Seal",
            "Seal of Blasphemous Prayer",
            "Equipment",
            "Ability",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Abilities, ItemSubCategory::Seals)
        );
    }

    #[test]
    fn test_spell() {
        let a = make_asset(
            36,
            "T7_Spell",
            "Spell of Destruction",
            "Equipment",
            "Ability",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Abilities, ItemSubCategory::Spells)
        );
    }

    #[test]
    fn test_skull() {
        let a = make_asset(
            37,
            "T7_Skull",
            "Skull of Endless Torment",
            "Equipment",
            "Ability",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Abilities, ItemSubCategory::Skulls)
        );
    }

    #[test]
    fn test_scepter() {
        let a = make_asset(
            38,
            "T7_Scepter",
            "Scepter of Storms",
            "Equipment",
            "Ability",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Abilities, ItemSubCategory::Scepters)
        );
    }

    #[test]
    fn test_poison() {
        let a = make_asset(
            39,
            "T7_Poison",
            "Murky Toxin Poison",
            "Equipment",
            "Ability",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Abilities, ItemSubCategory::Poisons)
        );
    }

    #[test]
    fn test_trap() {
        let a = make_asset(
            40,
            "T7_Trap",
            "Coral Venom Trap",
            "Equipment",
            "Ability",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Abilities, ItemSubCategory::Traps)
        );
    }

    #[test]
    fn test_orb() {
        let a = make_asset(
            41,
            "T7_Orb",
            "Orb of Conflict",
            "Equipment",
            "Ability",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Abilities, ItemSubCategory::Orbs)
        );
    }

    #[test]
    fn test_prism() {
        let a = make_asset(
            42,
            "T7_Prism",
            "Prism of Dancing Swords",
            "Equipment",
            "Ability",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Abilities, ItemSubCategory::Prisms)
        );
    }

    #[test]
    fn test_star() {
        let a = make_asset(
            43,
            "T7_Star",
            "Doom Circle Star",
            "Equipment",
            "Ability",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Abilities, ItemSubCategory::Stars)
        );
    }

    #[test]
    fn test_waki() {
        let a = make_asset(
            44,
            "T7_Waki",
            "Wakizashi of Crossed Eyes",
            "Equipment",
            "Ability",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Abilities, ItemSubCategory::Wakis)
        );
    }

    #[test]
    fn test_lute() {
        let a = make_asset(
            45,
            "T7_Lute",
            "Lute of Lullabies",
            "Equipment",
            "Ability",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Abilities, ItemSubCategory::Lutes)
        );
    }

    #[test]
    fn test_mace() {
        let a = make_asset(
            46,
            "T7_Mace",
            "Mace of the Celestial",
            "Equipment",
            "Ability",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Abilities, ItemSubCategory::Maces)
        );
    }

    #[test]
    fn test_sheath() {
        let a = make_asset(
            47,
            "T7_Sheath",
            "Sheath of the Demon Blade",
            "Equipment",
            "Ability",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Abilities, ItemSubCategory::Sheaths)
        );
    }

    // -- Accessories --

    #[test]
    fn test_ring() {
        let a = make_asset(
            50,
            "T7_Ring",
            "Ring of Decades",
            "Equipment",
            "Ring",
            "UT",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Accessories, ItemSubCategory::Rings)
        );
    }

    // -- Fishing rods (Accessories) --

    #[test]
    fn test_fishing_rod() {
        let a = make_asset(
            55,
            "FishingRod_MV",
            "MV Fishing Rod",
            "Equipment",
            "Misc",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Accessories, ItemSubCategory::MiscEquipment)
        );
    }

    // -- Consumables --

    #[test]
    fn test_key() {
        let a = make_asset(60, "DavyKey", "Davy Jones' Key", "Consumable", "", "", None);
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::Keys)
        );
    }

    #[test]
    fn test_dye() {
        let a = make_asset(61, "Dye_Red", "Red Dye", "Consumable", "", "", None);
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::DyesCloths)
        );
    }

    #[test]
    fn test_blueprint() {
        let a = make_asset(
            62,
            "Blueprint_Sword",
            "Sword Blueprint",
            "Consumable",
            "",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::Blueprints)
        );
    }

    #[test]
    fn test_unlocker() {
        let a = make_asset(
            63,
            "BackpackUnlocker",
            "Backpack Unlocker",
            "Consumable",
            "",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::Unlockers)
        );
    }

    #[test]
    fn test_booster() {
        let a = make_asset(
            64,
            "LootDropBooster",
            "Loot Drop Booster",
            "Consumable",
            "",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::Boosters)
        );
    }

    #[test]
    fn test_pet_egg() {
        let a = make_asset(
            65,
            "PetEgg_Rare",
            "Rare Pet Egg",
            "Consumable",
            "",
            "pet_egg",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::PetEggs)
        );
    }

    #[test]
    fn test_crate() {
        let a = make_asset(
            66,
            "MysteryBox",
            "Mystery Crate",
            "Consumable",
            "",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::Crates)
        );
    }

    #[test]
    fn test_ichor_helpful_consumable() {
        let a = make_asset(
            67,
            "IchorAttack",
            "Ichor of Attack",
            "Consumable",
            "",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (
                ItemCategory::Consumables,
                ItemSubCategory::HelpfulConsumables
            )
        );
    }

    #[test]
    fn test_candy() {
        // Stat candies have slot_type 10 (CONSUMABLE) and pattern "Candy of Extreme [Stat]"
        let a = make_asset(
            9729,
            "Candy of Extreme Attack",
            "Candy of Extreme Attack",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::Candies)
        );
    }

    // -- Utility Items --

    #[test]
    fn test_mark() {
        let a = make_asset(70, "Mark_Oryx", "Mark of Oryx", "Consumable", "", "", None);
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::UtilityItems, ItemSubCategory::Marks)
        );
    }

    #[test]
    fn test_ore() {
        // Ores are narrowed to the broad-use forge materials only.
        let a = make_asset(
            71,
            "Basic Ore",
            "Common Ore",
            "Equipment",
            "",
            "MATERIAL,FORGESTORE,UPGRADECORE,NO_DISMANTLE",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::UtilityItems, ItemSubCategory::Ores)
        );
    }

    #[test]
    fn test_nilshard_is_ore() {
        let a = make_asset(
            50413,
            "Common Nilshard x1",
            "",
            "Equipment",
            "",
            "NILSHARD,FORGESTORE",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::UtilityItems, ItemSubCategory::Ores)
        );
    }

    // -- Recategorizations --

    #[test]
    fn test_reskin_flail_by_label() {
        // Damnation shares the sword slot but is a Flail (by label).
        let a = make_asset(
            200,
            "Damnation",
            "Damnation",
            "Equipment",
            "Weapon",
            "EQUIPMENT,WEAPON,FLAIL,UT",
            Some(1),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Weapons, ItemSubCategory::Flails)
        );
    }

    #[test]
    fn test_beekeepers_flamethrower_is_morningstar() {
        // Labeled WAND but is a Morning Star.
        let a = make_asset(
            55381,
            "Beekeeper's Flamethrower",
            "",
            "Equipment",
            "",
            "EQUIPMENT,WEAPON,WAND,UT",
            Some(8),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Weapons, ItemSubCategory::MorningStars)
        );
    }

    #[test]
    fn test_sigil_by_label() {
        let a = make_asset(
            31611,
            "Draconic Insignia",
            "",
            "Equipment",
            "",
            "EQUIPMENT,ABILITY,SIGIL,UT",
            Some(31),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Abilities, ItemSubCategory::Sigils)
        );
    }

    #[test]
    fn test_petskin_class() {
        let a = make_asset(
            32796,
            "Tomb Snake Skin",
            "Tomb Snake",
            "PetSkin",
            "",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::PetSkins)
        );
    }

    #[test]
    fn test_technology_is_artifact() {
        let a = make_asset(
            56105,
            "Malogia Technology x1",
            "Malogia Technology",
            "Equipment",
            "",
            "ARTIFACT,FORGESTORE",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (
                ItemCategory::UtilityItems,
                ItemSubCategory::EnchantmentArtifacts
            )
        );
    }

    #[test]
    fn test_set_token_is_forge() {
        let a = make_asset(
            5371,
            "3RogueSTToken",
            "Spectral Rogue Set Token",
            "Equipment",
            "",
            "TAB_ST,FORGESTORE,UPGRADECORE",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Misc, ItemSubCategory::MiscEquipment)
        );
    }

    #[test]
    fn test_set_shard_is_forge() {
        let a = make_asset(
            53361,
            "3AssassinSTShard x1",
            "Alchemist Assassin Set Shard x1",
            "Equipment",
            "",
            "FORGESTORE,PETBLACKLIST",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Misc, ItemSubCategory::MiscEquipment)
        );
    }

    #[test]
    fn test_upgrade_core_is_forge() {
        let a = make_asset(
            49473,
            "Kogbold Enhancement Core",
            "",
            "Equipment",
            "",
            "MARK,FORGESTORE,UPGRADECORE",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Misc, ItemSubCategory::MiscEquipment)
        );
    }

    #[test]
    fn test_mystery_st_shard_is_shard() {
        let a = make_asset(
            8643,
            "Mystery ST Shard x1",
            "",
            "Equipment",
            "",
            "TAB_ST,FORGESTORE",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::UtilityItems, ItemSubCategory::EventTokens)
        );
    }

    #[test]
    fn test_mythical_st_shard_is_forge() {
        // Mythical ST Shard (idname 3STTokenShard) is the shard for the Mythical
        // ST Gem token, so it belongs with Forge Tokens.
        let a = make_asset(
            32227,
            "3STTokenShard x1",
            "Mythical ST Shard x1",
            "Equipment",
            "",
            "FORGESTORE",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Misc, ItemSubCategory::MiscEquipment)
        );
    }

    #[test]
    fn test_alien_essences_are_forge() {
        // All five alien-invasion essences carry the MARK label but are forge
        // upgrade materials, so they belong with Forge Tokens, not Marks.
        let a = make_asset(
            56191,
            "Malogia Essence",
            "Malogia Essence",
            "Equipment",
            "",
            "MARK,TAB_ALL,FORGESTORE,UPGRADECORE,MALOGIA_ESSENCE,ALIEN_ESSENCE",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Misc, ItemSubCategory::MiscEquipment)
        );
    }

    #[test]
    fn test_quest_token_not_forge() {
        // idname "QuestToken" contains the substring "sttoken"; ensure it is not
        // misrouted to Forge Tokens.
        let a = make_asset(
            9001,
            "QuestToken_Valentines",
            "Valentine Token",
            "Consumable",
            "",
            "event_token",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::UtilityItems, ItemSubCategory::EventTokens)
        );
    }

    #[test]
    fn test_gem_shard_is_shard() {
        let a = make_asset(
            16890,
            "Amethyst Shard x 1",
            "Amethyst Shard x1",
            "Equipment",
            "",
            "FORGESTORE",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::UtilityItems, ItemSubCategory::EventTokens)
        );
    }

    #[test]
    fn test_title_unlocker_by_label() {
        let a = make_asset(
            7419,
            "Choofer Title Unlocker",
            "",
            "Equipment",
            "",
            "TITLE,EQUIPMENT,CONSUMABLE,UT",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::Titles)
        );
    }

    #[test]
    fn test_sulphur_is_booster() {
        let a = make_asset(
            812,
            "Basic Sulphur",
            "",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT,FORGESTORE",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::Boosters)
        );
    }

    #[test]
    fn test_max_level_potion_is_booster() {
        let a = make_asset(
            6731,
            "Potion of Max Level 1",
            "Potion of Max Level",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::Boosters)
        );
    }

    #[test]
    fn test_candy_apple_is_pet_food() {
        let a = make_asset(
            25766,
            "Candy Apple",
            "",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::UtilityItems, ItemSubCategory::PetFood)
        );
    }

    #[test]
    fn test_chocolate_bar_cloth_is_dye() {
        // Cloths use class "Dye"; must not be caught by the pet-food "chocolate bar" match.
        let a = make_asset(
            4858,
            "Large Chocolate Bar Cloth",
            "{textiles.Large_Chocolate_Bar_Cloth}",
            "Dye",
            "",
            "",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::DyesCloths)
        );
    }

    #[test]
    fn test_chocolate_bar_is_pet_food() {
        // The actual "Chocolate Bar" (class Equipment) is still pet food.
        let a = make_asset(
            25770,
            "Chocolate Bar",
            "",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::UtilityItems, ItemSubCategory::PetFood)
        );
    }

    #[test]
    fn test_interregnum_lore_is_vanity() {
        let a = make_asset(
            37458,
            "InterregnumLore_4",
            "Vagrant's Journal",
            "Equipment",
            "",
            "MISC,TRADEABLE",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::UtilityItems, ItemSubCategory::VanityItems)
        );
    }

    #[test]
    fn test_interregnum_treasure_chest_is_crate() {
        let a = make_asset(
            38191,
            "Interregnum Quest Chest Item",
            "Interregnum Treasure Chest",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::Crates)
        );
    }

    #[test]
    fn test_snowman_part_is_event_special() {
        let a = make_asset(
            46066,
            "Arachnid Coal",
            "",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::EventSpecial)
        );
    }

    #[test]
    fn test_coalbearing_quiver_not_event_special() {
        // "coal" substring must not steal a real quiver into Event Special.
        let a = make_asset(
            8336,
            "Coalbearing Quiver",
            "",
            "Equipment",
            "Ability",
            "EQUIPMENT,ABILITY,QUIVER,UT",
            Some(15),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Abilities, ItemSubCategory::Quivers)
        );
    }

    #[test]
    fn test_pet_food() {
        let a = make_asset(
            72,
            "PetFood_Power",
            "Power Pet Food",
            "Consumable",
            "",
            "pet_food",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::UtilityItems, ItemSubCategory::PetFood)
        );
    }

    #[test]
    fn test_enchantment_artifact() {
        let a = make_asset(
            73,
            "TarotCard_Fool",
            "The Fool Tarot",
            "Consumable",
            "",
            "enchantment",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (
                ItemCategory::UtilityItems,
                ItemSubCategory::EnchantmentArtifacts
            )
        );
    }

    // -- Other --

    #[test]
    fn test_unknown_falls_to_other() {
        let a = make_asset(
            99,
            "SomethingWeird",
            "Something Weird",
            "Character",
            "NPC",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Other, ItemSubCategory::Uncategorized)
        );
    }

    #[test]
    fn test_weapon_name_fallback() {
        // Weapon with no projectile data falls back to name matching
        let a = make_asset(
            100,
            "SwordOfExample",
            "Sword of Example",
            "Equipment",
            "Weapon",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Weapons, ItemSubCategory::Swords)
        );
    }

    #[test]
    fn test_title() {
        let a = make_asset(
            80,
            "Title_Champion",
            "Champion Title",
            "Consumable",
            "",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::Titles)
        );
    }

    #[test]
    fn test_emote() {
        let a = make_asset(81, "Emote_Wave", "Wave Emote", "Consumable", "", "", None);
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::Emotes)
        );
    }

    #[test]
    fn test_token() {
        let a = make_asset(
            82,
            "EventToken_Valentines",
            "Valentine Token",
            "Consumable",
            "",
            "event_token",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::UtilityItems, ItemSubCategory::EventTokens)
        );
    }

    #[test]
    fn test_pet_skin() {
        let a = make_asset(
            83,
            "PetSkin_Dragon",
            "Dragon Pet Skin",
            "Consumable",
            "",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::PetSkins)
        );
    }

    #[test]
    fn test_vanity() {
        let a = make_asset(
            84,
            "Treasure_Gold",
            "Gold Treasure",
            "Consumable",
            "",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::UtilityItems, ItemSubCategory::VanityItems)
        );
    }

    #[test]
    fn test_greater_potion() {
        let a = make_asset(
            85,
            "GreaterPotionAttack",
            "Greater Potion of Attack",
            "Consumable",
            "",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::GreaterPotions)
        );
    }

    #[test]
    fn test_stat_potion() {
        let a = make_asset(
            86,
            "PotionAttack",
            "Potion of Attack",
            "Consumable",
            "",
            "",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::Potions)
        );
    }

    #[test]
    fn test_stat_potion_sb() {
        let a = make_asset(
            87,
            "PotionAttack_SB",
            "Potion of Attack (SB)",
            "Consumable",
            "",
            "soulbound",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::PotionsSB)
        );
    }

    #[test]
    fn test_from_object_list_lookup() {
        let _list = ObjectList::new();
        // Manually add items via iter (ObjectList doesn't have a public insert, but
        // we can test categorize directly which is the core logic)
        let asset = make_asset(
            1,
            "T14_Sword",
            "Sword of the Colossus",
            "Equipment",
            "Weapon",
            "",
            Some(1),
        );
        let (cat, sub) = ItemCategorizer::categorize(&asset);
        assert_eq!(cat, ItemCategory::Weapons);
        assert_eq!(sub, ItemSubCategory::Swords);
    }

    // -- Restoratives --

    #[test]
    fn test_restorative_by_tab_ut_label() {
        // Temp consumables carry TAB_UT; must land in Restoratives, not Other.
        let a = make_asset(
            200,
            "Snake Oil",
            "Snake Oil",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT,TAB_UT,TRADEABLE",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (
                ItemCategory::Consumables,
                ItemSubCategory::HelpfulConsumables
            )
        );
    }

    #[test]
    fn test_restorative_by_apply_stat_eff_label() {
        let a = make_asset(
            201,
            "Fairy Dust",
            "Fairy Dust",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT,APPLY_STAT_EFF",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (
                ItemCategory::Consumables,
                ItemSubCategory::HelpfulConsumables
            )
        );
    }

    #[test]
    fn test_restorative_wine_by_name() {
        // Tiered wines have no distinguishing label; matched by name.
        let a = make_asset(
            202,
            "Fire Water",
            "Fire Water",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,TIERED,LOOTABLE,T3,TRADEABLE",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (
                ItemCategory::Consumables,
                ItemSubCategory::HelpfulConsumables
            )
        );
    }

    #[test]
    fn test_restorative_fishing_food_by_name() {
        // Fishing foods carry no labels; matched by name.
        let a = make_asset(203, "Sushi Roll", "Sushi Roll", "Equipment", "", "", None);
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (
                ItemCategory::Consumables,
                ItemSubCategory::HelpfulConsumables
            )
        );
    }

    #[test]
    fn test_restorative_heal_potion_by_name() {
        let a = make_asset(
            204,
            "Health Potion",
            "Health Potion",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,TIERED,LOOTABLE,T1,TRADEABLE",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (
                ItemCategory::Consumables,
                ItemSubCategory::HelpfulConsumables
            )
        );
    }

    #[test]
    fn test_account_booster_stays_out_of_restoratives() {
        // XP/Loot boosters have only UT (no TAB_UT/APPLY_STAT_EFF) and use the
        // consumable slot_type (10); they must stay in Boosters, not Restoratives.
        let a = make_asset(
            205,
            "XP Booster",
            "XP Booster",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::Boosters)
        );
    }

    #[test]
    fn test_title_unlocker_not_restorative() {
        // TITLE rule must win over the restorative label rule.
        let a = make_asset(
            206,
            "MOTMG Title Unlocker",
            "MOTMG Title Unlocker",
            "Equipment",
            "",
            "TITLE,EQUIPMENT,CONSUMABLE,UT,TAB_UT,TRADEABLE",
            None,
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::Titles)
        );
    }

    #[test]
    fn test_restorative_halloween_bottle_by_name() {
        // Halloween Bottles have only CONSUMABLE,UT; matched by name incl. "Greater".
        let a = make_asset(
            207,
            "Greater Bat Bottle 1",
            "Greater Bat Bottle",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (
                ItemCategory::Consumables,
                ItemSubCategory::HelpfulConsumables
            )
        );
        let b = make_asset(
            208,
            "Mini Oryx Bottle",
            "Mini Oryx Bottle",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&b),
            (
                ItemCategory::Consumables,
                ItemSubCategory::HelpfulConsumables
            )
        );
    }

    #[test]
    fn test_restorative_halloween_treat_by_name() {
        let a = make_asset(
            209,
            "Beryl Treat",
            "Beryl Treat",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (
                ItemCategory::Consumables,
                ItemSubCategory::HelpfulConsumables
            )
        );
    }

    #[test]
    fn test_restorative_christmas_booster_by_name() {
        let a = make_asset(
            210,
            "Candy Cane",
            "Candy Cane",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (
                ItemCategory::Consumables,
                ItemSubCategory::HelpfulConsumables
            )
        );
        let b = make_asset(
            211,
            "Blue Ice Candy",
            "Blue Ice Candy",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&b),
            (
                ItemCategory::Consumables,
                ItemSubCategory::HelpfulConsumables
            )
        );
    }

    #[test]
    fn test_restorative_santas_reins_by_name() {
        // Reins buff stats but carry only COOLDOWN/MISC (no CONSUMABLE); by name.
        let a = make_asset(
            212,
            "Rudolph's Reins",
            "Rudolph's Reins",
            "Equipment",
            "",
            "COOLDOWN",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (
                ItemCategory::Consumables,
                ItemSubCategory::HelpfulConsumables
            )
        );
        let b = make_asset(
            213,
            "Dasher's Reins",
            "Dasher's Reins",
            "Equipment",
            "",
            "MISC,COOLDOWN",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&b),
            (
                ItemCategory::Consumables,
                ItemSubCategory::HelpfulConsumables
            )
        );
    }

    #[test]
    fn test_oryx_rune_is_key() {
        // Sword/Shield/Helmet Rune are dungeon keys; display name is empty so
        // name() falls back to id_name.
        let a = make_asset(
            214,
            "Sword Rune",
            "",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::Keys)
        );
        let b = make_asset(
            215,
            "Helmet Rune",
            "",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&b),
            (ItemCategory::Consumables, ItemSubCategory::Keys)
        );
    }

    #[test]
    fn test_snowman_accessory_is_event_special() {
        // Oryxmas Nexus snowman accessories share the "XN " id_name prefix.
        let a = make_asset(
            216,
            "XN SH",
            "Santa Hat",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::EventSpecial)
        );
        let b = make_asset(
            217,
            "XN S",
            "Tree branch",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&b),
            (ItemCategory::Consumables, ItemSubCategory::EventSpecial)
        );
        // Dev/test item with the prefix must be excluded.
        let t = make_asset(
            218,
            "XN Test Sn Base",
            "XN Test Sn Base",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT",
            Some(10),
        );
        assert_ne!(
            ItemCategorizer::categorize(&t),
            (ItemCategory::Consumables, ItemSubCategory::EventSpecial)
        );
    }

    #[test]
    fn test_jack_frost_items_are_event_special() {
        for id_name in [
            "Snowman Body",
            "Buccaneer Top Hat",
            "Mad God's Coal",
            "Jungle Carrot",
            "Frosty Carrot",
        ] {
            let a = make_asset(
                46075,
                id_name,
                "",
                "Equipment",
                "",
                "EQUIPMENT,CONSUMABLE,UT",
                Some(10),
            );
            assert_eq!(
                ItemCategorizer::categorize(&a),
                (ItemCategory::Consumables, ItemSubCategory::EventSpecial),
                "{id_name} should be Event Special"
            );
        }
        // Carrot Cake / Carrot Coin are NOT Jack Frost decorations.
        let cake = make_asset(
            14278,
            "Carrot Cake",
            "",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT",
            Some(10),
        );
        assert_ne!(
            ItemCategorizer::categorize(&cake),
            (ItemCategory::Consumables, ItemSubCategory::EventSpecial)
        );
    }

    #[test]
    fn test_gifts_are_crates() {
        for id_name in ["Gift of the Void", "Gift of the Daeva"] {
            let a = make_asset(
                14532,
                id_name,
                "",
                "Equipment",
                "",
                "EQUIPMENT,CONSUMABLE,UT",
                Some(10),
            );
            assert_eq!(
                ItemCategorizer::categorize(&a),
                (ItemCategory::Consumables, ItemSubCategory::Crates),
                "{id_name}"
            );
        }
        let urn = make_asset(
            3406,
            "Silver Urn",
            "",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&urn),
            (ItemCategory::Consumables, ItemSubCategory::Crates)
        );
    }

    #[test]
    fn test_dust_rewards_are_boosters() {
        for id_name in [
            "Green Dust Charm",
            "Red Dust Shard",
            "Purple Dust Essence",
            "Green Dust Gem",
            "Double Green Dust Gem Pack",
        ] {
            let a = make_asset(
                1114,
                id_name,
                "",
                "Equipment",
                "",
                "EQUIPMENT,CONSUMABLE,UT",
                Some(10),
            );
            assert_eq!(
                ItemCategorizer::categorize(&a),
                (ItemCategory::Consumables, ItemSubCategory::Boosters),
                "{id_name}"
            );
        }
    }

    #[test]
    fn test_pet_food_full_list_by_name() {
        // Representative items from across the realmeye pet-food list.
        for (idn, disp) in [
            ("Cheese x3", ""), // stackable: display empty, id_name has " xN"
            ("Grilled Honey Comb x1", ""),
            ("Vamp Steak Flambe' x5", ""),
            ("Hot Sauce", "Hot Sauce"),
            ("Xeno Heart", "Xeno Heart"),
            ("Gold Bar", "Gold Bar"),
            ("Chalice of Bloody Ambrosia", "Chalice of Bloody Ambrosia"),
            ("The Appetizer's Spicy Superburger", ""),
        ] {
            let a = make_asset(
                300,
                idn,
                disp,
                "Equipment",
                "",
                "EQUIPMENT,CONSUMABLE,UT",
                Some(10),
            );
            assert_eq!(
                ItemCategorizer::categorize(&a),
                (ItemCategory::UtilityItems, ItemSubCategory::PetFood),
                "expected pet food for {idn}"
            );
        }
    }

    #[test]
    fn test_pet_food_recipe_not_pet_food() {
        // Forge recipes share the food name but carry no CONSUMABLE label and
        // end in "Recipe"; they must not be classified as pet food.
        let a = make_asset(
            301,
            "Grilled Honey Comb Recipe",
            "",
            "Equipment",
            "",
            "",
            Some(10),
        );
        assert_ne!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::UtilityItems, ItemSubCategory::PetFood)
        );
    }

    #[test]
    fn test_pail_is_restorative() {
        let a = make_asset(
            302,
            "Sand Pail 5",
            "Sand Pail",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (
                ItemCategory::Consumables,
                ItemSubCategory::HelpfulConsumables
            )
        );
        // The "Ice Pail" summon is a Character with no labels; not a restorative.
        let c = make_asset(303, "Ice Pail", "", "Character", "", "", None);
        assert_ne!(
            ItemCategorizer::categorize(&c),
            (
                ItemCategory::Consumables,
                ItemSubCategory::HelpfulConsumables
            )
        );
        let d = make_asset(
            8264,
            "Ice Castle 5",
            "Ice Castle",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&d),
            (
                ItemCategory::Consumables,
                ItemSubCategory::HelpfulConsumables
            )
        );
    }

    #[test]
    fn test_seasonal_jumpstart_gift_is_stat_candy() {
        // Permanently boosts stats; belongs in Stat Candies. Display empty so
        // name() falls back to id_name.
        let a = make_asset(
            304,
            "Seasonal Jumpstart Gift",
            "",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&a),
            (ItemCategory::Consumables, ItemSubCategory::Candies)
        );
    }

    #[test]
    fn test_valentine_and_energy_signet_are_vanity() {
        let v = make_asset(
            5407,
            "Valentine Generator",
            "Valentine",
            "Equipment",
            "",
            "",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&v),
            (ItemCategory::UtilityItems, ItemSubCategory::VanityItems)
        );
        let s = make_asset(
            325,
            "Energy Signet",
            "",
            "Equipment",
            "",
            "EQUIPMENT,CONSUMABLE,UT",
            Some(10),
        );
        assert_eq!(
            ItemCategorizer::categorize(&s),
            (ItemCategory::UtilityItems, ItemSubCategory::VanityItems)
        );
    }

    #[test]
    fn test_gear_and_dust_chests_are_crates() {
        for id_name in [
            "Alien Weapon Cache",
            "Shards of Destiny",
            "Miner's Arsenal Chest",
            "Large Dust Chest",
            "Medium Dust Chest",
            "Aspirant Weapon Chest",
            "Aspirant Ring Chest",
            "Aspirant Armor Chest",
            "Oryxmas Ice Weapons Chest",
            "Oryxmas Ability Chest",
            "Easter Weapon Chest",
            "Advanced Ring Chest",
            "Expert Ring Chest",
            "Beginner Weapon",
            "Beginner Ability",
            "Beginner Armor",
            "Beginner Ring",
            "Intermediate Weapon",
            "Advanced Armor",
            "Expert Ring",
        ] {
            let a = make_asset(
                70000,
                id_name,
                "",
                "Equipment",
                "",
                "EQUIPMENT,CONSUMABLE,UT",
                Some(10),
            );
            assert_eq!(
                ItemCategorizer::categorize(&a),
                (ItemCategory::Consumables, ItemSubCategory::Crates),
                "{id_name} should be a Crate"
            );
        }
    }
}

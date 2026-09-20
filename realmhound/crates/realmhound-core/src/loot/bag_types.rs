//! Loot bag type definitions.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// Loot bag types with their object IDs.
///
/// Note: Only bags with ID >= 1287 are considered "loot drop" bags
/// (excludes Brown and Soulbound which are common drops).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum LootBagType {
    // Regular bags
    Brown = 1280,
    Soulbound = 1283,
    Pink = 1286,
    Purple = 1287,
    Egg = 1288,
    Teal = 1289,
    Blue = 1291,
    White = 1292,
    Gold = 1294,
    Orange = 1295,
    Red = 1708,
    // Boosted variants
    BoostedWhite = 1296,
    BoostedBrown = 1709,
    BoostedPink = 1710,
    BoostedPurple = 1722,
    BoostedEgg = 1723,
    BoostedGold = 1724,
    BoostedTeal = 1725,
    BoostedBlue = 1726,
    BoostedOrange = 1727,
    BoostedRed = 1728,
}

impl LootBagType {
    /// Get the object ID for this bag type.
    pub fn id(&self) -> i32 {
        *self as i32
    }

    /// Get the display name for this bag type.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Brown => "Brown",
            Self::Soulbound => "Soulbound",
            Self::Pink => "Pink",
            Self::Purple => "Purple",
            Self::Egg => "Egg Basket",
            Self::Teal => "Teal",
            Self::Blue => "Blue",
            Self::White => "White",
            Self::Gold => "Gold",
            Self::Orange => "Orange",
            Self::Red => "Red",
            Self::BoostedWhite => "B.White",
            Self::BoostedBrown => "B.Brown",
            Self::BoostedPink => "B.Pink",
            Self::BoostedPurple => "B.Purple",
            Self::BoostedEgg => "B.Egg",
            Self::BoostedGold => "B.Gold",
            Self::BoostedTeal => "B.Teal",
            Self::BoostedBlue => "B.Blue",
            Self::BoostedOrange => "B.Orange",
            Self::BoostedRed => "B.Red",
        }
    }

    /// Try to convert an object ID to a bag type.
    pub fn from_id(id: i32) -> Option<Self> {
        match id {
            1280 => Some(Self::Brown),
            1283 => Some(Self::Soulbound),
            1286 => Some(Self::Pink),
            1287 => Some(Self::Purple),
            1288 => Some(Self::Egg),
            1289 => Some(Self::Teal),
            1291 => Some(Self::Blue),
            1292 => Some(Self::White),
            1294 => Some(Self::Gold),
            1295 => Some(Self::Orange),
            1708 => Some(Self::Red),
            1296 => Some(Self::BoostedWhite),
            1709 => Some(Self::BoostedBrown),
            1710 => Some(Self::BoostedPink),
            1722 => Some(Self::BoostedPurple),
            1723 => Some(Self::BoostedEgg),
            1724 => Some(Self::BoostedGold),
            1725 => Some(Self::BoostedTeal),
            1726 => Some(Self::BoostedBlue),
            1727 => Some(Self::BoostedOrange),
            1728 => Some(Self::BoostedRed),
            _ => None,
        }
    }

    /// Check if this is a "notable" loot drop bag (Purple or better).
    /// Excludes Brown, Soulbound, and Pink which are common drops.
    #[deprecated(note = "Use is_always_tracked() or is_conditional_bag() instead")]
    pub fn is_notable(&self) -> bool {
        self.id() >= 1287
    }

    /// Check if this bag is always tracked regardless of enchantments.
    /// These are high-tier bags: White, Orange, Red, Gold, Blue.
    pub fn is_always_tracked(&self) -> bool {
        matches!(
            self,
            Self::White
                | Self::BoostedWhite
                | Self::Orange
                | Self::BoostedOrange
                | Self::Red
                | Self::BoostedRed
                | Self::Gold
                | Self::BoostedGold
                | Self::Blue
                | Self::BoostedBlue
        )
    }

    /// Check if this bag is only tracked when it has valuable enchantments.
    /// These are lower-tier bags: Brown, Soulbound, Pink, Purple, Egg, Teal.
    pub fn is_conditional_bag(&self) -> bool {
        matches!(
            self,
            Self::Brown
                | Self::BoostedBrown
                | Self::Soulbound
                | Self::Pink
                | Self::BoostedPink
                | Self::Purple
                | Self::BoostedPurple
                | Self::Egg
                | Self::BoostedEgg
                | Self::Teal
                | Self::BoostedTeal
        )
    }

    /// Check if this is a white bag (regular or boosted).
    pub fn is_white(&self) -> bool {
        matches!(self, Self::White | Self::BoostedWhite)
    }

    /// Check if this is an orange bag (regular or boosted).
    pub fn is_orange(&self) -> bool {
        matches!(self, Self::Orange | Self::BoostedOrange)
    }

    /// Check if this is a red bag (regular or boosted).
    pub fn is_red(&self) -> bool {
        matches!(self, Self::Red | Self::BoostedRed)
    }

    /// Check if this is a gold bag (regular or boosted).
    pub fn is_gold(&self) -> bool {
        matches!(self, Self::Gold | Self::BoostedGold)
    }

    /// Check if this is an egg basket bag (regular or boosted).
    pub fn is_egg(&self) -> bool {
        matches!(self, Self::Egg | Self::BoostedEgg)
    }

    /// Check if this is a blue bag (regular or boosted).
    pub fn is_blue(&self) -> bool {
        matches!(self, Self::Blue | Self::BoostedBlue)
    }

    /// Check if this is a teal bag (regular or boosted).
    pub fn is_teal(&self) -> bool {
        matches!(self, Self::Teal | Self::BoostedTeal)
    }

    /// Check if this is a purple bag (regular or boosted).
    pub fn is_purple(&self) -> bool {
        matches!(self, Self::Purple | Self::BoostedPurple)
    }

    /// Check if this is a pink bag (regular or boosted).
    pub fn is_pink(&self) -> bool {
        matches!(self, Self::Pink | Self::BoostedPink)
    }

    /// Check if this is a brown bag (regular or boosted).
    pub fn is_brown(&self) -> bool {
        matches!(self, Self::Brown | Self::BoostedBrown)
    }

    /// Check if this is a boosted variant.
    pub fn is_boosted(&self) -> bool {
        matches!(
            self,
            Self::BoostedWhite
                | Self::BoostedBrown
                | Self::BoostedPink
                | Self::BoostedPurple
                | Self::BoostedEgg
                | Self::BoostedGold
                | Self::BoostedTeal
                | Self::BoostedBlue
                | Self::BoostedOrange
                | Self::BoostedRed
        )
    }

    /// Rarity rank for sorting (higher = rarer). Boosted variants share rank.
    pub fn rarity_rank(&self) -> u8 {
        match self {
            Self::Brown | Self::BoostedBrown => 0,
            Self::Soulbound => 1,
            Self::Pink | Self::BoostedPink => 2,
            Self::Purple | Self::BoostedPurple => 3,
            Self::Egg | Self::BoostedEgg => 4,
            Self::Teal | Self::BoostedTeal => 5,
            Self::Blue | Self::BoostedBlue => 6,
            Self::Gold | Self::BoostedGold => 7,
            Self::White | Self::BoostedWhite => 8,
            Self::Orange | Self::BoostedOrange => 9,
            Self::Red | Self::BoostedRed => 10,
        }
    }
}

/// Set of bag IDs considered "loot drop" bags (ID >= 1287).
static LOOT_DROP_BAGS: LazyLock<HashSet<i32>> = LazyLock::new(|| {
    let mut set = HashSet::new();
    for bag in ALL_BAG_TYPES.iter() {
        if bag.id() >= 1287 {
            set.insert(bag.id());
        }
    }
    set
});

/// Map of bag ID to display name.
static BAG_NAMES: LazyLock<HashMap<i32, &'static str>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    for bag in ALL_BAG_TYPES.iter() {
        map.insert(bag.id(), bag.name());
    }
    map
});

/// All bag types for iteration.
const ALL_BAG_TYPES: [LootBagType; 21] = [
    LootBagType::Brown,
    LootBagType::Soulbound,
    LootBagType::Pink,
    LootBagType::Purple,
    LootBagType::Egg,
    LootBagType::Teal,
    LootBagType::Blue,
    LootBagType::White,
    LootBagType::Gold,
    LootBagType::Orange,
    LootBagType::Red,
    LootBagType::BoostedWhite,
    LootBagType::BoostedBrown,
    LootBagType::BoostedPink,
    LootBagType::BoostedPurple,
    LootBagType::BoostedEgg,
    LootBagType::BoostedGold,
    LootBagType::BoostedTeal,
    LootBagType::BoostedBlue,
    LootBagType::BoostedOrange,
    LootBagType::BoostedRed,
];

/// Check if an object type ID is a "loot drop" bag (Purple or better).
///
/// Note: Returns false for Brown, Soulbound, and Pink bags.
pub fn is_loot_drop_bag(object_type: i32) -> bool {
    LOOT_DROP_BAGS.contains(&object_type)
}

/// Check if an object type ID is any kind of loot bag.
pub fn is_any_loot_bag(object_type: i32) -> bool {
    LootBagType::from_id(object_type).is_some()
}

/// Get the display name for a bag type ID.
pub fn loot_bag_name(object_type: i32) -> Option<&'static str> {
    BAG_NAMES.get(&object_type).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bag_ids() {
        assert_eq!(LootBagType::White.id(), 1292);
        assert_eq!(LootBagType::Orange.id(), 1295);
        assert_eq!(LootBagType::Red.id(), 1708);
        assert_eq!(LootBagType::BoostedWhite.id(), 1296);
    }

    #[test]
    fn test_is_loot_drop_bag() {
        // Notable bags (Purple+) should be loot drops
        assert!(is_loot_drop_bag(1287)); // Purple
        assert!(is_loot_drop_bag(1292)); // White
        assert!(is_loot_drop_bag(1295)); // Orange
        assert!(is_loot_drop_bag(1708)); // Red

        // Common bags should NOT be loot drops
        assert!(!is_loot_drop_bag(1280)); // Brown
        assert!(!is_loot_drop_bag(1283)); // Soulbound
        assert!(!is_loot_drop_bag(1286)); // Pink
    }

    #[test]
    fn test_bag_names() {
        assert_eq!(loot_bag_name(1292), Some("White"));
        assert_eq!(loot_bag_name(1296), Some("B.White"));
        assert_eq!(loot_bag_name(9999), None);
    }

    #[test]
    fn test_from_id() {
        assert_eq!(LootBagType::from_id(1292), Some(LootBagType::White));
        assert_eq!(LootBagType::from_id(1296), Some(LootBagType::BoostedWhite));
        assert_eq!(LootBagType::from_id(9999), None);
    }

    #[test]
    fn test_bag_type_checks() {
        assert!(LootBagType::White.is_white());
        assert!(LootBagType::BoostedWhite.is_white());
        assert!(!LootBagType::Orange.is_white());

        assert!(LootBagType::BoostedWhite.is_boosted());
        assert!(!LootBagType::White.is_boosted());
    }
}

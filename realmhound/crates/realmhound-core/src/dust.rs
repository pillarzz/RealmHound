//! Forge dust tracking for seasonal and regular accounts.
//!
//! Dust is earned by defeating dungeon bosses and used at the Forge
//! to enchant equipment. There are three types:
//! - Green (common) - index 1
//! - Red (rare) - index 5
//! - Purple (legendary) - index 4
//!
//! Each type has a maximum capacity of 2000.

use serde::{Deserialize, Serialize};

/// The three types of forge dust.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DustType {
    /// Green dust (common) - stat index 1
    Green,
    /// Red dust (rare) - stat index 5
    Red,
    /// Purple dust (legendary) - stat index 4
    Purple,
}

impl DustType {
    /// Maximum capacity for any dust type.
    pub const MAX_CAPACITY: i32 = 2000;

    /// Object ID for the dust sprite (for UI rendering).
    pub fn object_id(self) -> i32 {
        match self {
            DustType::Green => 13870,
            DustType::Red => 13871,
            DustType::Purple => 13872,
        }
    }

    /// Display name for the dust type.
    pub fn name(self) -> &'static str {
        match self {
            DustType::Green => "Green",
            DustType::Red => "Red",
            DustType::Purple => "Purple",
        }
    }

    /// Get DustType from the stat string index.
    /// Returns None for unknown indices.
    pub fn from_index(index: u8) -> Option<Self> {
        match index {
            1 => Some(DustType::Green),
            5 => Some(DustType::Red),
            4 => Some(DustType::Purple),
            _ => None,
        }
    }
}

/// Amounts for each dust type (current and max).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DustAmounts {
    /// Green dust: (current, max)
    pub green: (i32, i32),
    /// Red dust: (current, max)
    pub red: (i32, i32),
    /// Purple dust: (current, max)
    pub purple: (i32, i32),
}

impl DustAmounts {
    /// Create new DustAmounts with all zeros.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the amount for a specific dust type.
    pub fn get(&self, dust_type: DustType) -> (i32, i32) {
        match dust_type {
            DustType::Green => self.green,
            DustType::Red => self.red,
            DustType::Purple => self.purple,
        }
    }

    /// Set the amount for a specific dust type.
    pub fn set(&mut self, dust_type: DustType, current: i32, max: i32) {
        match dust_type {
            DustType::Green => self.green = (current, max),
            DustType::Red => self.red = (current, max),
            DustType::Purple => self.purple = (current, max),
        }
    }

    /// Check if any dust type is at capacity (100%).
    pub fn any_full(&self) -> bool {
        self.is_full(self.green) || self.is_full(self.red) || self.is_full(self.purple)
    }

    /// Dust types that are at capacity (100%), in Green/Red/Purple order.
    pub fn full_types(&self) -> Vec<DustType> {
        [DustType::Green, DustType::Red, DustType::Purple]
            .into_iter()
            .filter(|&dt| self.is_full(self.get(dt)))
            .collect()
    }

    /// Check if any dust type is at warning threshold (90%+) but not full.
    pub fn any_warning(&self) -> bool {
        !self.any_full()
            && (self.is_warning(self.green)
                || self.is_warning(self.red)
                || self.is_warning(self.purple))
    }

    /// Check if a dust amount is full (100%).
    fn is_full(&self, (current, max): (i32, i32)) -> bool {
        max > 0 && current >= max
    }

    /// Check if a dust amount is at warning threshold (90%+).
    fn is_warning(&self, (current, max): (i32, i32)) -> bool {
        max > 0 && current >= (max * 9 / 10)
    }
}

/// Combined dust state for both seasonal and regular accounts.
#[derive(Debug, Clone, Default)]
pub struct DustState {
    /// Regular (non-seasonal) account dust.
    pub regular: Option<DustAmounts>,
    /// Seasonal account dust.
    pub seasonal: Option<DustAmounts>,
}

impl DustState {
    /// Create new empty DustState.
    pub fn new() -> Self {
        Self::default()
    }

    /// Update dust amounts for the specified account type.
    pub fn update(&mut self, amounts: DustAmounts, is_seasonal: bool) {
        if is_seasonal {
            self.seasonal = Some(amounts);
        } else {
            self.regular = Some(amounts);
        }
    }

    /// Get dust amounts for the specified account type.
    pub fn get(&self, is_seasonal: bool) -> Option<&DustAmounts> {
        if is_seasonal {
            self.seasonal.as_ref()
        } else {
            self.regular.as_ref()
        }
    }
}

/// Parse dust stat string into amounts.
///
/// Format: comma-separated `index:value` pairs, e.g., `"1:150,4:200,5:75"`
/// - Index 1 = Green
/// - Index 4 = Purple
/// - Index 5 = Red
///
/// Returns None if parsing fails.
pub fn parse_dust_stat(amounts_str: &str, caps_str: &str) -> Option<DustAmounts> {
    let mut result = DustAmounts::new();

    // Parse current amounts
    for part in amounts_str.split(',') {
        let mut split = part.split(':');
        let index: u8 = split.next()?.parse().ok()?;
        let value: i32 = split.next()?.parse().ok()?;

        if let Some(dust_type) = DustType::from_index(index) {
            let (_, max) = result.get(dust_type);
            result.set(dust_type, value, max);
        }
    }

    // Parse max capacities
    for part in caps_str.split(',') {
        let mut split = part.split(':');
        let index: u8 = split.next()?.parse().ok()?;
        let value: i32 = split.next()?.parse().ok()?;

        if let Some(dust_type) = DustType::from_index(index) {
            let (current, _) = result.get(dust_type);
            result.set(dust_type, current, value);
        }
    }

    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_dust_stat() {
        let amounts = "1:1935,4:1990,5:1800";
        let caps = "1:2000,4:2000,5:2000";

        let result = parse_dust_stat(amounts, caps).unwrap();

        assert_eq!(result.green, (1935, 2000));
        assert_eq!(result.purple, (1990, 2000));
        assert_eq!(result.red, (1800, 2000));
    }

    #[test]
    fn test_dust_type_from_index() {
        assert_eq!(DustType::from_index(1), Some(DustType::Green));
        assert_eq!(DustType::from_index(5), Some(DustType::Red));
        assert_eq!(DustType::from_index(4), Some(DustType::Purple));
        assert_eq!(DustType::from_index(0), None);
        assert_eq!(DustType::from_index(2), None);
        assert_eq!(DustType::from_index(3), None);
    }

    #[test]
    fn test_any_full() {
        let mut amounts = DustAmounts::new();
        amounts.green = (1500, 2000);
        amounts.red = (2000, 2000);
        amounts.purple = (500, 2000);

        assert!(amounts.any_full());

        amounts.red = (1999, 2000);
        assert!(!amounts.any_full());
    }
}

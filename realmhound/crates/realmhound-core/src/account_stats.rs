//! Account-level values read live from the local player's object stats.
//!
//! These back the configurable Widget Bar. Account-wide values
//! (stars, account fame, gold) are shared across the seasonal and regular
//! accounts; forge fire and forge materials are per-account, keyed like dust by
//! `is_seasonal`.

use serde::{Deserialize, Serialize};

/// The four forge-material tiers, in ascending rarity (also the order of their
/// Material stat-string indices).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ForgeMaterialTier {
    Common,
    Rare,
    Legendary,
    Mythical,
}

impl ForgeMaterialTier {
    /// All tiers, ascending rarity (Common -> Mythical).
    pub const ALL: [ForgeMaterialTier; 4] = [
        ForgeMaterialTier::Common,
        ForgeMaterialTier::Rare,
        ForgeMaterialTier::Legendary,
        ForgeMaterialTier::Mythical,
    ];

    /// Map a Material stat-string index to a tier (1=Common .. 4=Mythical).
    pub fn from_index(index: u8) -> Option<Self> {
        match index {
            1 => Some(ForgeMaterialTier::Common),
            2 => Some(ForgeMaterialTier::Rare),
            3 => Some(ForgeMaterialTier::Legendary),
            4 => Some(ForgeMaterialTier::Mythical),
            _ => None,
        }
    }

    /// Array slot used for per-tier storage (Common=0 .. Mythical=3).
    pub fn slot(self) -> usize {
        match self {
            ForgeMaterialTier::Common => 0,
            ForgeMaterialTier::Rare => 1,
            ForgeMaterialTier::Legendary => 2,
            ForgeMaterialTier::Mythical => 3,
        }
    }

    /// Display name (e.g. "Mythical").
    pub fn name(self) -> &'static str {
        match self {
            ForgeMaterialTier::Common => "Common",
            ForgeMaterialTier::Rare => "Rare",
            ForgeMaterialTier::Legendary => "Legendary",
            ForgeMaterialTier::Mythical => "Mythical",
        }
    }
}

/// Per-tier forge-material amounts, each `(current, cap)`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterialAmounts {
    /// Common material (current, cap).
    #[serde(default)]
    pub common: (i32, i32),
    /// Rare material (current, cap).
    #[serde(default)]
    pub rare: (i32, i32),
    /// Legendary material (current, cap).
    #[serde(default)]
    pub legendary: (i32, i32),
    /// Mythical material (current, cap).
    #[serde(default)]
    pub mythical: (i32, i32),
}

impl MaterialAmounts {
    /// Amount `(current, cap)` for a tier.
    pub fn get(&self, tier: ForgeMaterialTier) -> (i32, i32) {
        match tier {
            ForgeMaterialTier::Common => self.common,
            ForgeMaterialTier::Rare => self.rare,
            ForgeMaterialTier::Legendary => self.legendary,
            ForgeMaterialTier::Mythical => self.mythical,
        }
    }

    /// Set the `(current, cap)` for a tier.
    pub fn set(&mut self, tier: ForgeMaterialTier, current: i32, cap: i32) {
        match tier {
            ForgeMaterialTier::Common => self.common = (current, cap),
            ForgeMaterialTier::Rare => self.rare = (current, cap),
            ForgeMaterialTier::Legendary => self.legendary = (current, cap),
            ForgeMaterialTier::Mythical => self.mythical = (current, cap),
        }
    }
}

/// Cached account-level stats, merged from repeated [`AccountStatsUpdate`]s.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountStats {
    /// Account star count (drives the star-rank widget + color).
    #[serde(default)]
    pub stars: Option<i32>,
    /// Current spendable account fame.
    #[serde(default)]
    pub account_fame: Option<i32>,
    /// Account gold (Realm gold / credits).
    #[serde(default)]
    pub gold: Option<i32>,
    /// Forge fire energy for the regular account.
    #[serde(default)]
    pub forge_fire_regular: Option<i32>,
    /// Forge fire energy for the seasonal account.
    #[serde(default)]
    pub forge_fire_seasonal: Option<i32>,
    /// Forge materials (per-tier current/cap) for the regular account.
    #[serde(default)]
    pub forge_materials_regular: Option<MaterialAmounts>,
    /// Forge materials (per-tier current/cap) for the seasonal account.
    #[serde(default)]
    pub forge_materials_seasonal: Option<MaterialAmounts>,
}

impl AccountStats {
    /// Forge fire for the account matching `is_seasonal`.
    pub fn forge_fire(&self, is_seasonal: bool) -> Option<i32> {
        if is_seasonal {
            self.forge_fire_seasonal
        } else {
            self.forge_fire_regular
        }
    }

    /// Forge materials (per-tier current/cap) for the account matching `is_seasonal`.
    pub fn materials(&self, is_seasonal: bool) -> Option<&MaterialAmounts> {
        if is_seasonal {
            self.forge_materials_seasonal.as_ref()
        } else {
            self.forge_materials_regular.as_ref()
        }
    }

    /// Merge a single live update into this cache. Returns `true` if any stored
    /// value changed (so callers can decide whether to persist / bump).
    pub fn merge(&mut self, update: &AccountStatsUpdate) -> bool {
        let mut changed = false;

        macro_rules! set_wide {
            ($field:ident, $val:expr) => {
                if let Some(v) = $val {
                    if self.$field != Some(v) {
                        self.$field = Some(v);
                        changed = true;
                    }
                }
            };
        }

        set_wide!(stars, update.stars);
        set_wide!(account_fame, update.account_fame);
        set_wide!(gold, update.gold);

        if let Some(v) = update.forge_fire {
            let slot = if update.is_seasonal {
                &mut self.forge_fire_seasonal
            } else {
                &mut self.forge_fire_regular
            };
            if *slot != Some(v) {
                *slot = Some(v);
                changed = true;
            }
        }

        // Materials arrive as two separate stats (current, cap), each a
        // per-tier `index:value` string. NewTick is delta-based, so a changed
        // current can arrive without the unchanged cap (and per-tier deltas may
        // omit unchanged tiers); merge each tier/component independently so a
        // partial update doesn't clobber the rest.
        if update.has_material() {
            let slot = if update.is_seasonal {
                &mut self.forge_materials_seasonal
            } else {
                &mut self.forge_materials_regular
            };
            let mut amounts = slot.clone().unwrap_or_default();
            for tier in ForgeMaterialTier::ALL {
                let i = tier.slot();
                let (mut cur, mut cap) = amounts.get(tier);
                if let Some(c) = update.material_current[i] {
                    cur = c;
                }
                if let Some(m) = update.material_cap[i] {
                    cap = m;
                }
                amounts.set(tier, cur, cap);
            }
            let new_val = Some(amounts);
            if *slot != new_val {
                *slot = new_val;
                changed = true;
            }
        }

        changed
    }
}

/// A single batch of account-level values extracted from one player-object
/// status. Only the fields actually present in the packet are `Some`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountStatsUpdate {
    /// Whether the reporting character is seasonal (routes forge fire / materials).
    pub is_seasonal: bool,
    pub stars: Option<i32>,
    pub account_fame: Option<i32>,
    pub gold: Option<i32>,
    pub forge_fire: Option<i32>,
    /// Per-tier current forge-material counts, indexed by
    /// [`ForgeMaterialTier::slot`] (Common..Mythical). A delta tick may set
    /// only some tiers, and only the current or only the cap.
    pub material_current: [Option<i32>; 4],
    /// Per-tier forge-material caps, indexed like `material_current`.
    pub material_cap: [Option<i32>; 4],
}

impl AccountStatsUpdate {
    /// True when no field was populated (nothing to emit).
    pub fn is_empty(&self) -> bool {
        self.stars.is_none()
            && self.account_fame.is_none()
            && self.gold.is_none()
            && self.forge_fire.is_none()
            && !self.has_material()
    }

    /// True when any per-tier material current or cap value is present.
    pub fn has_material(&self) -> bool {
        self.material_current
            .iter()
            .chain(self.material_cap.iter())
            .any(Option::is_some)
    }
}

/// Parse a forge-material stat string of comma-separated `index:value` pairs
/// (same wire format as dust) into per-tier values, indexed by
/// [`ForgeMaterialTier::slot`]. Tiers absent from the string stay `None`.
pub fn parse_material_stat(s: &str) -> [Option<i32>; 4] {
    let mut out = [None; 4];
    for part in s.split(',') {
        let mut it = part.split(':');
        let idx = it.next().and_then(|v| v.trim().parse::<u8>().ok());
        let val = it.next().and_then(|v| v.trim().parse::<i32>().ok());
        if let (Some(idx), Some(val)) = (idx, val) {
            if let Some(tier) = ForgeMaterialTier::from_index(idx) {
                out[tier.slot()] = Some(val);
            }
        }
    }
    out
}

/// RotMG star tiers: (minimum star count, RGB color). Highest tier first.
/// Colors sampled from the in-game star rank badges.
const STAR_TIERS: [(i32, [u8; 3]); 6] = [
    (95, [240, 240, 240]), // white / max (95)
    (76, [255, 255, 0]),   // yellow (76-94)
    (57, [246, 146, 29]),  // orange (57-75)
    (38, [192, 38, 44]),   // red (38-56)
    (19, [48, 76, 218]),   // blue (19-37)
    (0, [137, 151, 221]),  // light blue (0-18)
];

/// Colour (RGB) for a given star count, matching the in-game rank badge tiers.
pub fn star_color(stars: i32) -> [u8; 3] {
    for (min, color) in STAR_TIERS {
        if stars >= min {
            return color;
        }
    }
    STAR_TIERS[STAR_TIERS.len() - 1].1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_material_stat_indexes_tiers() {
        let out = parse_material_stat("1:10,2:5,3:0,4:99");
        assert_eq!(out[ForgeMaterialTier::Common.slot()], Some(10));
        assert_eq!(out[ForgeMaterialTier::Rare.slot()], Some(5));
        assert_eq!(out[ForgeMaterialTier::Legendary.slot()], Some(0));
        assert_eq!(out[ForgeMaterialTier::Mythical.slot()], Some(99));
        assert_eq!(parse_material_stat(""), [None; 4]);
        assert_eq!(parse_material_stat("garbage"), [None; 4]);
    }

    #[test]
    fn merge_reports_changes_and_routes_seasonal() {
        let mut stats = AccountStats::default();
        let mut u = AccountStatsUpdate {
            is_seasonal: true,
            stars: Some(42),
            forge_fire: Some(500),
            ..Default::default()
        };
        assert!(stats.merge(&u));
        assert_eq!(stats.stars, Some(42));
        assert_eq!(stats.forge_fire_seasonal, Some(500));
        assert_eq!(stats.forge_fire_regular, None);
        // Re-merging identical values reports no change.
        assert!(!stats.merge(&u));
        // A regular update lands in the regular slot.
        u.is_seasonal = false;
        u.stars = None;
        u.forge_fire = Some(600);
        assert!(stats.merge(&u));
        assert_eq!(stats.forge_fire_regular, Some(600));
    }

    #[test]
    fn merge_materials_partial_deltas() {
        let common = ForgeMaterialTier::Common.slot();
        let mythical = ForgeMaterialTier::Mythical.slot();
        let mut stats = AccountStats::default();

        // First a full pair (current + cap) for Common.
        let mut cur = [None; 4];
        let mut cap = [None; 4];
        cur[common] = Some(100);
        cap[common] = Some(500);
        let full = AccountStatsUpdate {
            is_seasonal: false,
            material_current: cur,
            material_cap: cap,
            ..Default::default()
        };
        assert!(stats.merge(&full));
        assert_eq!(stats.materials(false).unwrap().common, (100, 500));

        // A delta tick carrying only the changed current must keep the cap.
        let mut cur2 = [None; 4];
        cur2[common] = Some(150);
        let delta = AccountStatsUpdate {
            is_seasonal: false,
            material_current: cur2,
            ..Default::default()
        };
        assert!(stats.merge(&delta));
        assert_eq!(stats.materials(false).unwrap().common, (150, 500));

        // A delta carrying only the cap keeps the current.
        let mut cap3 = [None; 4];
        cap3[common] = Some(600);
        let cap_delta = AccountStatsUpdate {
            is_seasonal: false,
            material_cap: cap3,
            ..Default::default()
        };
        assert!(stats.merge(&cap_delta));
        assert_eq!(stats.materials(false).unwrap().common, (150, 600));

        // A delta for a different tier leaves Common untouched.
        let mut cur4 = [None; 4];
        let mut cap4 = [None; 4];
        cur4[mythical] = Some(7);
        cap4[mythical] = Some(2000);
        let myth = AccountStatsUpdate {
            is_seasonal: false,
            material_current: cur4,
            material_cap: cap4,
            ..Default::default()
        };
        assert!(stats.merge(&myth));
        let m = stats.materials(false).unwrap();
        assert_eq!(m.common, (150, 600));
        assert_eq!(m.mythical, (7, 2000));
    }

    #[test]
    fn star_color_tiers() {
        assert_eq!(star_color(95), [240, 240, 240]); // white / max
        assert_eq!(star_color(80), [255, 255, 0]); // yellow (76-94)
        assert_eq!(star_color(50), [192, 38, 44]); // red (38-56)
        assert_eq!(star_color(19), [48, 76, 218]); // blue (19-37, boundary)
        assert_eq!(star_color(0), [137, 151, 221]); // light blue (0-18)
    }
}

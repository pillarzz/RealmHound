//! Dungeon modifier reward-bonus lookup.
//!
//! RotMG's `MapInfo` packet carries the active dungeon modifiers as text id
//! tokens (e.g. `WEAKBOSS_3` for "Weak Boss III"). The packet does NOT include
//! the reward bonuses those modifiers grant.
//!
//! [`modifier_info`] resolves a token's display name and XP / Loot / Dust
//! bonuses from the authoritative `mods.xml` (see [`crate::assets::ModifierTable`]).
//! The asset manager loads `mods.xml` from the installed game's locally
//! extracted assets, which update automatically after game patches.
//!
//! Tokens are matched case-insensitively and ignoring any non-alphanumeric
//! characters, so the wire token `WEAKBOSS_3` matches the table id `WEAKBOSS_3`.

/// Reward bonuses granted by a dungeon modifier, in percent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModifierBonus {
    /// XP gain bonus (%).
    pub xp: i32,
    /// Loot drop bonus (%).
    pub loot: i32,
    /// Dust gain bonus (%).
    pub dust: i32,
}

/// Resolved information about a single dungeon modifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModifierInfo {
    /// Human-readable display name (e.g. "Weak Boss III").
    pub display_name: String,
    /// Reward bonuses granted by the modifier.
    pub bonus: ModifierBonus,
    /// Comma-separated label list from `mods.xml` (e.g. "ROLLABLE,ANY,REWARD").
    pub labels: String,
}

/// In-game dungeon modifier category, identified by a unique outline color and a
/// leading icon:
/// - [`ModType::Reward`]: gold outline, gift icon (has `REWARD` label).
/// - [`ModType::Unique`]: silver outline, diamond icon (has `UNIQUE` label).
/// - [`ModType::Normal`]: gray outline, no icon (generic `ANY`/`ANY_ELITE` stat mod).
/// - [`ModType::Special`]: holographic outline, spark icon (dungeon-specific).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModType {
    /// Reward mod (gold, gift icon).
    Reward,
    /// Unique mod (silver, diamond icon).
    Unique,
    /// Dungeon-specific special mod (holographic, spark icon).
    Special,
    /// Normal stat mod (gray, no icon).
    Normal,
}

/// Text-color category for a modifier name in the Live Feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModDanger {
    /// Negative DEF / pet-disable / more enemy damage / tankier enemies / dangerous
    /// dungeon-specific.
    Red,
    /// Other negative or trade-off player stats.
    Orange,
    /// Reward mods, Dimitus, and beneficial dungeon-specific mods.
    Golden,
    /// Unique enemy/portal spawners and remaining dungeon-specific mods.
    Purple,
    /// Significant player buffs / healing / other player benefits.
    Blue,
    /// Default (no notable effect).
    White,
}

/// Outline / background fill category for a whole dungeon callout,
/// resolved by priority: Dimitus -> dangerous -> rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutlineKind {
    /// Dimitus present.
    Golden,
    /// A dangerous modifier is present.
    Red,
    /// Default.
    Blue,
}

/// Normalize a modifier token to its canonical lookup key: uppercase ASCII
/// alphanumerics only (drops `_`, spaces, punctuation).
pub fn canonical(token: &str) -> String {
    token
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_uppercase())
        .collect()
}

/// Canonical bases of the dungeon modifiers that carry a tier-specific icon:
/// the 20 boss/minion mods plus the 17 player-stat mods.
pub const TIER_ICON_BASES: &[&str] = &[
    // Boss & Minions.
    "ELITEBOSS",
    "ELITEMINIONS",
    "FEEBLEBOSS",
    "FEEBLEMINIONS",
    "FEROCIOUSBOSS",
    "FEROCIOUSMINIONS",
    "GLASSBOSS",
    "GLASSMINIONS",
    "HEALTHYBOSS",
    "HEALTHYMINIONS",
    "RESILIENTBOSS",
    "RESILIENTMINIONS",
    "STONEBOSS",
    "STONEMINIONS",
    "TAMEBOSS",
    "TAMEMINIONS",
    "TOUGHBOSS",
    "TOUGHMINIONS",
    "WEAKBOSS",
    "WEAKMINIONS",
    // Player stat mods.
    "ANEMIA",
    "APATHETIC",
    "ARMORED",
    "CLUMSY",
    "DEXTEROUS",
    "DRAINED",
    "ENERGETIC",
    "EXPOSED",
    "FOOLISH",
    "FRENZIED",
    "LIVELY",
    "SLUGGISH",
    "SPEEDY",
    "STRENGTHENED",
    "VIGOROUS",
    "WEAK",
    "WISE",
];

/// Resolve a tier-icon modifier token to its canonical base and tier.
///
/// Tokens carry the tier as a trailing `_<n>` (e.g. `WEAKBOSS_3`). Returns
/// `Some((base, tier))` only for the known tier-icon bases with a tier in
/// 1..=4, so callers can pick the matching tier-specific icon.
pub fn tier_icon_base_tier(token: &str) -> Option<(String, u8)> {
    let (prefix, suffix) = token.rsplit_once('_')?;
    let tier: u8 = suffix.parse().ok()?;
    if !(1..=4).contains(&tier) {
        return None;
    }
    let base = canonical(prefix);
    if TIER_ICON_BASES.contains(&base.as_str()) {
        Some((base, tier))
    } else {
        None
    }
}

/// Look up the display name and reward bonuses for a modifier token.
///
/// Data comes from the `mods.xml` extracted from the installed game.
///
/// Returns `None` if the token is not a known modifier.
pub fn modifier_info(token: &str) -> Option<ModifierInfo> {
    if canonical(token).is_empty() {
        return None;
    }

    crate::assets::get_asset_manager()
        .modifier_def(token)
        .map(|def| ModifierInfo {
            display_name: def.display_name,
            bonus: ModifierBonus {
                xp: def.xp,
                loot: def.loot,
                dust: def.dust,
            },
            labels: def.labels,
        })
}

/// Human-readable display name for a modifier token, if known.
pub fn modifier_display_name(token: &str) -> Option<String> {
    modifier_info(token).map(|i| i.display_name)
}

/// Sum the loot bonus (%) granted by a set of modifier tokens. Unknown tokens
/// contribute 0.
pub fn total_loot_bonus(tokens: &[String]) -> i32 {
    tokens
        .iter()
        .filter_map(|t| modifier_info(t))
        .map(|i| i.bonus.loot)
        .sum()
}

/// Sum the XP, loot, and dust bonuses (%) granted by a set of modifier tokens.
/// Unknown tokens contribute 0.
pub fn total_bonus(tokens: &[String]) -> ModifierBonus {
    tokens.iter().filter_map(|t| modifier_info(t)).fold(
        ModifierBonus {
            xp: 0,
            loot: 0,
            dust: 0,
        },
        |acc, i| ModifierBonus {
            xp: acc.xp + i.bonus.xp,
            loot: acc.loot + i.bonus.loot,
            dust: acc.dust + i.bonus.dust,
        },
    )
}

/// Returns true if any token resolves to the modifier with the given canonical
/// key (e.g. `"DIMITUS"`).
pub fn contains_modifier(tokens: &[String], canonical_key: &str) -> bool {
    tokens.iter().any(|t| canonical(t) == canonical_key)
}

/// True if `labels` (a comma-separated list from `mods.xml`) contains `label` as
/// an exact, case-insensitive token.
fn has_label(labels: &str, label: &str) -> bool {
    labels
        .split(',')
        .any(|l| l.trim().eq_ignore_ascii_case(label))
}

/// Split a modifier display name into its canonical base (uppercase alphanumerics,
/// roman-numeral tier removed) and its tier (1-4, or 0 when untiered).
///
/// e.g. `"Glass Boss III"` -> (`"GLASSBOSS"`, 3), `"Berserk"` -> (`"BERSERK"`, 0).
fn base_and_tier(display_name: &str) -> (String, u8) {
    let trimmed = display_name.trim();
    let tier = match trimmed.rsplit(' ').next() {
        Some("IV") => 4,
        Some("III") => 3,
        Some("II") => 2,
        Some("I") => 1,
        _ => 0,
    };
    let base = if tier > 0 {
        // Drop the trailing roman-numeral word.
        match trimmed.rfind(' ') {
            Some(idx) => &trimmed[..idx],
            None => trimmed,
        }
    } else {
        trimmed
    };
    (canonical(base), tier)
}

/// The in-game mod type for a token, derived from `mods.xml` labels.
/// Returns `None` for unknown tokens.
pub fn mod_type(token: &str) -> Option<ModType> {
    let info = modifier_info(token)?;
    Some(if has_label(&info.labels, "REWARD") {
        ModType::Reward
    } else if has_label(&info.labels, "UNIQUE") {
        ModType::Unique
    } else if has_label(&info.labels, "ANY") || has_label(&info.labels, "ANY_ELITE") {
        ModType::Normal
    } else {
        ModType::Special
    })
}

/// Canonical base names whose modifiers color the name RED (all tiers).
const RED_BASES: &[&str] = &[
    "EXPOSED",
    "ARMORBROKEN",
    "SPONGY",
    "SICKPET",
    "SILENTPET",
    "ANEMIA",
    "GLASSBOSS",
    "GLASSMINIONS",
    "FEROCIOUSBOSS",
    "FEROCIOUSMINIONS",
    "ELITEBOSS",
    "ELITEMINIONS",
    "LETHAL",
    "STONEMINIONS",
    "STONEBOSS",
    "HAUNTEDHALLS",
    "CRABRAVE",
    "OUTOFTIME",
];

/// Canonical base names whose modifiers color the name ORANGE (all tiers).
const ORANGE_BASES: &[&str] = &[
    "SLUGGISH",
    "CLUMSY",
    "WEAK",
    "FOOLISH",
    "APATHETIC",
    "BERSERK",
    "MANABATTERY",
    "DRAINED",
    "FRENZIED",
];

/// Canonical base names colored GOLDEN beyond the `REWARD` label.
const GOLDEN_BASES: &[&str] = &["DIMITUS", "LANTERNLIGHT", "MOONLITFISHING", "SOULBOOST"];

/// Canonical base names colored BLUE at every tier.
const BLUE_BASES_ALL: &[&str] = &["SURVIVOR", "TAMEMINIONS", "TAMEBOSS"];

/// Canonical base names colored BLUE only at tier III-IV.
const BLUE_BASES_HIGH: &[&str] = &[
    "VIGOROUS",
    "SPEEDY",
    "ARMORED",
    "DEXTEROUS",
    "STRENGTHENED",
    "WISE",
    "LIVELY",
    "ENERGETIC",
];

/// Canonical base names colored PURPLE (unique enemy/portal spawners).
const PURPLE_BASES: &[&str] = &[
    "SYNDICATETAKEOVER",
    "PRISMIMIC",
    "WANDERER",
    "NOBLEBOSS",
    "ALIENWORMHOLES",
    "SPIRITSNAKESWARM",
    "SPIDERSWARMING",
];

/// Canonical base names that drive a RED whole-callout outline. Narrower than
/// [`RED_BASES`]: excludes Anemia and Stone (tanky/heal-reduction).
const OUTLINE_RED_BASES: &[&str] = &[
    "EXPOSED",
    "ARMORBROKEN",
    "SPONGY",
    "SICKPET",
    "SILENTPET",
    "GLASSBOSS",
    "GLASSMINIONS",
    "FEROCIOUSBOSS",
    "FEROCIOUSMINIONS",
    "ELITEBOSS",
    "ELITEMINIONS",
    "LETHAL",
    "HAUNTEDHALLS",
    "CRABRAVE",
    "OUTOFTIME",
];

/// The text-color category for a single modifier token. Unknown
/// tokens (and anything not otherwise classified) are [`ModDanger::White`].
pub fn mod_danger(token: &str) -> ModDanger {
    let Some(info) = modifier_info(token) else {
        return ModDanger::White;
    };
    let (base, tier) = base_and_tier(&info.display_name);
    let base = base.as_str();

    if has_label(&info.labels, "REWARD") || GOLDEN_BASES.contains(&base) {
        return ModDanger::Golden;
    }
    if RED_BASES.contains(&base) {
        return ModDanger::Red;
    }
    if ORANGE_BASES.contains(&base) {
        return ModDanger::Orange;
    }
    if BLUE_BASES_ALL.contains(&base) || (BLUE_BASES_HIGH.contains(&base) && tier >= 3) {
        return ModDanger::Blue;
    }
    if PURPLE_BASES.contains(&base) {
        return ModDanger::Purple;
    }
    // Remaining dungeon-specific (Special-type) mods default to purple.
    if mod_type(token) == Some(ModType::Special) {
        return ModDanger::Purple;
    }
    ModDanger::White
}

/// Human-readable names of the modifiers that give a dungeon a red
/// (dangerous) whole-callout outline, in the same order as
/// [`OUTLINE_RED_BASES`]. Used for the "Dangerous mods" alert tooltip.
pub const DANGEROUS_MODIFIER_NAMES: &[&str] = &[
    "Exposed",
    "Armor Broken",
    "Spongy",
    "Sick Pet",
    "Silent Pet",
    "Glass Boss",
    "Glass Minions",
    "Ferocious Boss",
    "Ferocious Minions",
    "Elite Boss",
    "Elite Minions",
    "Lethal",
    "Haunted Halls",
    "Crab Rave",
    "Out of Time",
];

/// The outline / background category for a whole dungeon callout,
/// by priority: Dimitus -> any dangerous mod -> default blue.
pub fn outline_kind(tokens: &[String]) -> OutlineKind {
    if contains_modifier(tokens, "DIMITUS") {
        return OutlineKind::Golden;
    }
    let any_dangerous = tokens.iter().any(|t| {
        modifier_info(t)
            .map(|info| {
                let (base, _) = base_and_tier(&info.display_name);
                OUTLINE_RED_BASES.contains(&base.as_str())
            })
            .unwrap_or(false)
    });
    if any_dangerous {
        OutlineKind::Red
    } else {
        OutlineKind::Blue
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_strips_separators_and_uppercases() {
        assert_eq!(canonical("WEAKBOSS_3"), "WEAKBOSS3");
        assert_eq!(canonical("pet collector"), "PETCOLLECTOR");
        assert_eq!(canonical("Found Treasure!"), "FOUNDTREASURE");
        assert_eq!(canonical(""), "");
    }

    #[test]
    fn unknown_token_contributes_zero() {
        assert!(modifier_info("NOT_A_REAL_MOD").is_none());
    }

    #[test]
    fn contains_modifier_matches_canonical_token() {
        let tokens = vec!["DIMITUS".to_string(), "WEAKBOSS_3".to_string()];
        assert!(contains_modifier(&tokens, "DIMITUS"));
        assert!(contains_modifier(&tokens, "WEAKBOSS3"));
        assert!(!contains_modifier(&tokens, "LOOTING"));
    }

    #[test]
    fn base_and_tier_strips_roman_numerals() {
        assert_eq!(
            base_and_tier("Glass Boss III"),
            ("GLASSBOSS".to_string(), 3)
        );
        assert_eq!(base_and_tier("Exposed IV"), ("EXPOSED".to_string(), 4));
        assert_eq!(base_and_tier("Berserk"), ("BERSERK".to_string(), 0));
        assert_eq!(base_and_tier("Weak I"), ("WEAK".to_string(), 1));
    }

    #[test]
    fn tier_icon_base_tier_resolves_known_tokens() {
        assert_eq!(
            tier_icon_base_tier("WEAKBOSS_3"),
            Some(("WEAKBOSS".to_string(), 3))
        );
        assert_eq!(
            tier_icon_base_tier("GLASSMINIONS_1"),
            Some(("GLASSMINIONS".to_string(), 1))
        );
        // Player stat mod (distinct from the boss/minion WEAK* bases).
        assert_eq!(tier_icon_base_tier("WEAK_2"), Some(("WEAK".to_string(), 2)));
        assert_eq!(tier_icon_base_tier("LOOTING"), None);
        // Out-of-range / missing tier.
        assert_eq!(tier_icon_base_tier("WEAKBOSS_0"), None);
        assert_eq!(tier_icon_base_tier("WEAKBOSS_5"), None);
        assert_eq!(tier_icon_base_tier("WEAKBOSS"), None);
    }
}

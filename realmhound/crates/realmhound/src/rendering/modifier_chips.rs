//! Shared dungeon-modifier chip data and reward icons.
//!
//! Both Live Feed and Combat History render the modifiers carried by a map's
//! `MapInfo`. Keeping display-name, type, danger and icon resolution in one
//! place stops the two surfaces from disagreeing.

use realmhound_core::dungeon_modifiers::{
    canonical, mod_danger, mod_type, modifier_display_name, ModDanger, ModType,
};

use super::{EmbeddedIcon, ModTierIcon};

/// Loot reward icon (white bag object type).
pub const WHITE_BAG_OBJECT_ID: i32 = 1292;

/// Dimitus boss object id, used as a distinct icon for Dimitus dungeons.
pub const DIMITUS_OBJECT_ID: i32 = 8813;

/// XP Booster object id, used as the XP-bonus symbol.
pub const XP_BOOSTER_OBJECT_ID: i32 = 3179;

/// A single modifier's display data: its name, in-game type (icon), danger
/// category and tier icon.
#[derive(Debug, Clone)]
pub struct ModChip {
    /// Raw wire token (e.g. "CHEF").
    pub token: String,
    /// Display name (e.g. "Weak Boss III").
    pub name: String,
    /// In-game mod type, drives the leading type icon.
    pub mod_type: ModType,
    /// Danger category, drives the name text color in Live Feed.
    pub danger: ModDanger,
    /// Whether this is the Dimitus modifier (shows its portrait, not a glyph).
    pub is_dimitus: bool,
    /// Tier-specific modifier icon, if this is one of the boss/minion or player
    /// mods. Drawn as the leading icon in place of the (absent) Normal type icon.
    pub modifier_icon: Option<ModTierIcon>,
}

/// Resolve modifier tokens into display chips, preserving the wire order.
pub fn chips_from_tokens(tokens: &[String]) -> Vec<ModChip> {
    tokens
        .iter()
        .map(|token| ModChip {
            token: token.clone(),
            name: modifier_display_name(token).unwrap_or_else(|| prettify_modifier(token)),
            mod_type: mod_type(token).unwrap_or(ModType::Normal),
            danger: mod_danger(token),
            is_dimitus: canonical(token) == "DIMITUS",
            modifier_icon: ModTierIcon::from_token(token),
        })
        .collect()
}

/// Convert a raw modifier id token (e.g. "SOUVENIR_1") into a display string
/// (e.g. "Souvenir 1") by splitting on underscores and title-casing each word.
pub fn prettify_modifier(token: &str) -> String {
    title_case_words(&token.replace('_', " "))
}

/// Leading glyph for a modifier's in-game type: gift for Reward, diamond for
/// Unique, spark for Special, none for Normal. Used as a fallback when the
/// embedded type icon fails to load.
pub fn mod_type_glyph(mod_type: ModType) -> Option<&'static str> {
    match mod_type {
        ModType::Reward => Some("🎁"),
        ModType::Unique => Some("💎"),
        ModType::Special => Some("✨"),
        ModType::Normal => None,
    }
}

/// Embedded type icon for a modifier's in-game type. `None` for Normal mods.
pub fn mod_type_icon(mod_type: ModType) -> Option<EmbeddedIcon> {
    match mod_type {
        ModType::Reward => Some(EmbeddedIcon::ModTypeReward),
        ModType::Unique => Some(EmbeddedIcon::ModTypeUnique),
        ModType::Special => Some(EmbeddedIcon::ModTypeSpecial),
        ModType::Normal => None,
    }
}

/// Title-case each whitespace-separated word in `input`.
fn title_case_words(input: &str) -> String {
    input
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

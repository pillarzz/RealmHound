//! Event/boss clipboard call short names and upcoming-dungeon hints.
//!
//! The Live Feed always shows full encounter names. These tables are used only
//! to build the *clipboard* callout text, controlled by the event name style
//! ([`DungeonNameStyle`](realmhound_core::settings::DungeonNameStyle)) and the
//! "add upcoming dungeons" toggle:
//! - Short name: the event's short community name (e.g. `rav rot`), or a user
//!   override.
//! - Full name: the encounter's full display name.
//! - Add upcoming: append the dungeon it is about to drop (e.g.
//!   `rav rot, halls soon`).
//!
//! Matching is done on a normalized key (lowercase ASCII alphanumerics only), so
//! punctuation/spacing/case differences in the encounter name don't matter.

use realmhound_core::settings::DungeonNameStyle;
use std::collections::BTreeMap;

/// Short community names for event/boss encounters, keyed by the encounter's
/// display name.
#[rustfmt::skip]
const EVENT_SHORT_NAMES: &[(&str, &str)] = &[
    ("Flying Behemoth",                "behemoth"),
    ("Avatar Of The Forgotten King",   "avatar"),
    ("Ethereal Shrine",                "MV shrine"),
    ("Ravenous Rot",                   "rav rot"),
    ("Lost Sentry",                    "sentry"),
    ("Bloodroot Heart",                "bloodroot"),
    ("Sentient Monolith",              "monolith"),
    ("Adult Baneserpent",              "baneserp"),
    ("Ancient Kaiju",                  "kaiju"),
    ("Dwarf Miner",                    "miner"),
    ("Kogbold Expedition Engine",      "train"),
    ("Aerial Warship",                 "warship"),
    ("Skeletal Centipede",             "centipede"),
    ("Killer Bee Nest",                "bees"),
    ("Bramblethorn",                   "bramble"),
    ("Corrupted Bramblethorn",         "bramble"),
    ("Lord of the Lost Lands",         "lotll"),
    ("Commander Calbrik",              "calbrik"),
    ("UFO",                            "UFO"),
    ("Mammoth Rat",                    "rat boss"),
    ("Mysterious Crystal",             "cry"),
    ("Grand Sphinx",                   "sphinx"),
    ("Jade and Garnet Statues",        "statues"),
    ("Legion General",                 "general"),
    ("Maze Minotaur",                  "minotaur"),
    ("Goblin Patriarch",               "goblin"),
    ("Rock Dragon",                    "rock dragon"),
    ("World's Oyster",                 "oyster"),
    ("Hermit God",                     "herm"),
    ("Ghost Ship",                     "gship"),
    ("Bilgewater's Galleon",           "galleon"),
    ("Eye of the Storm",               "eye of the storm"),
    ("Beer God",                       "beer"),
    ("Crab Sovereign",                 "crab"),
    ("Skull Knight",                   "skull knight"),
    ("Sigma Werewolf",                 "sigma"),
    ("Possessed Pumpkin",              "pumpkin"),
    ("Skull Shrine",                   "skull shrine"),
    ("Well of Souls",                  "well"),
    ("The Plague Doctor",              "doctor"),
    ("The Lich King",                  "lich king"),
    ("Pentaract",                      "pent"),
    ("Daughter of Limon",              "limon"),
    ("Cube God",                       "cube"),
    ("Astral Rift",                    "rift"),
    ("The Gardener",                   "gardener"),
    ("The Keyper",                     "keyper"),
    ("The Skeyper",                    "keyper"),
    ("Keyper of the Arena",            "keyper"),
    ("The Festival Overseer",          "keyper"),
    ("The Sweeper",                    "keyper"),
    ("The Appetizer",                  "appetizer"),
    ("Biff the Buffed Bunny",          "biff"),
    ("Totalia the Malevolent",         "totalia"),
    ("Bonegrind the Undead Butcher",   "zombie horde"),
    ("Permafrost Lord",                "permafrost"),
    ("Snowy the Frost God",            "snowy"),
    ("Jack Frost",                     "jack"),
    ("Jotunn",                         "jotunn"),
    ("Oryx Horde",                     "oryx horde"),
];

/// Upcoming-dungeon hint suffix for encounters that drop dungeon portals, keyed
/// by the encounter's display name. Only the trailing hint is stored (e.g.
/// `shatts soon`); it is composed as `<name>, <hint>` so it works with any short
/// name, full name, or user override.
#[rustfmt::skip]
const EVENT_UPCOMING: &[(&str, &str)] = &[
    ("Flying Behemoth",                "shatts soon"),
    ("Avatar Of The Forgotten King",   "shatts soon"),
    ("Ravenous Rot",                   "halls soon"),
    ("Lost Sentry",                    "halls soon"),
    ("Bloodroot Heart",                "halls soon"),
    ("Sentient Monolith",              "fungal soon"),
    ("Adult Baneserpent",              "fungal soon"),
    ("Ancient Kaiju",                  "fungal soon"),
    ("Dwarf Miner",                    "fungal soon"),
    ("Kogbold Expedition Engine",      "kog soon"),
    ("Aerial Warship",                 "kog soon"),
    ("Skeletal Centipede",             "spen soon"),
    ("Killer Bee Nest",                "nest soon"),
    ("Bramblethorn",                   "nest soon"),
    ("Lord of the Lost Lands",         "citadel soon"),
    ("Grand Sphinx",                   "tomb soon"),
    ("Jade and Garnet Statues",        "MT soon"),
    ("Legion General",                 "tomb soon"),
    ("Maze Minotaur",                  "encore soon"),
    ("Rock Dragon",                    "LOD soon"),
    ("World's Oyster",                 "reef soon"),
    ("Hermit God",                     "OT soon"),
    ("Ghost Ship",                     "davy soon"),
    ("Bilgewater's Galleon",           "ddocks soon"),
    ("Eye of the Storm",               "OT soon"),
    ("Beer God",                       "tavern soon"),
    ("Crab Sovereign",                 "ddocks soon"),
    ("Skull Knight",                   "MT soon"),
    ("Sigma Werewolf",                 "tavern soon"),
    ("Possessed Pumpkin",              "wlab soon"),
    ("Well of Souls",                  "davy soon"),
    ("The Plague Doctor",              "sulf soon"),
    ("The Lich King",                  "tomb soon"),
    ("Daughter of Limon",              "encore soon"),
    ("Cube God",                       "3D soon"),
    ("Astral Rift",                    "3D soon"),
    ("The Gardener",                   "bella soon"),
    ("Biff the Buffed Bunny",          "QBC soon"),
    ("Permafrost Lord",                "ice tomb soon"),
    ("Oryx Horde",                     "mayhem soon"),
];

/// Normalize an encounter name for table matching: lowercase ASCII
/// alphanumerics only (drops apostrophes, spaces, punctuation).
fn normalize_event_key(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Resolve an encounter display name to its short community name, if known.
pub fn event_short_name(display_name: &str) -> Option<&'static str> {
    let key = normalize_event_key(display_name);
    if key.is_empty() {
        return None;
    }
    EVENT_SHORT_NAMES
        .iter()
        .find(|(name, _)| normalize_event_key(name) == key)
        .map(|&(_, short)| short)
}

/// Resolve an encounter display name to its upcoming-dungeon hint suffix (e.g.
/// `halls soon`), if it drops a dungeon portal.
pub fn event_upcoming_hint(display_name: &str) -> Option<&'static str> {
    let key = normalize_event_key(display_name);
    if key.is_empty() {
        return None;
    }
    EVENT_UPCOMING
        .iter()
        .find(|(name, _)| normalize_event_key(name) == key)
        .map(|&(_, hint)| hint)
}

/// Resolve the event's callout name part, honoring a user short-name override
/// (Short mode only), then the curated short name, then the full display name.
fn resolve_event_name(
    display_name: &str,
    name_style: DungeonNameStyle,
    overrides: &BTreeMap<String, String>,
) -> String {
    if matches!(name_style, DungeonNameStyle::Short) {
        if let Some(custom) = overrides.get(display_name.trim()) {
            if !custom.trim().is_empty() {
                return custom.trim().to_string();
            }
        }
    }
    match name_style {
        DungeonNameStyle::Full => display_name.trim().to_string(),
        DungeonNameStyle::Short => event_short_name(display_name)
            .map(|s| s.to_string())
            .unwrap_or_else(|| display_name.trim().to_string()),
    }
}

/// Build the bare clipboard callout body (no `/p`, server, or `j`) for an event
/// encounter, honoring the event name style, upcoming toggle, and user
/// overrides. Falls back to the full display name when the encounter has no
/// curated short name.
pub fn event_call_body(
    display_name: &str,
    name_style: DungeonNameStyle,
    add_upcoming: bool,
    overrides: &BTreeMap<String, String>,
) -> String {
    let mut body = resolve_event_name(display_name, name_style, overrides);
    if add_upcoming {
        if let Some(hint) = event_upcoming_hint(display_name) {
            body.push_str(", ");
            body.push_str(hint);
        }
    }
    body
}

/// Parse an Alien Invasion wave encounter name into `(is_veteran, wave_number)`.
/// Returns `None` for non-alien-wave names.
pub fn parse_alien_wave(display_name: &str) -> Option<(bool, u32)> {
    let name = display_name.trim();
    let veteran = if name.starts_with("Alien Invasion Veteran") {
        true
    } else if name.starts_with("Alien Invasion Adept") {
        false
    } else {
        return None;
    };
    let wave = name.rsplit("Wave").next()?.trim().parse::<u32>().ok()?;
    Some((veteran, wave))
}

/// Build the clipboard callout body for an Alien Invasion wave using the
/// user-editable prefixes. The wave number is appended (`adept wave 4`); on
/// wave 4 with `add_wave4_boss`, the incoming boss hint is appended too
/// (`adept wave 4 UFO soon` / `veteran wave 4 calbrik soon`).
pub fn alien_wave_call_body(
    veteran: bool,
    wave: u32,
    adept_prefix: &str,
    veteran_prefix: &str,
    add_wave4_boss: bool,
) -> String {
    let prefix = if veteran {
        veteran_prefix
    } else {
        adept_prefix
    }
    .trim();
    let mut body = if prefix.is_empty() {
        format!("wave {wave}")
    } else {
        format!("{prefix} {wave}")
    };
    if add_wave4_boss && wave == 4 {
        body.push(' ');
        body.push_str(if veteran { "calbrik soon" } else { "UFO soon" });
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_overrides() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    #[test]
    fn short_name_lookup_is_normalized() {
        assert_eq!(event_short_name("Ravenous Rot"), Some("rav rot"));
        // Punctuation/case differences must not matter.
        assert_eq!(event_short_name("world's oyster"), Some("oyster"));
        assert_eq!(event_short_name("Mysterious Crystal"), Some("cry"));
    }

    #[test]
    fn keyper_aliases_all_resolve() {
        for name in [
            "The Keyper",
            "The Skeyper",
            "Keyper of the Arena",
            "The Festival Overseer",
            "The Sweeper",
        ] {
            assert_eq!(event_short_name(name), Some("keyper"), "{name}");
        }
    }

    #[test]
    fn alien_invasion_waves_build_from_prefixes() {
        assert_eq!(
            parse_alien_wave("Alien Invasion Adept - Wave 1"),
            Some((false, 1))
        );
        assert_eq!(
            parse_alien_wave("Alien Invasion Veteran - Wave 4"),
            Some((true, 4))
        );
        assert_eq!(parse_alien_wave("Ravenous Rot"), None);
        assert_eq!(
            alien_wave_call_body(false, 2, "adept wave", "veteran wave", true),
            "adept wave 2"
        );
        assert_eq!(
            alien_wave_call_body(false, 4, "adept wave", "veteran wave", true),
            "adept wave 4 UFO soon"
        );
        assert_eq!(
            alien_wave_call_body(true, 4, "adept wave", "veteran wave", true),
            "veteran wave 4 calbrik soon"
        );
        assert_eq!(
            alien_wave_call_body(true, 4, "adept wave", "veteran wave", false),
            "veteran wave 4"
        );
    }

    #[test]
    fn upcoming_composes_name_and_hint() {
        assert_eq!(
            event_call_body(
                "Ravenous Rot",
                DungeonNameStyle::Short,
                true,
                &no_overrides()
            ),
            "rav rot, halls soon"
        );
        // Short base + suffix composes even when they differ.
        assert_eq!(
            event_call_body(
                "Well of Souls",
                DungeonNameStyle::Short,
                false,
                &no_overrides()
            ),
            "well"
        );
        assert_eq!(
            event_call_body(
                "Well of Souls",
                DungeonNameStyle::Short,
                true,
                &no_overrides()
            ),
            "well, davy soon"
        );
    }

    #[test]
    fn full_name_style_uses_display_name() {
        assert_eq!(
            event_call_body(
                "Ravenous Rot",
                DungeonNameStyle::Full,
                false,
                &no_overrides()
            ),
            "Ravenous Rot"
        );
        // Upcoming hint still composes with the full name.
        assert_eq!(
            event_call_body(
                "Ravenous Rot",
                DungeonNameStyle::Full,
                true,
                &no_overrides()
            ),
            "Ravenous Rot, halls soon"
        );
    }

    #[test]
    fn override_takes_precedence_in_short_mode() {
        let mut overrides = BTreeMap::new();
        overrides.insert("Ravenous Rot".to_string(), "rot".to_string());
        assert_eq!(
            event_call_body("Ravenous Rot", DungeonNameStyle::Short, false, &overrides),
            "rot"
        );
        // Overrides are ignored in Full mode.
        assert_eq!(
            event_call_body("Ravenous Rot", DungeonNameStyle::Full, false, &overrides),
            "Ravenous Rot"
        );
    }

    #[test]
    fn upcoming_toggle_off_omits_hint_when_no_dungeon() {
        // Mysterious Crystal has a short name but no upcoming dungeon.
        assert_eq!(
            event_call_body(
                "Mysterious Crystal",
                DungeonNameStyle::Short,
                true,
                &no_overrides()
            ),
            "cry"
        );
    }

    #[test]
    fn unknown_event_falls_back_to_full_name() {
        assert_eq!(
            event_call_body(
                "Some Unknown Boss",
                DungeonNameStyle::Short,
                false,
                &no_overrides()
            ),
            "Some Unknown Boss"
        );
    }
}

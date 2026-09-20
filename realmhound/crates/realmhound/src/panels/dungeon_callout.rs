//! Dungeon-entry clipboard callouts.
//!
//! When the user loads into a dungeon, RealmHound builds a short callout *body*
//! to copy to the clipboard, e.g. `snake 15% lb synd`. The body is the dungeon's
//! short nickname followed by reward/modifier tags. The `/p` prefix, short
//! server name, and join marker `j` are added at copy time by the Live Feed
//! panel, so they are NOT part of the body built here.
//!
//! Tag rules: the total loot/dust/xp bonuses render as `<n>% lb`/`<n>% db`/
//! `<n>% xp` (labels and percent sign are configurable), followed by the
//! enabled reward-mod tags in list order (see [`default_reward_mods`], which
//! includes Syndicate, Prismimic, Crab Rave and other popular unique mods), then
//! Alexander's Legacy (`dying thessal <pct>%`), and finally Dimitus.
//!
//! By default, dungeons carrying the Dimitus modifier get NO callout because its
//! reward-mod entry ships disabled (they still surface via their own icon and
//! sound). Enabling the Dimitus entry appends a trailing `dimitus` tag instead.
//!
//! Some dungeons are reachable only from inside other instances (Crystal Cavern,
//! Cultist Hideout, The Void) and are deliberately given no callout because they
//! cannot be joined from the nexus.
//!
//! Modifier identification keys come from the authoritative `mods.xml` wire ids
//! (e.g. Syndicate = `MERCA..D`, Wanderer = `WANDERERBOSS`, Found Treasure =
//! `FOOUNDTREASURE`), matched via [`canonical`] so separators/case are ignored.
//! [`default_reward_mods`]: realmhound_core::settings::default_reward_mods

use realmhound_core::dungeon_modifiers::{canonical, total_bonus};
use realmhound_core::settings::{DungeonNameStyle, DustLabel, LootLabel, RewardModEntry, XpLabel};
use std::collections::BTreeMap;

/// Loot bonus (percent) must be strictly greater than this to add an `lb` tag.
const LOOT_THRESHOLD: i32 = 0;
/// Dust bonus (percent) must be greater than or equal to this to add a `db` tag.
const DUST_THRESHOLD: i32 = 20;

/// Short callout nicknames for known dungeons, keyed by the
/// dungeon's display name. Lookup is done on a normalized form (lowercase,
/// alphanumerics only) so punctuation/spacing/case differences don't matter.
#[rustfmt::skip]
const DUNGEON_NICKNAMES: &[(&str, &str)] = &[
    ("Lost Halls",                   "halls"),
    ("Kogbold Steamworks",           "kog"),
    ("Advanced Kogbold Steamworks",  "akog"),
    ("Fungal Cavern",                "fungal"),
    ("The Nest",                     "nest"),
    ("Plagued Nest",                 "pnest"),
    ("The Shatters",                 "shatts"),
    ("Spectral Penitentiary",        "spen"),
    ("Tomb of the Ancients",         "tomb"),
    ("Ocean Trench",                 "ot"),
    ("Parasite Chambers",            "para"),
    ("Woodland Labyrinth",           "wlab"),
    ("The Crawling Depths",          "cdepths"),
    ("Deadwater Docks",              "ddocks"),
    ("Sulfurous Wetlands",           "sulf"),
    ("High Tech Terror",             "htt"),
    ("Puppet Master's Encore",       "encore"),
    ("Lair of Shaitan",              "shait"),
    ("Cnidarian Reef",               "reef"),
    ("Secluded Thicket",             "thicket"),
    ("Infernal Abyss of Demons",     "inf abyss"),
    ("Heroic Undead Lair",           "hudl"),
    ("Abyss of Demons",              "abyss"),
    ("Undead Lair",                  "udl"),
    ("Snake Pit",                    "snake"),
    ("Manor of the Immortals",       "manor"),
    ("Sprite World",                 "sprite"),
    ("Magic Woods",                  "mw"),
    ("Cursed Library",               "clib"),
    ("Mad Lab",                      "lab"),
    ("Toxic Sewers",                 "sew"),
    ("Puppet Master's Theatre",      "puppet"),
    ("Ancient Ruins",                "ruins"),
    ("Cave of a Thousand Treasures", "tcave"),
    ("The Machine",                  "machine"),
    ("Haunted Cemetery",             "cem"),
    ("Lair of Draconis",             "LOD"),
    ("Beachzone",                    "beachzone"),
    ("Queen Bunny Chamber",          "QBC"),
    ("Battle for the Nexus",         "BFN"),
    ("Belladonna's Garden",          "bella"),
    ("Candyland Hunting Grounds",    "cland"),
    ("Davy Jones' Locker",           "davy"),
    ("Forbidden Jungle",             "jungle"),
    ("Forest Maze",                  "fmaze"),
    ("Ice Tomb",                     "ice tomb"),
    ("Legacy Heroic Abyss of Demons","legacy habyss"),
    ("Legacy Heroic Undead Lair",    "legacy hudl"),
    ("The Trials of Cronus",         "trials"),
    ("Mad God Mayhem",               "mayhem"),
    ("Mountain Temple",              "MT"),
    ("Pirate Cave",                  "pcave"),
    ("Rainbow Road",                 "rroad"),
    ("Spider Den",                   "sden"),
    ("The Hive",                     "hive"),
    ("The Third Dimension",          "3D"),
    ("The Tavern",                   "tavern"),
    ("Mysterious Arena",             "mysterious arena"),
    ("White Snake Invasion I",       "snake 1"),
    ("White Snake Invasion II",      "snake 2"),
    ("White Snake Invasion III",     "snake 3"),
    ("Remnants of the Void",         "rem void"),
];

/// Dungeons that are reachable only from inside other instances and therefore
/// cannot be joined from the nexus. They appear in the Live Feed (for awareness)
/// but never produce a clipboard callout. Matched via [`normalize_dungeon_key`].
#[rustfmt::skip]
const NON_CALLABLE_DUNGEONS: &[&str] = &[
    "Crystal Cavern",
    "Cultist Hideout",
    "The Void",
];

/// Normalize a dungeon name for nickname matching: lowercase ASCII
/// alphanumerics only (drops apostrophes, spaces, punctuation).
fn normalize_dungeon_key(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Resolve a dungeon display name to its short callout nickname, if known.
///
/// Matching is exact (on the normalized key), so variants like "Heroic Undead
/// Lair" and "Undead Lair" resolve independently to `hudl` and `udl`.
pub fn dungeon_nickname(display_name: &str) -> Option<&'static str> {
    let key = normalize_dungeon_key(display_name);
    if key.is_empty() {
        return None;
    }
    DUNGEON_NICKNAMES
        .iter()
        .find(|(name, _)| normalize_dungeon_key(name) == key)
        .map(|&(_, nick)| nick)
}

/// Whether a dungeon is reachable only from inside another instance and so
/// should never produce a clipboard callout (see [`NON_CALLABLE_DUNGEONS`]).
pub fn is_non_callable(display_name: &str) -> bool {
    let key = normalize_dungeon_key(display_name);
    if key.is_empty() {
        return false;
    }
    NON_CALLABLE_DUNGEONS
        .iter()
        .any(|name| normalize_dungeon_key(name) == key)
}

/// Formatting configuration for building a dungeon clipboard callout, borrowed
/// from the live settings. Bundles the name style + overrides, the loot/dust/xp
/// label modes and percent-sign preference, and the editable reward-mod tags.
pub struct DungeonCalloutParams<'a> {
    /// Short nickname vs. full lowercased name.
    pub name_style: DungeonNameStyle,
    /// User short-name overrides keyed by the dungeon's full display name.
    pub name_overrides: &'a BTreeMap<String, String>,
    /// How the loot-boost tag is rendered (or hidden).
    pub loot_label: LootLabel,
    /// How the dust-boost tag is rendered (or hidden).
    pub dust_label: DustLabel,
    /// How the XP-boost tag is rendered (or hidden).
    pub xp_label: XpLabel,
    /// Whether loot/dust/xp values include a `%` sign.
    pub percent: bool,
    /// Editable reward-modifier tags (id, short call, enabled).
    pub reward_mods: &'a [RewardModEntry],
}

/// Resolve a dungeon display name to the callout name part, honoring a user
/// short-name override (Short mode only), then the curated nickname, then the
/// lowercased full name.
fn resolve_dungeon_name(
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
        DungeonNameStyle::Full => display_name.trim().to_lowercase(),
        DungeonNameStyle::Short => dungeon_nickname(display_name)
            .map(|s| s.to_string())
            .unwrap_or_else(|| display_name.trim().to_lowercase()),
    }
}

/// Build the clipboard callout for a dungeon, resolving the name part to a user
/// override, the curated short nickname, or the lowercased full display name.
/// Every dungeon is callable except those in [`NON_CALLABLE_DUNGEONS`]. Returns
/// `None` when the dungeon should have no callout: a non-callable dungeon, or a
/// Dimitus dungeon whose reward-mod entry is disabled.
pub fn dungeon_callout_for(
    display_name: &str,
    modifier_tokens: &[String],
    params: &DungeonCalloutParams,
) -> Option<String> {
    // Dungeons entered only from inside other instances never get a callout.
    if is_non_callable(display_name) {
        return None;
    }
    let name = resolve_dungeon_name(display_name, params.name_style, params.name_overrides);
    if name.trim().is_empty() {
        return None;
    }
    build_dungeon_callout(&name, modifier_tokens, params)
}

/// Build the clipboard callout body for a dungeon from a resolved name plus its
/// modifier-derived tags, in this order: loot, dust, xp, Syndicate, the enabled
/// reward-mod tags (list order), Prismimic, Crab Rave, Alexander's Legacy, and
/// finally Dimitus. Returns `None` for a Dimitus dungeon whose reward-mod entry
/// is disabled (the whole callout is suppressed, matching the legacy toggle).
pub fn build_dungeon_callout(
    nickname: &str,
    modifier_tokens: &[String],
    params: &DungeonCalloutParams,
) -> Option<String> {
    // Dimitus is special: a disabled entry suppresses the whole callout.
    let dimitus = params.reward_mods.iter().find(|e| e.id == "DIMITUS");
    let is_dimitus = has_canonical(modifier_tokens, "DIMITUS");
    if is_dimitus && !dimitus.map(|e| e.enabled).unwrap_or(false) {
        return None;
    }

    let mut tags: Vec<String> = Vec::new();

    // Reward bonuses (thresholds preserved). Each honors its label mode and the
    // percent-sign preference; a `None` label mode omits the tag entirely.
    let bonus = total_bonus(modifier_tokens);
    if bonus.loot > LOOT_THRESHOLD {
        if let Some(tag) = loot_tag(bonus.loot, params.loot_label, params.percent) {
            tags.push(tag);
        }
    }
    if bonus.dust >= DUST_THRESHOLD {
        if let Some(tag) = dust_tag(bonus.dust, params.dust_label, params.percent) {
            tags.push(tag);
        }
    }
    if bonus.xp > 0 {
        if let Some(tag) = xp_tag(bonus.xp, params.xp_label, params.percent) {
            tags.push(tag);
        }
    }

    // Editable reward-mod tags, in list order, skipping Dimitus (emitted last).
    // Syndicate, Prismimic and Crab Rave now live in this list too.
    for entry in params.reward_mods {
        if entry.id == "DIMITUS" {
            continue;
        }
        if let Some(tag) = reward_mod_tag(modifier_tokens, entry) {
            tags.push(tag);
        }
    }

    // Alexander's Legacy carries a percentage, so it keeps special formatting.
    if let Some(pct) = modifier_tokens
        .iter()
        .find_map(|t| alexanders_legacy_pct(&canonical(t)))
    {
        tags.push(if params.percent {
            format!("dying thessal {pct}%")
        } else {
            format!("dying thessal {pct}")
        });
    }

    // Dimitus tag goes last, right before the trailing `j`.
    if let Some(entry) = dimitus {
        if let Some(tag) = reward_mod_tag(modifier_tokens, entry) {
            tags.push(tag);
        }
    }

    let mut call = nickname.to_string();
    for tag in &tags {
        call.push(' ');
        call.push_str(tag);
    }
    Some(call)
}

/// The short call for a reward-mod entry when the dungeon carries it and the
/// entry is enabled with a non-blank call; otherwise `None`.
fn reward_mod_tag(tokens: &[String], entry: &RewardModEntry) -> Option<String> {
    let short = entry.short.trim();
    if !entry.enabled || short.is_empty() {
        return None;
    }
    // Syndicate uses the synthetic id `SYNDICATE`, which spans the tiered wire
    // ids `MERCA`..`MERCD`. All other ids match themselves (and legacy tiered
    // variants like `SOUVENIR_1`, `CRABRAVE_1`) via `has_base_or_numbered`.
    let carried = if entry.id == "SYNDICATE" {
        has_syndicate(tokens)
    } else {
        has_base_or_numbered(tokens, &entry.id)
    };
    if carried {
        Some(short.to_string())
    } else {
        None
    }
}

/// Format a reward value as `<n>% <label>` or `<n> <label>`.
fn format_reward_value(value: i32, label: &str, percent: bool) -> String {
    if percent {
        format!("{value}% {label}")
    } else {
        format!("{value} {label}")
    }
}

/// Loot-boost tag per the configured [`LootLabel`], or `None` when hidden.
fn loot_tag(value: i32, label: LootLabel, percent: bool) -> Option<String> {
    let word = match label {
        LootLabel::Lb => "lb",
        LootLabel::Loot => "loot",
        LootLabel::None => return None,
    };
    Some(format_reward_value(value, word, percent))
}

/// Dust-boost tag per the configured [`DustLabel`], or `None` when hidden.
fn dust_tag(value: i32, label: DustLabel, percent: bool) -> Option<String> {
    let word = match label {
        DustLabel::Db => "db",
        DustLabel::Dust => "dust",
        DustLabel::None => return None,
    };
    Some(format_reward_value(value, word, percent))
}

/// XP-boost tag per the configured [`XpLabel`], or `None` when hidden.
fn xp_tag(value: i32, label: XpLabel, percent: bool) -> Option<String> {
    match label {
        XpLabel::Xp => Some(format_reward_value(value, "xp", percent)),
        XpLabel::None => None,
    }
}

/// Whether any token canonicalizes exactly to `key`.
fn has_canonical(tokens: &[String], key: &str) -> bool {
    tokens.iter().any(|t| canonical(t) == key)
}

/// Whether any token is the Syndicate Takeover modifier (`MERCA`..`MERCD`).
fn has_syndicate(tokens: &[String]) -> bool {
    tokens
        .iter()
        .any(|t| matches!(canonical(t).as_str(), "MERCA" | "MERCB" | "MERCC" | "MERCD"))
}

/// Whether any token is `base` exactly, or `base` followed only by digits (a
/// tier number). This avoids false positives from unrelated modifiers that
/// merely share a prefix.
fn has_base_or_numbered(tokens: &[String], base: &str) -> bool {
    tokens
        .iter()
        .any(|t| match canonical(t).strip_prefix(base) {
            Some(suffix) => suffix.is_empty() || suffix.chars().all(|c| c.is_ascii_digit()),
            None => false,
        })
}

/// If `canon` is an Alexander's Legacy modifier, returns Dying Thessal's spawn
/// chance (percent). The percentages live only in free-text descriptions, so
/// they are hardcoded per tier (I=25, II=50, III=75, current=100).
fn alexanders_legacy_pct(canon: &str) -> Option<u32> {
    match canon {
        "ALEXANDERSLEGACY1" => Some(25),
        "ALEXANDERSLEGACY2" => Some(50),
        "ALEXANDERSLEGACY3" => Some(75),
        "ALEXANDERSLEGACY" => Some(100),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use realmhound_core::settings::default_reward_mods;

    fn tokens(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn params<'a>(
        mods: &'a [RewardModEntry],
        overrides: &'a BTreeMap<String, String>,
        percent: bool,
    ) -> DungeonCalloutParams<'a> {
        DungeonCalloutParams {
            name_style: DungeonNameStyle::Short,
            name_overrides: overrides,
            loot_label: LootLabel::Lb,
            dust_label: DustLabel::Db,
            xp_label: XpLabel::None,
            percent,
            reward_mods: mods,
        }
    }

    /// Build a callout body with the default reward mods (Dimitus OFF) and `%`.
    fn call(nickname: &str, list: &[&str]) -> Option<String> {
        let mods = default_reward_mods();
        let ov = BTreeMap::new();
        build_dungeon_callout(nickname, &tokens(list), &params(&mods, &ov, true))
    }

    /// Reward mods with Dimitus enabled (legacy default behavior).
    fn mods_dimitus_on() -> Vec<RewardModEntry> {
        let mut mods = default_reward_mods();
        if let Some(e) = mods.iter_mut().find(|e| e.id == "DIMITUS") {
            e.enabled = true;
        }
        mods
    }

    /// Build a callout body with Dimitus enabled and `%`.
    fn call_dimitus_on(nickname: &str, list: &[&str]) -> Option<String> {
        let mods = mods_dimitus_on();
        let ov = BTreeMap::new();
        build_dungeon_callout(nickname, &tokens(list), &params(&mods, &ov, true))
    }

    /// Resolve + build a callout from a full display name (default mods, `%`).
    fn call_for(name: &str, list: &[&str]) -> Option<String> {
        let mods = default_reward_mods();
        let ov = BTreeMap::new();
        dungeon_callout_for(name, &tokens(list), &params(&mods, &ov, true))
    }

    #[test]
    fn plain_dungeon_is_just_nickname() {
        assert_eq!(call("snake", &[]).as_deref(), Some("snake"));
    }

    #[test]
    fn dimitus_appends_tag_when_enabled() {
        let _assets = crate::test_support::modifier_assets();
        // With Dimitus enabled, Dimitus dungeons get a trailing `dimitus` tag.
        // Dimitus itself grants +24 dust, so a `db` tag precedes it.
        let out = call_dimitus_on("ddocks", &["DIMITUS"]).unwrap();
        assert!(out.starts_with("ddocks "), "{out}");
        assert!(out.ends_with(" dimitus"), "{out}");
        assert_eq!(out, "ddocks 24% db dimitus");
        // Other tags come first, then `dimitus` last.
        let out = call_dimitus_on("halls", &["LOOTING", "DIMITUS"]).unwrap();
        let lb = out.find("lb").unwrap();
        let dim = out.find("dimitus").unwrap();
        assert!(lb < dim, "ordering wrong: {out}");
        assert!(out.ends_with("dimitus"), "{out}");
    }

    #[test]
    fn dimitus_suppressed_when_disabled() {
        // Default reward mods ship Dimitus OFF -> the whole callout is suppressed.
        assert_eq!(call("ddocks", &["DIMITUS"]), None);
        // Even alongside other mods, the disabled Dimitus entry wins.
        assert_eq!(call("ddocks", &["LOOTING", "DIMITUS"]), None);
    }

    #[test]
    fn loot_threshold_is_strictly_greater_than_zero() {
        let _assets = crate::test_support::modifier_assets();
        // Mystery Effusion grants +20 loot in mods.xml -> 20 > 0.
        let out = call("snake", &["MYSTERYEFFUSION"]).unwrap();
        assert!(out.contains("20% lb"), "got: {out}");
    }

    #[test]
    fn dust_threshold_is_greater_than_or_equal_to_20() {
        let _assets = crate::test_support::modifier_assets();
        // Dust Storm grants +50 dust in mods.xml.
        let out = call("snake", &["DUSTSTORM"]).unwrap();
        assert!(out.contains("50% db"), "got: {out}");
    }

    #[test]
    fn no_tag_when_below_threshold() {
        let _assets = crate::test_support::modifier_assets();
        // Colorful = +10 loot / +18 dust in mods.xml; loot exceeds 0 so lb shows,
        // but dust <20 so no db tag.
        let out = call("snake", &["COLORFUL"]).unwrap();
        assert!(out.contains("10% lb"), "got: {out}");
        assert!(!out.contains("db"), "got: {out}");
    }

    #[test]
    fn both_loot_and_dust_thresholds() {
        let _assets = crate::test_support::modifier_assets();
        let out = call("halls", &["LOOTING", "DUSTSTORM"]).unwrap();
        assert!(out.contains("50% lb"), "got: {out}");
        assert!(out.contains("50% db"), "got: {out}");
        // Loot tag comes before dust tag.
        let lb = out.find("lb").unwrap();
        let db = out.find("db").unwrap();
        assert!(lb < db, "ordering wrong: {out}");
    }

    #[test]
    fn syndicate_tag_from_merc_ids() {
        for id in ["MERCA", "MERCB", "MERCC", "MERCD"] {
            let out = call("halls", &[id]).unwrap();
            assert!(out.contains("synd"), "{id} -> {out}");
        }
    }

    #[test]
    fn flag_tags_resolve_from_wire_ids() {
        let cases = [
            ("GENEROUS", "generous"),
            ("FOOUNDTREASURE", "troom"),
            ("SOUVENIR", "souv"),
            ("SOUVENIR_1", "souv"),
            ("EXALTEDBANNER", "banner"),
            ("KEYFAIRY", "keyf"),
            ("SKINHUNTER", "skinhunt"),
            ("SKINHUNTER_1", "skinhunt"),
            ("MYSTERYSKIN", "skin token"),
            ("WANDERERBOSS", "wanderer"),
            ("PRISMIMIC", "prismimic"),
            ("NOBLEBOSS", "court"),
            ("SPIRITSNAKESWARM", "snakes"),
            ("NILDROPS", "nildrop"),
            ("ALIENWORMHOLES", "alien portal"),
            ("LANTERNLIGHT", "troom activated"),
            ("SOULBOOST", "soul boost"),
            ("SQUARED", "squared"),
            ("STEAMWORKSMAINTENANCE", "turrets off"),
            ("SHATTERSACCEL", "lingering magi"),
        ];
        for (id, tag) in cases {
            let out = call("d", &[id]).unwrap();
            assert!(out.contains(tag), "{id} should yield {tag}, got: {out}");
        }
    }

    #[test]
    fn disabled_reward_mod_emits_no_tag() {
        // Loot Bunny ships disabled with a blank call -> no tag even when present.
        let out = call("d", &["EASTERLOOTBUNNY"]).unwrap();
        assert_eq!(out, "d", "disabled reward mod should not add a tag: {out}");
    }

    #[test]
    fn crab_rave_tag_ignores_tier() {
        // Both the current and legacy tiered wire ids map to the same tag now.
        let out = call("ddocks", &["CRABRAVE"]).unwrap();
        assert!(
            out.contains("crab rave") && !out.contains("crab rave 1"),
            "{out}"
        );
        let out1 = call("ddocks", &["CRABRAVE_1"]).unwrap();
        assert!(
            out1.contains("crab rave") && !out1.contains("crab rave 1"),
            "{out1}"
        );
    }

    #[test]
    fn prefix_tags_do_not_false_positive() {
        // A modifier merely sharing a prefix must not trigger the tag.
        let out = call("d", &["MERCENARYGUILD"]).unwrap();
        assert!(!out.contains("synd"), "{out}");
        let out = call("d", &["SOUVENIRSHOP"]).unwrap();
        assert!(!out.contains("souv"), "{out}");
        let out = call("d", &["SKINHUNTERPRO"]).unwrap();
        assert!(!out.contains("skinhunt"), "{out}");
        // But exact + numbered tiers still match.
        assert!(call("d", &["SOUVENIR_2"]).unwrap().contains("souv"));
        assert!(call("d", &["SKINHUNTER_2"]).unwrap().contains("skinhunt"));
    }

    #[test]
    fn alexanders_legacy_percentages() {
        let cases = [
            ("ALEXANDERSLEGACY_1", "dying thessal 25%"),
            ("ALEXANDERSLEGACY_2", "dying thessal 50%"),
            ("ALEXANDERSLEGACY_3", "dying thessal 75%"),
            ("ALEXANDERSLEGACY", "dying thessal 100%"),
        ];
        for (id, expect) in cases {
            let out = call("ot", &[id]).unwrap();
            assert!(out.contains(expect), "{id} -> {out}");
        }
    }

    #[test]
    fn combined_tags_keep_order() {
        let _assets = crate::test_support::modifier_assets();
        // Looting (+50 loot) + Generous + Syndicate (MERCB).
        let out = call("halls", &["LOOTING", "GENEROUS", "MERCB"]).unwrap();
        assert!(out.starts_with("halls "), "{out}");
        let lb = out.find("lb").unwrap();
        let synd = out.find("synd").unwrap();
        let gen = out.find("generous").unwrap();
        assert!(lb < synd && synd < gen, "order wrong: {out}");
    }

    #[test]
    fn nickname_resolves_known_dungeons() {
        assert_eq!(dungeon_nickname("Snake Pit"), Some("snake"));
        assert_eq!(dungeon_nickname("Lost Halls"), Some("halls"));
        assert_eq!(dungeon_nickname("The Machine"), Some("machine"));
        // Heroic / variant dungeons resolve independently of their base.
        assert_eq!(dungeon_nickname("Undead Lair"), Some("udl"));
        assert_eq!(dungeon_nickname("Heroic Undead Lair"), Some("hudl"));
        assert_eq!(dungeon_nickname("Abyss of Demons"), Some("abyss"));
        assert_eq!(
            dungeon_nickname("Infernal Abyss of Demons"),
            Some("inf abyss")
        );
        assert_eq!(dungeon_nickname("Kogbold Steamworks"), Some("kog"));
        assert_eq!(
            dungeon_nickname("Advanced Kogbold Steamworks"),
            Some("akog")
        );
    }

    #[test]
    fn nickname_matching_ignores_case_and_punctuation() {
        assert_eq!(dungeon_nickname("puppet master's theatre"), Some("puppet"));
        assert_eq!(dungeon_nickname("PUPPET MASTERS THEATRE"), Some("puppet"));
        assert_eq!(
            dungeon_nickname("Cave of a Thousand Treasures"),
            Some("tcave")
        );
    }

    #[test]
    fn nickname_unknown_dungeon_is_none() {
        assert_eq!(dungeon_nickname("Nexus"), None);
        assert_eq!(dungeon_nickname(""), None);
    }

    #[test]
    fn nickname_table_has_no_duplicates() {
        use std::collections::HashSet;
        let mut keys = HashSet::new();
        let mut nicks = HashSet::new();
        let mut norm_nicks = HashSet::new();
        for (name, nick) in DUNGEON_NICKNAMES {
            assert!(keys.insert(normalize_dungeon_key(name)), "dup name {name}");
            assert!(nicks.insert(*nick), "dup nickname {nick}");
            // Also reject case/punctuation-only duplicate command forms.
            assert!(
                norm_nicks.insert(normalize_dungeon_key(nick)),
                "normalized dup nickname {nick}"
            );
        }
        assert_eq!(DUNGEON_NICKNAMES.len(), 62);
    }

    #[test]
    fn nickname_resolves_issue_107_additions() {
        // Keyed off the authoritative display names (see stats::ALL_DUNGEONS).
        let cases = [
            ("Lair of Draconis", "LOD"),
            ("Beachzone", "beachzone"),
            ("Queen Bunny Chamber", "QBC"),
            ("Battle for the Nexus", "BFN"),
            ("Belladonna's Garden", "bella"),
            ("Candyland Hunting Grounds", "cland"),
            ("Davy Jones' Locker", "davy"),
            ("Forbidden Jungle", "jungle"),
            ("Forest Maze", "fmaze"),
            ("Ice Tomb", "ice tomb"),
            ("Legacy Heroic Abyss of Demons", "legacy habyss"),
            ("Legacy Heroic Undead Lair", "legacy hudl"),
            ("The Trials of Cronus", "trials"),
            ("Mad God Mayhem", "mayhem"),
            ("Mountain Temple", "MT"),
            ("Pirate Cave", "pcave"),
            ("Rainbow Road", "rroad"),
            ("Spider Den", "sden"),
            ("The Hive", "hive"),
            ("The Third Dimension", "3D"),
            ("The Tavern", "tavern"),
            ("Mysterious Arena", "mysterious arena"),
            ("White Snake Invasion I", "snake 1"),
            ("White Snake Invasion II", "snake 2"),
            ("White Snake Invasion III", "snake 3"),
            ("Remnants of the Void", "rem void"),
        ];
        for (name, nick) in cases {
            assert_eq!(dungeon_nickname(name), Some(nick), "name {name}");
        }
    }

    #[test]
    fn non_callable_dungeons_have_no_callout() {
        for name in ["Crystal Cavern", "Cultist Hideout", "The Void"] {
            assert!(is_non_callable(name), "{name} should be non-callable");
            assert_eq!(call_for(name, &[]), None, "{name}");
            // Matching ignores case/punctuation.
            assert_eq!(call_for(&name.to_lowercase(), &[]), None, "{name}");
        }
        // Joinable dungeons are unaffected.
        assert!(!is_non_callable("Snake Pit"));
    }

    #[test]
    fn non_callable_wins_over_dimitus() {
        // Even with Dimitus enabled, a non-callable dungeon yields no callout.
        let mods = mods_dimitus_on();
        let ov = BTreeMap::new();
        assert_eq!(
            dungeon_callout_for("The Void", &tokens(&["DIMITUS"]), &params(&mods, &ov, true)),
            None
        );
    }

    #[test]
    fn callout_for_resolves_curated_nickname() {
        let _assets = crate::test_support::modifier_assets();
        assert_eq!(
            call_for("Haunted Cemetery", &["MYSTERYEFFUSION"]).as_deref(),
            Some("cem 20% lb")
        );
    }

    #[test]
    fn callout_for_falls_back_to_lowercased_full_name() {
        let _assets = crate::test_support::modifier_assets();
        // Unknown dungeon -> lowercased full name so it's still callable.
        // Looting is a numeric boost only (its name tag is off by default).
        assert_eq!(
            call_for("Some Unknown Place", &["LOOTING"]).as_deref(),
            Some("some unknown place 50% lb")
        );
        // No mods -> just the name (join marker is added later by the panel).
        assert_eq!(
            call_for("Some Unknown Place", &[]).as_deref(),
            Some("some unknown place")
        );
    }

    #[test]
    fn callout_for_dimitus_unresolved_dungeon() {
        // With Dimitus enabled, an unresolved Dimitus dungeon gets a callout.
        let mods = mods_dimitus_on();
        let ov = BTreeMap::new();
        let out = dungeon_callout_for(
            "Some Random Dungeon",
            &tokens(&["DIMITUS"]),
            &params(&mods, &ov, true),
        )
        .unwrap();
        assert!(out.starts_with("some random dungeon "), "{out}");
        assert!(out.ends_with(" dimitus"), "{out}");
        // Default (disabled): no callout.
        assert_eq!(call_for("Some Random Dungeon", &["DIMITUS"]), None);
    }

    #[test]
    fn full_name_style_uses_lowercased_display_name() {
        let _assets = crate::test_support::modifier_assets();
        let mods = default_reward_mods();
        let ov = BTreeMap::new();
        let mut p = params(&mods, &ov, true);
        p.name_style = DungeonNameStyle::Full;
        assert_eq!(
            dungeon_callout_for("Lost Halls", &tokens(&["MYSTERYEFFUSION"]), &p).as_deref(),
            Some("lost halls 20% lb")
        );
        // Short style still resolves the nickname.
        assert_eq!(
            call_for("Lost Halls", &["MYSTERYEFFUSION"]).as_deref(),
            Some("halls 20% lb")
        );
    }

    #[test]
    fn name_override_takes_precedence_in_short_mode() {
        let _assets = crate::test_support::modifier_assets();
        let mods = default_reward_mods();
        let mut ov = BTreeMap::new();
        ov.insert("Lost Halls".to_string(), "lh".to_string());
        let p = params(&mods, &ov, true);
        assert_eq!(
            dungeon_callout_for("Lost Halls", &tokens(&["MYSTERYEFFUSION"]), &p).as_deref(),
            Some("lh 20% lb")
        );
    }

    #[test]
    fn words_label_style_uses_loot_and_dust() {
        let _assets = crate::test_support::modifier_assets();
        let mods = default_reward_mods();
        let ov = BTreeMap::new();
        let mut p = params(&mods, &ov, true);
        p.loot_label = LootLabel::Loot;
        p.dust_label = DustLabel::Dust;
        let out = build_dungeon_callout("halls", &tokens(&["LOOTING", "DUSTSTORM"]), &p).unwrap();
        assert!(out.contains("50% loot"), "got: {out}");
        assert!(out.contains("50% dust"), "got: {out}");
        assert!(!out.contains(" lb"), "got: {out}");
        assert!(!out.contains(" db"), "got: {out}");
    }

    #[test]
    fn hidden_label_modes_omit_reward_tags() {
        let mods = default_reward_mods();
        let ov = BTreeMap::new();
        let mut p = params(&mods, &ov, true);
        p.loot_label = LootLabel::None;
        p.dust_label = DustLabel::None;
        let out = build_dungeon_callout("halls", &tokens(&["LOOTING", "DUSTSTORM"]), &p).unwrap();
        // Boost tags are numeric (`50% lb` etc.); hidden modes emit no numbers.
        // (The "looting" reward-mod tag may still appear; it carries no value.)
        assert!(!out.contains('%'), "got: {out}");
        assert!(!out.contains("50"), "got: {out}");
        assert!(!out.contains(" lb") && !out.contains(" db"), "got: {out}");
    }

    #[test]
    fn xp_tag_emitted_only_when_enabled() {
        let _assets = crate::test_support::modifier_assets();
        let mods = default_reward_mods();
        let ov = BTreeMap::new();
        // Experienced grants an XP bonus; default XpLabel::None hides it.
        let out = call("halls", &["EXPERIENCED"]).unwrap();
        assert!(!out.contains("xp"), "xp hidden by default: {out}");
        // With XpLabel::Xp it appears.
        let mut p = params(&mods, &ov, true);
        p.xp_label = XpLabel::Xp;
        let out = build_dungeon_callout("halls", &tokens(&["EXPERIENCED"]), &p).unwrap();
        assert!(out.contains("xp"), "xp shown when enabled: {out}");
    }

    #[test]
    fn percent_sign_excluded_when_disabled() {
        let _assets = crate::test_support::modifier_assets();
        let mods = default_reward_mods();
        let ov = BTreeMap::new();
        let p = params(&mods, &ov, false);
        let out = build_dungeon_callout("halls", &tokens(&["LOOTING", "DUSTSTORM"]), &p).unwrap();
        assert!(out.contains("50 lb"), "got: {out}");
        assert!(out.contains("50 db"), "got: {out}");
        assert!(!out.contains('%'), "got: {out}");
    }

    #[test]
    fn percent_sign_excluded_from_alexanders_legacy() {
        let mods = default_reward_mods();
        let ov = BTreeMap::new();
        let p = params(&mods, &ov, false);
        let out = build_dungeon_callout("ot", &tokens(&["ALEXANDERSLEGACY_1"]), &p).unwrap();
        assert!(out.contains("dying thessal 25"), "got: {out}");
        assert!(!out.contains('%'), "got: {out}");
    }
}

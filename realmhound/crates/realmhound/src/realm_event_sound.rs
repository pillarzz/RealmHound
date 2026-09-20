//! Realm-event notification sound triggering.
//!
//! Maps a Live Feed boss call (see [`crate::sound::EventSound`]) to the
//! configured realm-event notification, applying the section exclusions:
//! Commander Calbrik and the UFO are routed to the Alien Invasion section
//! (not Veteran/Adept), and the dungeon-internal seasonal bosses never notify.

use std::collections::HashMap;

use realmhound_core::assets::{
    get_asset_manager, BossGroup, CatalogEntry, SEASONAL_NOTIFY_EXCLUDED_IDS,
};
use realmhound_core::settings::RealmEventSounds;

use crate::sound::EventSound;

/// A catalog-backed realm-event section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionKind {
    Veteran,
    Adept,
    Seasonal,
}

impl SectionKind {
    /// Stable prefix used in `custom_sounds` keys and coalescing.
    pub fn key_prefix(self) -> &'static str {
        match self {
            SectionKind::Veteran => "veteran",
            SectionKind::Adept => "adept",
            SectionKind::Seasonal => "seasonal",
        }
    }

    pub fn default_sound(self) -> EventSound {
        match self {
            SectionKind::Veteran => EventSound::Veteran,
            SectionKind::Adept => EventSound::Adept,
            SectionKind::Seasonal => EventSound::Seasonal,
        }
    }

    fn group(self) -> BossGroup {
        match self {
            SectionKind::Veteran => BossGroup::VeteranEncounter,
            SectionKind::Adept => BossGroup::AdeptEncounter,
            SectionKind::Seasonal => BossGroup::SeasonalEncounter,
        }
    }
}

/// The boss id a catalog entry represents (its sprite sits last, after any
/// dungeon-portal icon).
fn entry_boss_id(entry: &CatalogEntry) -> Option<i32> {
    entry.sprite_ids.last().copied()
}

/// The catalog for a section with the section's exclusions applied. Shared by
/// the settings picker and the trigger sets so both stay in lockstep.
pub fn section_catalog(kind: SectionKind) -> Vec<CatalogEntry> {
    let mut entries = kind.group().catalog_bosses();
    match kind {
        SectionKind::Veteran => {
            entries.retain(|e| {
                let n = e.name.to_lowercase();
                // Calbrik is routed to the Alien section. Behemoth's Egg (20799)
                // spawns paired with the Flying Behemoth boss and shares its
                // VETERAN_ENCOUNTER label, so it's a duplicate callout here; drop
                // it from the sound list (Combat History still tracks it).
                entry_boss_id(e) != Some(20799) && !n.contains("calbrik")
            });
        }
        SectionKind::Adept => {
            entries.retain(|e| {
                let n = e.name.to_lowercase();
                // These Adept encounters are excluded from notifications:
                // - UFO is tied to the Alien Invasion and routed to that section
                // - Carp Emperor gets no chat announcement, so it can only be
                //   found in-game (quest marker / minimap).
                !n.contains("ufo") && !n.contains("carp emperor")
            });
        }
        SectionKind::Seasonal => {
            entries.retain(|e| {
                entry_boss_id(e)
                    .map(|id| !SEASONAL_NOTIFY_EXCLUDED_IDS.contains(&id))
                    .unwrap_or(true)
            });
        }
    }
    entries
}

/// Runtime boss-id lookups resolved from the asset catalogs. Built once per
/// boss call (boss calls are rare); kept separate from [`resolve`] so the
/// decision logic is unit-testable without loaded assets.
///
/// Each section map keys every triggering object id (including biome / "New"
/// variants that share a display name) to that encounter's *representative* id
/// -- the same id the settings picker stores for an override. This lets a live
/// spawn arriving under a variant id (e.g. the old Skull Shrine, id 3414) still
/// match the catalog entry and any override (New Skull Shrine, id 22003).
pub struct EventSets {
    pub veteran: HashMap<i32, i32>,
    pub adept: HashMap<i32, i32>,
    pub seasonal: HashMap<i32, i32>,
    /// Commander Calbrik's boss id (excluded from Veteran, routed to Alien).
    pub calbrik: Option<i32>,
    /// UFO's boss id (excluded from Adept, routed to Alien).
    pub ufo: Option<i32>,
}

impl EventSets {
    pub fn from_assets() -> Self {
        let veteran_full = BossGroup::VeteranEncounter.catalog_bosses();
        let adept_full = BossGroup::AdeptEncounter.catalog_bosses();

        let calbrik = veteran_full
            .iter()
            .find(|e| e.name.to_lowercase().contains("calbrik"))
            .and_then(entry_boss_id);
        let ufo = adept_full
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case("UFO"))
            .and_then(entry_boss_id);

        Self {
            veteran: section_id_map(SectionKind::Veteran),
            adept: section_id_map(SectionKind::Adept),
            seasonal: section_id_map(SectionKind::Seasonal),
            calbrik,
            ufo,
        }
    }
}

/// Map every triggering object id in a section to its representative catalog id.
///
/// Seeds each catalog entry's own sprite ids, then expands by display name so
/// re-skinned / biome variants of the same encounter (which the name-deduped
/// catalog drops) still resolve to the representative the picker stored.
fn section_id_map(kind: SectionKind) -> HashMap<i32, i32> {
    let am = get_asset_manager();
    let mut map: HashMap<i32, i32> = HashMap::new();
    for entry in section_catalog(kind) {
        let Some(&rep) = entry.sprite_ids.last() else {
            continue;
        };
        for sid in entry.sprite_ids {
            map.entry(sid).or_insert(rep);
            for vid in am.ids_sharing_display_name(sid) {
                map.entry(vid).or_insert(rep);
            }
        }
    }
    map
}

/// Sprite ids for the Alien UFO and Commander Calbrik boss icons, resolved by
/// substring from the Adept/Veteran catalogs so the settings rows can show a
/// leading icon. Returns `(ufo, calbrik)`; either may be `None` if the asset
/// catalog has no matching entry.
pub fn alien_icon_ids() -> (Option<i32>, Option<i32>) {
    let icon_from = |group: BossGroup, needle: &str| -> Option<i32> {
        group
            .catalog_bosses()
            .iter()
            .find(|e| e.name.to_lowercase().contains(needle))
            .and_then(|e| e.sprite_ids.last().copied())
    };
    let ufo = icon_from(BossGroup::AdeptEncounter, "ufo");
    let calbrik = icon_from(BossGroup::VeteranEncounter, "calbrik");
    (ufo, calbrik)
}

/// UI state for one section's boss-search autocomplete (adapted from the
/// Treasury dungeon-category picker).
#[derive(Default)]
pub struct EventPickerState {
    pub text: String,
    pub prev_text: String,
    pub popup_open: bool,
    pub selected: Option<usize>,
}

/// The sound + volume to play for a matched boss call.
#[derive(Debug, Clone, PartialEq)]
pub struct EventTrigger {
    pub sound: EventSound,
    /// `custom_sounds` key selecting a per-boss/per-row custom override (may be
    /// absent, in which case the embedded default plays).
    pub custom_key: String,
    /// Per-event volume (0.0-1.0); the audio thread scales it by master.
    pub volume: f32,
}

/// Alien custom-sound / coalescing keys.
pub const ALIEN_WAVE_KEY: &str = "event:alien:wave";
pub const ALIEN_UFO_KEY: &str = "event:alien:ufo";
pub const ALIEN_CALBRIK_KEY: &str = "event:alien:calbrik";

/// The `custom_sounds` key for a per-boss override in a section.
pub fn override_key(kind: SectionKind, boss_id: i32) -> String {
    format!("event:{}:{}", kind.key_prefix(), boss_id)
}

/// Why a boss call produced no sound (recorded by the diagnostic log so a
/// silent event can be traced back to its cause).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SilentReason {
    /// Alien Invasion event but the Alien section is disabled.
    AlienDisabled,
    /// The boss belongs to a section whose mode is None.
    SectionNone,
    /// Selected mode and the boss is not in the section's list.
    NotSelected,
    /// The boss id matched no notification section at all.
    NoSection,
}

impl SilentReason {
    pub fn as_str(self) -> &'static str {
        match self {
            SilentReason::AlienDisabled => "Alien section disabled",
            SilentReason::SectionNone => "section set to None",
            SilentReason::NotSelected => "Selected mode, boss not in list",
            SilentReason::NoSection => "boss not in any notification section",
        }
    }
}

/// The full outcome of matching a boss call, for both playback and the
/// diagnostic log. `section` is a human label ("Veteran"/"Adept"/"Seasonal"/
/// "Alien"/"-"); `matched_id` is the representative catalog id the spawn id
/// resolved to (the same id the settings picker stores), or `None` when the
/// spawn id matched nothing.
#[derive(Debug, Clone)]
pub struct EventDiagnosis {
    pub section: &'static str,
    pub matched_id: Option<i32>,
    pub trigger: Option<EventTrigger>,
    pub silent_reason: Option<SilentReason>,
}

impl EventDiagnosis {
    fn played(section: &'static str, matched_id: Option<i32>, trigger: EventTrigger) -> Self {
        Self {
            section,
            matched_id,
            trigger: Some(trigger),
            silent_reason: None,
        }
    }

    fn silent(section: &'static str, matched_id: Option<i32>, reason: SilentReason) -> Self {
        Self {
            section,
            matched_id,
            trigger: None,
            silent_reason: Some(reason),
        }
    }
}

/// Decide which realm-event sound (if any) a boss call should play.
///
/// Alien Invasion is matched first (by callout text / boss id) so Calbrik and
/// the UFO never fall through to the encounter sections they are excluded from.
pub fn resolve(
    events: &RealmEventSounds,
    boss_id: i32,
    text: &str,
    sets: &EventSets,
) -> Option<EventTrigger> {
    diagnose(events, boss_id, text, sets).trigger
}

/// Like [`resolve`] but always returns the full [`EventDiagnosis`] (matched
/// section, representative id, and -- when silent -- the reason). Used by the
/// diagnostic log so every Live Feed boss call can be recorded, including the
/// silent ones.
pub fn diagnose(
    events: &RealmEventSounds,
    boss_id: i32,
    text: &str,
    sets: &EventSets,
) -> EventDiagnosis {
    use realmhound_core::settings::EventMode;

    // Alien Invasion: wave starts announce as "Alien Invasion ..."; the UFO and
    // Commander Calbrik are identified by id (with a text fallback for the UFO).
    let alien =
        |sound: EventSound, key: &str, volume: f32, matched: Option<i32>| -> EventDiagnosis {
            if events.alien.enabled {
                EventDiagnosis::played(
                    "Alien",
                    matched,
                    EventTrigger {
                        sound,
                        custom_key: key.to_string(),
                        volume,
                    },
                )
            } else {
                EventDiagnosis::silent("Alien", matched, SilentReason::AlienDisabled)
            }
        };
    if text.starts_with("Alien Invasion") {
        return alien(
            EventSound::AlienWave,
            ALIEN_WAVE_KEY,
            events.alien.wave.volume,
            None,
        );
    }
    if text == "UFO" || sets.ufo == Some(boss_id) {
        return alien(
            EventSound::Ufo,
            ALIEN_UFO_KEY,
            events.alien.ufo.volume,
            Some(boss_id),
        );
    }
    if sets.calbrik == Some(boss_id) {
        return alien(
            EventSound::Calbrik,
            ALIEN_CALBRIK_KEY,
            events.alien.calbrik.volume,
            Some(boss_id),
        );
    }

    // Encounter sections. Each map is disjoint, so a spawn id belongs to at most
    // one section -- find that section (regardless of mode) so a silent outcome
    // can name the real reason.
    let sections = [
        (
            SectionKind::Veteran,
            "Veteran",
            &events.veteran,
            &sets.veteran,
        ),
        (SectionKind::Adept, "Adept", &events.adept, &sets.adept),
        (
            SectionKind::Seasonal,
            "Seasonal",
            &events.seasonal,
            &sets.seasonal,
        ),
    ];
    let Some((kind, label, section, rep)) =
        sections
            .into_iter()
            .find_map(|(kind, label, section, map)| {
                map.get(&boss_id).map(|&rep| (kind, label, section, rep))
            })
    else {
        return EventDiagnosis::silent("-", None, SilentReason::NoSection);
    };

    if section.mode == EventMode::None {
        return EventDiagnosis::silent(label, Some(rep), SilentReason::SectionNone);
    }

    // Match the override against the representative id (what the picker stores)
    // so biome variants of the same encounter still resolve.
    let over = section.overrides.iter().find(|o| o.boss_id == rep);
    let (volume, custom_key) = match (section.mode, over) {
        // Listed boss (custom sound/volume): honored in both All and Selected.
        (_, Some(o)) => (o.volume, override_key(kind, rep)),
        // All: unlisted bosses play the section default.
        (EventMode::All, None) => (
            section.default_volume,
            // Per-boss key: no custom file is stored under it (so the embedded
            // default plays), but it keeps the audio thread from coalescing two
            // *different* default bosses into one sound.
            format!("event:{}:default:{}", kind.key_prefix(), rep),
        ),
        // Selected: unlisted bosses are silent.
        (EventMode::Selected, None) => {
            return EventDiagnosis::silent(label, Some(rep), SilentReason::NotSelected);
        }
        (EventMode::None, None) => unreachable!("None handled above"),
    };
    EventDiagnosis::played(
        label,
        Some(rep),
        EventTrigger {
            sound: kind.default_sound(),
            custom_key,
            volume,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use realmhound_core::settings::{EventMode, EventOverride, EventSection};

    fn sets() -> EventSets {
        EventSets {
            veteran: [100, 101].into_iter().map(|id| (id, id)).collect(),
            adept: [200].into_iter().map(|id| (id, id)).collect(),
            seasonal: [300].into_iter().map(|id| (id, id)).collect(),
            calbrik: Some(900),
            ufo: Some(901),
        }
    }

    fn events() -> RealmEventSounds {
        let mut e = RealmEventSounds::default();
        e.veteran.mode = EventMode::All;
        e.veteran.default_volume = 0.5;
        e.adept.mode = EventMode::All;
        e.seasonal.mode = EventMode::All;
        e.alien.enabled = true;
        e
    }

    #[test]
    fn veteran_boss_uses_section_default() {
        let t = resolve(&events(), 100, "Lost Sentry", &sets()).unwrap();
        assert_eq!(t.sound, EventSound::Veteran);
        assert_eq!(t.volume, 0.5);
        assert_eq!(t.custom_key, "event:veteran:default:100");
    }

    #[test]
    fn override_wins_over_default() {
        let mut e = events();
        e.veteran.overrides.push(EventOverride {
            boss_id: 100,
            name: "Lost Sentry".into(),
            volume: 0.9,
        });
        let t = resolve(&e, 100, "Lost Sentry", &sets()).unwrap();
        assert_eq!(t.volume, 0.9);
        assert_eq!(t.custom_key, "event:veteran:100");
    }

    #[test]
    fn variant_id_resolves_to_representative_override() {
        // A biome / "New" variant (id 999) that maps to the catalog
        // representative (id 200) must still hit the override stored on 200
        // (regression for Skull Shrine: live spawn id != catalog id).
        let mut s = sets();
        s.adept.insert(999, 200);
        let mut e = events();
        e.adept.mode = EventMode::Selected;
        e.adept.overrides.push(EventOverride {
            boss_id: 200,
            name: "Skull Shrine".into(),
            volume: 0.8,
        });
        let t = resolve(&e, 999, "Skull Shrine", &s).unwrap();
        assert_eq!(t.sound, EventSound::Adept);
        assert_eq!(t.volume, 0.8);
        assert_eq!(t.custom_key, "event:adept:200");
    }

    #[test]
    fn none_mode_section_is_silent() {
        let mut e = events();
        e.veteran.mode = EventMode::None;
        assert!(resolve(&e, 100, "Lost Sentry", &sets()).is_none());
    }

    #[test]
    fn selected_mode_only_listed_bosses_play() {
        let mut e = events();
        e.veteran.mode = EventMode::Selected;
        // Nothing selected yet: the in-category boss is silent.
        assert!(resolve(&e, 100, "Lost Sentry", &sets()).is_none());
        // Select boss 100: now it plays (default sound unless a custom file is set).
        e.veteran.overrides.push(EventOverride {
            boss_id: 100,
            name: "Lost Sentry".into(),
            volume: 0.7,
        });
        let t = resolve(&e, 100, "Lost Sentry", &sets()).unwrap();
        assert_eq!(t.sound, EventSound::Veteran);
        assert_eq!(t.volume, 0.7);
        assert_eq!(t.custom_key, "event:veteran:100");
        // A different in-category boss that is not selected stays silent.
        assert!(resolve(&e, 101, "Other", &sets()).is_none());
    }

    #[test]
    fn alien_wave_ufo_calbrik_route_to_alien() {
        let e = events();
        let s = sets();
        assert_eq!(
            resolve(&e, 42, "Alien Invasion Adept - Wave 1", &s)
                .unwrap()
                .sound,
            EventSound::AlienWave
        );
        assert_eq!(resolve(&e, 901, "UFO", &s).unwrap().sound, EventSound::Ufo);
        assert_eq!(
            resolve(&e, 900, "Commander Calbrik", &s).unwrap().sound,
            EventSound::Calbrik
        );
    }

    #[test]
    fn calbrik_and_ufo_never_fall_through_when_alien_off() {
        let mut e = events();
        e.alien.enabled = false;
        let s = sets();
        // Even though Veteran/Adept are on, the excluded ids produce nothing.
        assert!(resolve(&e, 900, "Commander Calbrik", &s).is_none());
        assert!(resolve(&e, 901, "UFO", &s).is_none());
    }

    #[test]
    fn unrelated_boss_is_silent() {
        assert!(resolve(&events(), 555, "Some Boss", &sets()).is_none());
    }

    // Guard against the settings default drifting away from "all off".
    #[test]
    fn defaults_are_all_off() {
        let e = RealmEventSounds::default();
        assert!(e.veteran.mode == EventMode::None);
        assert!(e.adept.mode == EventMode::None);
        assert!(e.seasonal.mode == EventMode::None);
        assert!(!e.alien.enabled);
        let _ = EventSection::default();
    }
}

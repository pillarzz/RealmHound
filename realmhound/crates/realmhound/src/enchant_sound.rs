//! Enchantment notification sound matching.
//!
//! Maps the enchantments found in a loot bag to the configured enchantment
//! notification categories:
//! - **Tiered**: matched by stable family key (e.g. `LOOT_BONUS`) plus a
//!   minimum tier, so any enchant of that family at the selected tier or above
//!   notifies.
//! - **Unique / Awakened**: matched by exact internal id (e.g.
//!   `FLURRY_OF_BLOWS`).

use std::collections::{HashMap, HashSet};

use realmhound_core::assets::EnchantCatalogEntry;
use realmhound_core::settings::EnchantmentSounds;

use crate::sound::EventSound;

/// The two enchantment notification categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnchantCategory {
    Tiered,
    Special,
}

impl EnchantCategory {
    /// Stable prefix used in `custom_sounds` keys.
    pub fn key_prefix(self) -> &'static str {
        match self {
            EnchantCategory::Tiered => "tiered",
            EnchantCategory::Special => "special",
        }
    }
}

/// The `custom_sounds` key for a configured entry in a category.
pub fn override_key(cat: EnchantCategory, entry_key: &str) -> String {
    format!("enchant:{}:{}", cat.key_prefix(), entry_key)
}

/// A sound to play for a matched enchant. The embedded default is always
/// [`EventSound::Enchant`]; `custom_key` selects an optional per-entry custom
/// override, and `volume` is the per-entry volume (scaled by master later).
#[derive(Debug, Clone, PartialEq)]
pub struct EnchantTrigger {
    pub custom_key: String,
    pub volume: f32,
}

/// Match the enchants found in a loot bag against the configured notification
/// categories, returning one trigger per distinct matched entry (deduplicated
/// by `custom_key` so a single bag never plays the same sound twice).
pub fn resolve(
    settings: &EnchantmentSounds,
    dropped_enchant_ids: &[i32],
    catalog: &[EnchantCatalogEntry],
) -> Vec<EnchantTrigger> {
    if !settings.tiered.enabled && !settings.special.enabled {
        return Vec::new();
    }

    // Resolve the dropped enchant ids to their catalog entries.
    let lookup: HashMap<u16, &EnchantCatalogEntry> =
        catalog.iter().map(|e| (e.type_id, e)).collect();
    let dropped: Vec<&EnchantCatalogEntry> = dropped_enchant_ids
        .iter()
        .filter_map(|&id| u16::try_from(id).ok())
        .filter_map(|id| lookup.get(&id).copied())
        .collect();
    if dropped.is_empty() {
        return Vec::new();
    }

    let mut triggers = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    if settings.tiered.enabled {
        for entry in &settings.tiered.entries {
            let Some(min_tier) = entry.tier else { continue };
            let matched = dropped.iter().any(|d| {
                d.family_id == entry.key && d.tier_num.map(|t| t >= min_tier).unwrap_or(false)
            });
            if matched {
                let key = override_key(EnchantCategory::Tiered, &entry.key);
                if seen.insert(key.clone()) {
                    triggers.push(EnchantTrigger {
                        custom_key: key,
                        volume: entry.volume,
                    });
                }
            }
        }
    }

    if settings.special.enabled {
        for entry in &settings.special.entries {
            let matched = dropped.iter().any(|d| {
                d.tier_class.is_special()
                    && (d.display_name == entry.key || d.internal_id == entry.key)
            });
            if matched {
                let key = override_key(EnchantCategory::Special, &entry.key);
                if seen.insert(key.clone()) {
                    triggers.push(EnchantTrigger {
                        custom_key: key,
                        volume: entry.volume,
                    });
                }
            }
        }
    }

    triggers
}

/// Convenience: does any category have at least one enabled entry? Used to
/// skip work entirely when nothing is configured.
pub fn any_enabled(settings: &EnchantmentSounds) -> bool {
    (settings.tiered.enabled && !settings.tiered.entries.is_empty())
        || (settings.special.enabled && !settings.special.entries.is_empty())
}

/// The default embedded sound for enchantment notifications.
pub const ENCHANT_SOUND: EventSound = EventSound::Enchant;

/// Base display name with any trailing roman-numeral tier (I-IV) removed, e.g.
/// "Loot Bonus III" -> "Loot Bonus". Names without a trailing numeral are
/// returned unchanged.
pub fn base_display_name(name: &str) -> String {
    match name.rsplit_once(' ') {
        Some((base, suffix)) if matches!(suffix, "I" | "II" | "III" | "IV") => base.to_string(),
        _ => name.to_string(),
    }
}

/// A tiered-category picker candidate: one enchant family.
#[derive(Debug, Clone)]
pub struct TieredCandidate {
    pub family: String,
    pub name: String,
    pub icon_id: u16,
}

/// Build the tiered picker candidates: one per enchant family that carries
/// tiers, using the lowest-tier member for the display name and icon. Families
/// already present in `exclude` (by family key) are skipped. Sorted by name.
pub fn tiered_candidates(
    catalog: &[EnchantCatalogEntry],
    exclude: &HashSet<String>,
) -> Vec<TieredCandidate> {
    let mut by_family: HashMap<String, &EnchantCatalogEntry> = HashMap::new();
    for e in catalog.iter().filter(|e| e.tier_num.is_some()) {
        by_family
            .entry(e.family_id.clone())
            .and_modify(|cur| {
                if e.tier_num < cur.tier_num {
                    *cur = e;
                }
            })
            .or_insert(e);
    }
    let mut out: Vec<TieredCandidate> = by_family
        .into_iter()
        .filter(|(fam, _)| !exclude.contains(fam))
        .map(|(fam, e)| TieredCandidate {
            family: fam,
            name: base_display_name(&e.display_name),
            icon_id: e.type_id,
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// A unique/awakened picker candidate: one specific enchant, grouped by
/// display name (unique enchants have one internal id per weapon type, all
/// sharing a display name).
#[derive(Debug, Clone)]
pub struct SpecialCandidate {
    /// Stable match key -- the display name (also the display label).
    pub key: String,
    pub name: String,
    pub icon_id: u16,
}

/// Build the unique/awakened picker candidates: one per Unique or Awakened
/// enchant *display name* (deduped across weapon variants), excluding names
/// already present in `exclude`. Sorted by name.
pub fn special_candidates(
    catalog: &[EnchantCatalogEntry],
    exclude: &HashSet<String>,
) -> Vec<SpecialCandidate> {
    let mut by_name: HashMap<String, &EnchantCatalogEntry> = HashMap::new();
    for e in catalog
        .iter()
        .filter(|e| e.tier_class.is_special())
        .filter(|e| e.rollable)
        .filter(|e| !e.display_name.contains("(Legacy)"))
    {
        by_name.entry(e.display_name.clone()).or_insert(e);
    }
    let mut out: Vec<SpecialCandidate> = by_name
        .into_iter()
        .filter(|(name, _)| !exclude.contains(name))
        .map(|(name, e)| SpecialCandidate {
            key: name.clone(),
            name,
            icon_id: e.type_id,
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Resolve the enchant type id for a specific family + tier, for the row icon.
/// Falls back to any member of the family, then 0 if unknown.
pub fn family_tier_icon(catalog: &[EnchantCatalogEntry], family: &str, tier: u8) -> u16 {
    let members: Vec<&EnchantCatalogEntry> =
        catalog.iter().filter(|e| e.family_id == family).collect();
    members
        .iter()
        .find(|e| e.tier_num == Some(tier))
        .or_else(|| members.first())
        .map(|e| e.type_id)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use realmhound_core::assets::EnchantmentTier;
    use realmhound_core::settings::{EnchantEntry, EnchantSection};

    fn entry(
        type_id: u16,
        name: &str,
        internal: &str,
        family: &str,
        tier: Option<u8>,
        class: EnchantmentTier,
    ) -> EnchantCatalogEntry {
        EnchantCatalogEntry {
            type_id,
            display_name: name.to_string(),
            internal_id: internal.to_string(),
            family_id: family.to_string(),
            tier_num: tier,
            tier_class: class,
            rollable: true,
        }
    }

    fn non_rollable(
        type_id: u16,
        name: &str,
        internal: &str,
        class: EnchantmentTier,
    ) -> EnchantCatalogEntry {
        EnchantCatalogEntry {
            type_id,
            display_name: name.to_string(),
            internal_id: internal.to_string(),
            family_id: internal.to_string(),
            tier_num: None,
            tier_class: class,
            rollable: false,
        }
    }

    fn catalog() -> Vec<EnchantCatalogEntry> {
        vec![
            entry(
                1,
                "Loot Bonus I",
                "Loot_Bonus_1",
                "LOOT_BONUS",
                Some(1),
                EnchantmentTier::Normal,
            ),
            entry(
                2,
                "Loot Bonus II",
                "Loot_Bonus_2",
                "LOOT_BONUS",
                Some(2),
                EnchantmentTier::Normal,
            ),
            entry(
                3,
                "Loot Bonus III",
                "Loot_Bonus_3",
                "LOOT_BONUS",
                Some(3),
                EnchantmentTier::HighLootBonus,
            ),
            entry(
                4,
                "Loot Bonus IV",
                "Loot_Bonus_4",
                "LOOT_BONUS",
                Some(4),
                EnchantmentTier::HighLootBonus,
            ),
            entry(
                5,
                "XP Loot Bonus III",
                "XP_Loot_Bonus_3",
                "XP_LOOT_BONUS",
                Some(3),
                EnchantmentTier::Normal,
            ),
            entry(
                9,
                "Flurry of Blows",
                "FLURRY_OF_BLOWS",
                "FLURRY_OF_BLOWS",
                None,
                EnchantmentTier::Unique,
            ),
            // Two weapon variants of the same unique share a display name.
            entry(
                20,
                "Adonis' Shot",
                "VALENTINES_ADONIS_SHOT_DAGGER",
                "VALENTINES_ADONIS_SHOT_DAGGER",
                None,
                EnchantmentTier::Unique,
            ),
            entry(
                21,
                "Adonis' Shot",
                "VALENTINES_ADONIS_SHOT_TACHI",
                "VALENTINES_ADONIS_SHOT_TACHI",
                None,
                EnchantmentTier::Unique,
            ),
        ]
    }

    fn tiered_entry(key: &str, tier: u8) -> EnchantEntry {
        EnchantEntry {
            key: key.to_string(),
            name: key.to_string(),
            tier: Some(tier),
            volume: 0.5,
            icon_id: 0,
        }
    }

    fn special_entry(key: &str) -> EnchantEntry {
        EnchantEntry {
            key: key.to_string(),
            name: key.to_string(),
            tier: None,
            volume: 0.5,
            icon_id: 0,
        }
    }
    fn settings(
        tiered_on: bool,
        tiered: Vec<EnchantEntry>,
        special_on: bool,
        special: Vec<EnchantEntry>,
    ) -> EnchantmentSounds {
        EnchantmentSounds {
            tiered: EnchantSection {
                enabled: tiered_on,
                default_volume: 0.5,
                entries: tiered,
            },
            special: EnchantSection {
                enabled: special_on,
                default_volume: 0.5,
                entries: special,
            },
        }
    }

    #[test]
    fn tiered_matches_selected_tier_and_above() {
        let s = settings(true, vec![tiered_entry("LOOT_BONUS", 3)], false, vec![]);
        // Tier III drop matches.
        assert_eq!(resolve(&s, &[3], &catalog()).len(), 1);
        // Tier IV drop matches (above threshold).
        assert_eq!(resolve(&s, &[4], &catalog()).len(), 1);
        // Tier II drop does not match.
        assert!(resolve(&s, &[2], &catalog()).is_empty());
    }

    #[test]
    fn tiered_does_not_cross_families() {
        let s = settings(true, vec![tiered_entry("LOOT_BONUS", 3)], false, vec![]);
        // XP Loot Bonus III is a different family, no match.
        assert!(resolve(&s, &[5], &catalog()).is_empty());
    }

    #[test]
    fn disabled_category_is_silent() {
        let s = settings(false, vec![tiered_entry("LOOT_BONUS", 3)], false, vec![]);
        assert!(resolve(&s, &[3, 4], &catalog()).is_empty());
    }

    #[test]
    fn special_matches_legacy_internal_id_key() {
        // Settings saved before the display-name migration store the internal id.
        let s = settings(false, vec![], true, vec![special_entry("FLURRY_OF_BLOWS")]);
        let t = resolve(&s, &[9], &catalog());
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn special_matches_exact_display_name() {
        let s = settings(false, vec![], true, vec![special_entry("Flurry of Blows")]);
        let t = resolve(&s, &[9], &catalog());
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].custom_key, "enchant:special:Flurry of Blows");
    }

    #[test]
    fn special_matches_any_weapon_variant_and_dedups() {
        // Both Adonis' Shot variants (ids 20, 21) share a display name; a bag
        // with either (or both) produces exactly one trigger.
        let s = settings(false, vec![], true, vec![special_entry("Adonis' Shot")]);
        assert_eq!(resolve(&s, &[20], &catalog()).len(), 1);
        assert_eq!(resolve(&s, &[21], &catalog()).len(), 1);
        assert_eq!(resolve(&s, &[20, 21], &catalog()).len(), 1);
    }

    #[test]
    fn dedup_by_key_within_a_bag() {
        // Two Loot Bonus IIIs in one bag produce a single trigger.
        let s = settings(true, vec![tiered_entry("LOOT_BONUS", 3)], false, vec![]);
        assert_eq!(resolve(&s, &[3, 4], &catalog()).len(), 1);
    }

    #[test]
    fn both_categories_can_trigger() {
        let s = settings(
            true,
            vec![tiered_entry("LOOT_BONUS", 3)],
            true,
            vec![special_entry("Flurry of Blows")],
        );
        let t = resolve(&s, &[3, 9], &catalog());
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn unknown_and_out_of_range_ids_ignored() {
        let s = settings(true, vec![tiered_entry("LOOT_BONUS", 3)], false, vec![]);
        // 999 not in catalog; -1 not a valid u16.
        assert!(resolve(&s, &[999, -1], &catalog()).is_empty());
    }

    #[test]
    fn base_display_name_strips_roman_numerals() {
        assert_eq!(base_display_name("Loot Bonus III"), "Loot Bonus");
        assert_eq!(base_display_name("Loot Bonus IV"), "Loot Bonus");
        assert_eq!(base_display_name("Flurry of Blows"), "Flurry of Blows");
        assert_eq!(base_display_name("Attack Bonus I"), "Attack Bonus");
    }

    #[test]
    fn tiered_candidates_dedup_by_family() {
        let cands = tiered_candidates(&catalog(), &HashSet::new());
        // LOOT_BONUS and XP_LOOT_BONUS families (Flurry has no tier).
        assert_eq!(cands.len(), 2);
        let loot = cands.iter().find(|c| c.family == "LOOT_BONUS").unwrap();
        assert_eq!(loot.name, "Loot Bonus");
        // Lowest tier (I) member used for the icon.
        assert_eq!(loot.icon_id, 1);
    }

    #[test]
    fn tiered_candidates_respect_exclude() {
        let mut ex = HashSet::new();
        ex.insert("LOOT_BONUS".to_string());
        let cands = tiered_candidates(&catalog(), &ex);
        assert!(cands.iter().all(|c| c.family != "LOOT_BONUS"));
    }

    #[test]
    fn special_candidates_dedup_by_display_name() {
        let cands = special_candidates(&catalog(), &HashSet::new());
        // Flurry of Blows + Adonis' Shot (two variants collapse to one).
        assert_eq!(cands.len(), 2);
        assert!(cands.iter().any(|c| c.key == "Flurry of Blows"));
        let adonis: Vec<_> = cands.iter().filter(|c| c.key == "Adonis' Shot").collect();
        assert_eq!(adonis.len(), 1);
    }

    #[test]
    fn special_candidates_exclude_non_rollable() {
        let mut cat = catalog();
        // Engraving-only enchants (no ROLLABLE label) must not be offered.
        cat.push(non_rollable(
            40,
            "Adonis' Shot",
            "VALENTINES_ADONIS_SHOT_DAGGER",
            EnchantmentTier::Unique,
        ));
        cat.push(non_rollable(
            41,
            "Path of the Magus",
            "PATH_OF_THE_MAGUS",
            EnchantmentTier::Unique,
        ));
        cat.push(non_rollable(
            42,
            "Retrowinds Weapon",
            "RETROWINDS_WEAPON",
            EnchantmentTier::Unique,
        ));
        let cands = special_candidates(&cat, &HashSet::new());
        assert!(cands.iter().all(|c| c.key != "Path of the Magus"));
        assert!(cands.iter().all(|c| c.key != "Retrowinds Weapon"));
        // Rollable uniques remain.
        assert!(cands.iter().any(|c| c.key == "Flurry of Blows"));
    }

    #[test]
    fn special_candidates_exclude_legacy_suffix() {
        let mut cat = catalog();
        cat.push(entry(
            30,
            "Buzzing Bullets (Legacy)",
            "BUZZING_BULLETS_LEGACY",
            "BUZZING_BULLETS_LEGACY",
            None,
            EnchantmentTier::Unique,
        ));
        cat.push(entry(
            31,
            "El Dorado's Legacy",
            "EL_DORADOS_LEGACY",
            "EL_DORADOS_LEGACY",
            None,
            EnchantmentTier::Unique,
        ));
        let cands = special_candidates(&cat, &HashSet::new());
        assert!(cands.iter().all(|c| c.key != "Buzzing Bullets (Legacy)"));
        assert!(cands.iter().any(|c| c.key == "El Dorado's Legacy"));
    }

    #[test]
    fn family_tier_icon_resolves_tier() {
        assert_eq!(family_tier_icon(&catalog(), "LOOT_BONUS", 3), 3);
        assert_eq!(family_tier_icon(&catalog(), "LOOT_BONUS", 4), 4);
    }
}

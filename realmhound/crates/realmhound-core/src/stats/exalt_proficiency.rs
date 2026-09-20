//! Account-wide exaltation proficiency math for the in-game "Exaltations"
//! menu.
//!
//! Each of the account's classes has 8 stats, each at an exalt level 0-5
//! (derived from raw completion counts via `ClassExaltation::raw_to_exalt_level`).
//! From those per-class levels we derive four proficiency categories that match
//! the in-game menu:
//!
//! - Fast Learner (XP):  per class, `min(sum_of_8_levels / 8, 4)` -> +level*5% XP.
//! - Mastery (DMG):      per class, `min(min_of_8_levels, 4)`     -> +level*2.5% DMG.
//! - Armor Proficiency:  min level across every class sharing the armor type
//!                       AND all 8 stats (0-5) -> -level*0.2s in-combat.
//! - Weapon Proficiency: min level across every class sharing the weapon type
//!                       AND all 8 stats (0-5) -> +level*5% drop rate.
//!
//! Classes with no exalt data are treated as all-zero (level 0), matching the
//! game (a proficiency stays at 0 until every class in the group has progress).

use std::collections::HashMap;

use crate::api::ClassExaltation;
use crate::loot::class_ids;

/// Maximum total exalt levels for a single class (8 stats * 5 levels).
pub const MAX_CLASS_LEVELS: u32 = 40;

/// The per-stat exalt levels (0-5) of a class, in display order
/// (ATT, DEF, SPD, DEX, VIT, WIS, HP, MP).
pub fn stat_levels(exalt: &ClassExaltation) -> [u8; 8] {
    [
        ClassExaltation::raw_to_exalt_level(exalt.attack),
        ClassExaltation::raw_to_exalt_level(exalt.defense),
        ClassExaltation::raw_to_exalt_level(exalt.speed),
        ClassExaltation::raw_to_exalt_level(exalt.dexterity),
        ClassExaltation::raw_to_exalt_level(exalt.vitality),
        ClassExaltation::raw_to_exalt_level(exalt.wisdom),
        ClassExaltation::raw_to_exalt_level(exalt.hp),
        ClassExaltation::raw_to_exalt_level(exalt.mp),
    ]
}

/// Sum of a class's 8 stat exalt levels (0-40).
pub fn class_total_levels(exalt: &ClassExaltation) -> u32 {
    stat_levels(exalt).iter().map(|&l| l as u32).sum()
}

/// Fast Learner level (0-4): each 8 exalt levels fills one quarter.
pub fn fast_learner_level(exalt: &ClassExaltation) -> u8 {
    ((class_total_levels(exalt) / 8).min(4)) as u8
}

/// Fast Learner XP bonus percent (0/5/10/15/20).
pub fn fast_learner_xp_pct(exalt: &ClassExaltation) -> u32 {
    fast_learner_level(exalt) as u32 * 5
}

/// Mastery level (0-4): the minimum stat level, capped at 4.
pub fn mastery_level(exalt: &ClassExaltation) -> u8 {
    let min = stat_levels(exalt).iter().copied().min().unwrap_or(0);
    min.min(4)
}

/// Mastery damage bonus, in tenths of a percent (0/25/50/75/100 = 0-10.0%).
/// Returned in tenths to avoid floats (e.g. 75 -> "7.5%").
pub fn mastery_dmg_tenths(exalt: &ClassExaltation) -> u32 {
    mastery_level(exalt) as u32 * 25
}

/// Minimum exalt level (0-5) across every class in `group` and all 8 stats.
/// A class missing from `stats` contributes level 0.
pub fn group_min_level(stats: &HashMap<i32, ClassExaltation>, group: &[i32]) -> u8 {
    group
        .iter()
        .map(|class_id| match stats.get(class_id) {
            Some(exalt) => stat_levels(exalt).iter().copied().min().unwrap_or(0),
            None => 0,
        })
        .min()
        .unwrap_or(0)
}

/// Armor Proficiency level (0-5) for the given class's armor group.
pub fn armor_proficiency_level(stats: &HashMap<i32, ClassExaltation>, class_id: i32) -> u8 {
    group_min_level(stats, &class_ids::armor_classes(class_id))
}

/// Weapon Proficiency level (0-5) for the given class's weapon group.
pub fn weapon_proficiency_level(stats: &HashMap<i32, ClassExaltation>, class_id: i32) -> u8 {
    group_min_level(stats, &class_ids::weapon_classes(class_id))
}

/// Armor Proficiency in-combat reduction, in tenths of a second
/// (0/2/4/6/8/10 = 0-1.0s).
pub fn armor_ic_tenths(stats: &HashMap<i32, ClassExaltation>, class_id: i32) -> u32 {
    armor_proficiency_level(stats, class_id) as u32 * 2
}

/// Weapon Proficiency drop-rate bonus percent (0/5/10/15/20/25).
pub fn weapon_dr_pct(stats: &HashMap<i32, ClassExaltation>, class_id: i32) -> u32 {
    weapon_proficiency_level(stats, class_id) as u32 * 5
}

/// Total account exaltation progress as `(current, max)` where `max` is
/// `40 * number_of_canonical_classes`. Only canonical classes are counted, so
/// stray map entries can't inflate progress.
pub fn total_progress(stats: &HashMap<i32, ClassExaltation>) -> (u32, u32) {
    let current: u32 = class_ids::ALL
        .iter()
        .map(|id| stats.get(id).map(class_total_levels).unwrap_or(0))
        .sum();
    let max = MAX_CLASS_LEVELS * class_ids::ALL.len() as u32;
    (current, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn knight() -> ClassExaltation {
        // Raw completions chosen so levels are ATT4 DEF4 SPD3 DEX4 VIT5 WIS5 HP5 MP4
        // (matches the in-game screenshot).
        // Level thresholds: 5->1, 15->2, 30->3, 50->4, 75->5.
        ClassExaltation {
            class_type: class_ids::KNIGHT,
            attack: 50,    // 4
            defense: 50,   // 4
            speed: 30,     // 3
            dexterity: 50, // 4
            vitality: 75,  // 5
            wisdom: 75,    // 5
            hp: 75,        // 5
            mp: 50,        // 4
        }
    }

    #[test]
    fn knight_stat_levels_match_screenshot() {
        assert_eq!(stat_levels(&knight()), [4, 4, 3, 4, 5, 5, 5, 4]);
        assert_eq!(class_total_levels(&knight()), 34);
    }

    #[test]
    fn knight_fast_learner_is_20pct() {
        // sum 34 / 8 = 4 (capped) -> +20% XP
        assert_eq!(fast_learner_level(&knight()), 4);
        assert_eq!(fast_learner_xp_pct(&knight()), 20);
    }

    #[test]
    fn knight_mastery_is_7_5pct() {
        // min level = 3 (SPD) -> +7.5% DMG
        assert_eq!(mastery_level(&knight()), 3);
        assert_eq!(mastery_dmg_tenths(&knight()), 75);
    }

    #[test]
    fn fully_exalted_class_caps_fast_learner_at_4() {
        let maxed = ClassExaltation {
            class_type: class_ids::WARRIOR,
            attack: 75,
            defense: 75,
            speed: 75,
            dexterity: 75,
            vitality: 75,
            wisdom: 75,
            hp: 75,
            mp: 75,
        };
        // sum 40 / 8 = 5, must cap at 4.
        assert_eq!(fast_learner_level(&maxed), 4);
        assert_eq!(mastery_level(&maxed), 4);
    }

    #[test]
    fn group_min_zero_when_a_class_is_absent() {
        let mut stats = HashMap::new();
        // Only Warrior present and maxed; Knight/Paladin absent -> group min 0.
        stats.insert(
            class_ids::WARRIOR,
            ClassExaltation {
                class_type: class_ids::WARRIOR,
                attack: 75,
                defense: 75,
                speed: 75,
                dexterity: 75,
                vitality: 75,
                wisdom: 75,
                hp: 75,
                mp: 75,
            },
        );
        assert_eq!(weapon_proficiency_level(&stats, class_ids::WARRIOR), 0);
        assert_eq!(weapon_dr_pct(&stats, class_ids::WARRIOR), 0);
    }

    #[test]
    fn group_min_uses_weakest_stat_across_group() {
        let full = |c| ClassExaltation {
            class_type: c,
            attack: 75,
            defense: 75,
            speed: 75,
            dexterity: 75,
            vitality: 75,
            wisdom: 75,
            hp: 75,
            mp: 75,
        };
        let mut stats = HashMap::new();
        // Sword group: Warrior, Knight, Paladin all maxed -> level 5.
        stats.insert(class_ids::WARRIOR, full(class_ids::WARRIOR));
        stats.insert(class_ids::KNIGHT, full(class_ids::KNIGHT));
        stats.insert(class_ids::PALADIN, full(class_ids::PALADIN));
        assert_eq!(weapon_proficiency_level(&stats, class_ids::KNIGHT), 5);

        // Drop one stat on Paladin to level 2 (raw 15) -> group min becomes 2.
        stats.get_mut(&class_ids::PALADIN).unwrap().speed = 15;
        assert_eq!(weapon_proficiency_level(&stats, class_ids::KNIGHT), 2);
    }

    #[test]
    fn wand_group_includes_druid() {
        let full = |c| ClassExaltation {
            class_type: c,
            attack: 75,
            defense: 75,
            speed: 75,
            dexterity: 75,
            vitality: 75,
            wisdom: 75,
            hp: 75,
            mp: 75,
        };
        let mut stats = HashMap::new();
        // Priest, Sorcerer, Summoner maxed but Druid absent -> Wand proficiency 0.
        stats.insert(class_ids::PRIEST, full(class_ids::PRIEST));
        stats.insert(class_ids::SORCERER, full(class_ids::SORCERER));
        stats.insert(class_ids::SUMMONER, full(class_ids::SUMMONER));
        assert_eq!(weapon_proficiency_level(&stats, class_ids::PRIEST), 0);

        // Add a maxed Druid -> now all 4 wand classes maxed -> level 5.
        stats.insert(class_ids::DRUID, full(class_ids::DRUID));
        assert_eq!(weapon_proficiency_level(&stats, class_ids::PRIEST), 5);
    }

    #[test]
    fn armor_group_heavy_includes_samurai_and_kensei() {
        let group = class_ids::armor_classes(class_ids::KNIGHT);
        for c in [
            class_ids::WARRIOR,
            class_ids::KNIGHT,
            class_ids::PALADIN,
            class_ids::SAMURAI,
            class_ids::KENSEI,
        ] {
            assert!(group.contains(&c), "Heavy group should contain {c}");
        }
        assert_eq!(group.len(), 5);
    }

    #[test]
    fn total_progress_denominator_is_760() {
        let (_current, max) = total_progress(&HashMap::new());
        assert_eq!(max, 760); // 19 classes * 40
    }

    #[test]
    fn total_progress_counts_only_canonical_classes() {
        let full = |c| ClassExaltation {
            class_type: c,
            attack: 75,
            defense: 75,
            speed: 75,
            dexterity: 75,
            vitality: 75,
            wisdom: 75,
            hp: 75,
            mp: 75,
        };
        let mut stats = HashMap::new();
        stats.insert(class_ids::KNIGHT, full(class_ids::KNIGHT)); // 40
        stats.insert(9999, full(9999)); // non-canonical, must be ignored
        let (current, max) = total_progress(&stats);
        assert_eq!(current, 40);
        assert_eq!(max, 760);
    }

    // Regression test: a fully-exalted Ninja was displayed with
    // MP/DEF (and other stats) below 5/5 because raw progress was truncated to
    // u8. These are FutuRe's real Ninja completion counts from packet captures,
    // several of which exceed 255. With u32 storage every stat resolves to 5/5.
    // CSV order is DEX,SPD,VIT,WIS,DEF,ATT,MP,HP.
    const FUTURE_NINJA_CSV: &str = "1719,307,121,706,118,185,298,233";

    #[test]
    fn future_fully_exalted_ninja_from_csv_is_all_level_5() {
        let ninja = ClassExaltation::from_csv(class_ids::NINJA, FUTURE_NINJA_CSV).unwrap();
        assert_eq!(stat_levels(&ninja), [5; 8]);
        assert_eq!(class_total_levels(&ninja), 40);
    }

    #[test]
    fn future_fully_exalted_ninja_from_packet_is_all_level_5() {
        // Packet order matches CSV: DEX, SPD, VIT, WIS, DEF, ATT, MP, HP.
        let ninja = ClassExaltation::from_packet(
            class_ids::NINJA as i16,
            1719,
            307,
            121,
            706,
            118,
            185,
            298,
            233,
        );
        assert_eq!(stat_levels(&ninja), [5; 8]);
        assert_eq!(class_total_levels(&ninja), 40);
    }
}

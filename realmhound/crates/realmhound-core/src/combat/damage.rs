//! Local-player damage simulation.
//!
//! The local player's own shots carry no damage value in any packet, so the
//! exact damage must be reproduced the way the game client computes it. This is
//! a faithful port of the game's deterministic damage model:
//!
//! - A Park-Miller LCG ([`Rng`]) is seeded once per map from `MapInfo.fp` (our
//!   `map_seed`) and advanced exactly once per `PlayerShoot`, but only when the
//!   fired projectile's `min != max` (fixed-damage projectiles do not advance it).
//! - The rolled base is scaled by the attack-stat multiplier for main weapons.
//! - At hit time the target's live DEF + condition bits reduce it
//!   ([`damage_with_defense`]).
//!
//! Every arithmetic step mirrors the reference integer/float truncation exactly so
//! the sequence stays in lockstep with the client.

use crate::assets::LethalStrikeParams;

/// Weapon slot types that receive the attack-stat damage multiplier (main-hand
/// weapons). Ability projectiles use a flat 1.0 multiplier. Values match the
/// RotMG slot-type numbering (Sword, Dagger, Bow, Wand, Staff, Katana).
const MAIN_WEAPON_SLOTS: [i32; 6] = [1, 2, 3, 8, 17, 24];

// Condition bits (primary condition stat, id 29).
const COND_ARMORED: i32 = 0x0200_0000;
const COND_ARMORBROKEN: i32 = 0x0400_0000;
const COND_INVULNERABLE: i32 = 0x0100_0000;

// Condition bits (secondary "new condition" stat, id 69).
const NEWCON_EXPOSED: i32 = 0x0002_0000;
const NEWCON_PETRIFIED: i32 = 0x0000_0008;
const NEWCON_CURSED: i32 = 0x0000_0040;

// Player condition bits affecting outgoing damage (primary condition stat).
const COND_WEAK: i32 = 0x0000_0004;
const COND_DAMAGING: i32 = 0x0004_0000;

/// Default `ExaltationBonusDamage` stat value (per-mille); 1000 => x1.0.
pub const DEFAULT_EXALT_BONUS: i32 = 1000;

/// Park-Miller multiplicative LCG matching the game's RNG.
///
/// `next()` reproduces the reference 64-bit (`long`) arithmetic bit-for-bit (including the
/// `(int)` truncation and unsigned widening), so a Rust replay stays in lockstep
/// with the game client as long as it is seeded once per map and advanced once
/// per variable-damage shot in packet order.
#[derive(Debug, Clone)]
pub struct Rng {
    seed: i64,
}

impl Rng {
    /// Seed the generator with the map seed (`MapInfo.fp`, a signed 32-bit value
    /// widened with sign extension, exactly as a 32-bit `int` widens to a 64-bit `long`).
    pub fn new(seed: i32) -> Self {
        Self { seed: seed as i64 }
    }

    /// Advance the generator and return the next value in `[0, 2^32 - 1]`.
    pub fn next(&mut self) -> i64 {
        let seed = self.seed;
        let right16 = (seed >> 16).wrapping_mul(0x41A7);
        let left = (seed & 0xFFFF).wrapping_mul(0x41A7);
        let right15 = right16 >> 15;
        let right_and = (right16 & 0x7FFF) << 16;
        let sum = left.wrapping_add(right_and).wrapping_add(right15);

        let final_value = if sum <= 0x7FFF_FFFF {
            sum
        } else {
            // Integer.toUnsignedLong((int) sum - 0x7FFFFFFF)
            let sub = (sum as i32).wrapping_sub(0x7FFF_FFFF);
            (sub as u32) as i64
        };
        self.seed = final_value;
        final_value
    }
}

/// The local player's outgoing-damage-relevant stats, snapshotted at shoot time.
#[derive(Debug, Clone, Copy)]
pub struct AttackerStats {
    /// Effective Attack: base Attack (id 20) plus AttackBoost (id 48). The
    /// multiplier uses the *total* attack the client applies, so any gear /
    /// buff / pet ATT bonus tracked as AttackBoost must be folded in here or the
    /// self-computed damage falls short of what the game rolled.
    pub attack: i32,
    /// Primary condition bitmask (id 29): Weak / Damaging.
    pub condition: i32,
    /// Raw `ExaltationBonusDamage` (id 113), per-mille; [`DEFAULT_EXALT_BONUS`]
    /// when unknown.
    pub exalt_bonus: i32,
}

impl Default for AttackerStats {
    fn default() -> Self {
        Self {
            attack: 0,
            condition: 0,
            exalt_bonus: DEFAULT_EXALT_BONUS,
        }
    }
}

/// Whether a weapon slot type gets the attack-stat multiplier.
pub fn is_main_weapon(slot_type: i32) -> bool {
    MAIN_WEAPON_SLOTS.contains(&slot_type)
}

/// The attack-stat damage multiplier for a main weapon (32-bit `float` math).
fn attack_multiplier(atk: &AttackerStats) -> f32 {
    if atk.condition & COND_WEAK != 0 {
        return 0.5;
    }
    let mut number = (atk.attack + 25) as f32 * 0.02;
    if atk.condition & COND_DAMAGING != 0 {
        number *= 1.25;
    }
    number * (atk.exalt_bonus as f32 / 1000.0)
}

/// Roll a shot's pre-defense damage.
///
/// `min_mult` / `max_mult` are the products of the weapon's `MultiplyMinDamage`
/// / `MultiplyMaxDamage` enchant mutators (`1.0` when none). To stay in lockstep
/// with the client's per-map RNG, the decision to consume an RNG step is made on
/// the *original* `min != max` (never on the scaled endpoints), so an enchanted
/// weapon advances the generator exactly as the unscaled roll would. The random
/// value is then spread across the enchant-scaled endpoints, and the attack-stat
/// multiplier is applied for main-weapon slots. Passing `(1.0, 1.0)` reproduces
/// the unenchanted roll bit-for-bit. Armor-piercing is carried through to
/// [`damage_with_defense`].
pub fn roll_base_damage(
    rng: &mut Rng,
    min: i32,
    max: i32,
    min_mult: f32,
    max_mult: f32,
    slot_type: i32,
    atk: &AttackerStats,
) -> i32 {
    let emin = (min as f32 * min_mult) as i32;
    let emax = (max as f32 * max_mult) as i32;
    let mut dmg = emin;
    if min != max {
        let r = rng.next();
        let span = (emax - emin) as i64;
        if span > 0 {
            dmg = emin + (r % span) as i32;
        }
        // A malformed (or enchant-collapsed) max <= min still consumes one RNG
        // step, matching the unscaled roll, and falls back to the scaled minimum.
    }
    if is_main_weapon(slot_type) {
        dmg = (dmg as f32 * attack_multiplier(atk)) as i32;
    }
    dmg
}

/// Reduce a shot's pre-defense damage by the target's defense and condition
/// effects (64-bit `double` math).
///
/// `cond0` is the target's primary condition stat (id 29) and `cond1` the
/// secondary "new condition" stat (id 69).
pub fn damage_with_defense(
    damage: i32,
    armor_piercing: bool,
    defence: i32,
    cond0: i32,
    cond1: i32,
) -> i32 {
    if damage == 0 {
        return 0;
    }

    let mut defence = if armor_piercing || cond0 & COND_ARMORBROKEN != 0 {
        0
    } else if cond0 & COND_ARMORED != 0 {
        (defence as f64 * 1.5) as i32
    } else {
        defence
    };
    if cond1 & NEWCON_EXPOSED != 0 {
        defence -= 20;
    }

    let floor = (damage * 2) / 20;
    let mut dmg = floor.max(damage - defence);

    if cond0 & COND_INVULNERABLE != 0 {
        dmg = 0;
    }
    if cond1 & NEWCON_PETRIFIED != 0 {
        dmg = (dmg as f64 * 0.9) as i32;
    }
    if cond1 & NEWCON_CURSED != 0 {
        dmg = (dmg as f64 * 1.25) as i32;
    }
    dmg
}

/// Armor of Nil "Planar Absorption" post-defense reduction (15%).
pub const ARMOR_OF_NIL_REDUCTION_PCT: u8 = 15;

/// Post-defense percentage damage reductions the local player has active at hit
/// time. These are applied AFTER the defense stage (and after the
/// Cursed/Petrified modifiers in [`damage_with_defense`]), stacking
/// multiplicatively, mirroring how the game layers each protection on top of the
/// post-armor figure.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DamageReductions {
    /// Damage Resistance armor enchant: 2/3/4/5% for tiers I-IV. Percent (0-100).
    pub enchant_pct: u8,
    /// Whether Armor of Nil's Planar Absorption window is active this hit (15%).
    pub armor_of_nil: bool,
    /// Knight shield reduction while its post-activation window is active: 25%,
    /// or 30% for the Kogbold Cower Shield. Percent (0-100).
    pub shield_pct: u8,
}

impl DamageReductions {
    /// Whether any post-defense reduction is active.
    pub fn is_active(&self) -> bool {
        self.enchant_pct > 0 || self.armor_of_nil || self.shield_pct > 0
    }
}

/// Map a Damage Resistance armor enchant id to its percentage:
/// 1556 => 2% (I), 1557 => 3% (II), 1558 => 4% (III), 1559 => 5% (IV).
pub fn damage_resistance_pct(enchant_id: u16) -> u8 {
    match enchant_id {
        1556 => 2,
        1557 => 3,
        1558 => 4,
        1559 => 5,
        _ => 0,
    }
}

/// Apply the local player's active post-defense percentage reductions to an
/// already-post-defense damage figure. Each reduction multiplies the running
/// value and the client truncates to an integer, so they compound in the fixed
/// order enchant -> Armor of Nil -> shield.
pub fn apply_post_defense_reductions(after_defense: i32, r: &DamageReductions) -> i32 {
    if after_defense <= 0 || !r.is_active() {
        return after_defense.max(0);
    }
    let mut dmg = after_defense as f64;
    if r.enchant_pct > 0 {
        dmg *= 1.0 - r.enchant_pct.min(100) as f64 / 100.0;
    }
    if r.armor_of_nil {
        dmg *= 1.0 - ARMOR_OF_NIL_REDUCTION_PCT as f64 / 100.0;
    }
    if r.shield_pct > 0 {
        dmg *= 1.0 - r.shield_pct.min(100) as f64 / 100.0;
    }
    dmg as i32
}

/// Resolve the local player's damage taken from a single enemy projectile, and
/// how much of the pre-defense damage was negated by all protection. Returns `(taken, blocked)` where `taken` is the final HP lost after
/// defense (incl. Armored / Invulnerable) plus every post-defense reduction, and
/// `blocked = max(0, base - taken)` is the "Mell stat" -- damage the player's
/// defense and other protections absorbed. `blocked` is clamped so a damage
/// amplifier (Cursed) never reports negative absorption.
pub fn damage_taken_and_blocked(
    base: i32,
    armor_piercing: bool,
    defence: i32,
    cond0: i32,
    cond1: i32,
    reductions: &DamageReductions,
) -> (i32, i32) {
    let after_defense = damage_with_defense(base, armor_piercing, defence, cond0, cond1);
    let taken = apply_post_defense_reductions(after_defense, reductions);
    let blocked = (base - taken).max(0);
    (taken, blocked)
}

/// Bonus damage a single shot deals under the rogue Lethal Strike buff, vs a
/// target with base `defense`, at the player's `effective_wis` (the Wisdom stat
/// id 27, which the server already reports with WisdomBoost folded in). Matches
/// the in-game wiki formula:
///
/// ```text
/// w    = max(0, effective_wis - stat_mod_scaling_min)   // e.g. WIS - 34
/// flat = ignore_flat + stat_mod_flat * w
/// perc = ignore_perc + stat_mod_perc * w
/// bonus = flat + perc * defense
/// ```
///
/// This is both the extra damage added to a normal shot and the full damage of
/// each of the two side "proc" projectiles ("your Lethal Strike damage"). The
/// result is floored to an integer and never negative.
pub fn lethal_strike_bonus(params: &LethalStrikeParams, effective_stat: i32, defense: i32) -> i64 {
    let w = (effective_stat as f32 - params.stat_mod_scaling_min).max(0.0);
    let flat = params.ignore_flat + params.stat_mod_flat * w;
    let perc = params.ignore_perc + params.stat_mod_perc * w;
    let bonus = flat + perc * defense.max(0) as f32;
    bonus.max(0.0).floor() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::ScalingStat;

    /// Reference sequence produced by the reference `RNG` for seed 12345, computed
    /// independently with a faithful emulation of the 64-bit/32-bit integer
    /// semantics. The generator must match exactly or the whole replay desyncs.
    #[test]
    fn rng_matches_reference_sequence() {
        let mut rng = Rng::new(12345);
        let expected = [207482415i64, 1790989824, 2035175616, 77048696, 24794531];
        for (i, &e) in expected.iter().enumerate() {
            assert_eq!(rng.next(), e, "mismatch at index {i}");
        }
    }

    /// Seed 1 reproduces the textbook Park-Miller (minstd) sequence, a strong
    /// cross-check that the arithmetic is the standard 16807 LCG.
    #[test]
    fn rng_matches_minstd_for_seed_one() {
        let mut rng = Rng::new(1);
        let expected = [16807i64, 282475249, 1622650073, 984943658, 1144108930];
        for (i, &e) in expected.iter().enumerate() {
            assert_eq!(rng.next(), e, "mismatch at index {i}");
        }
    }

    /// Negative map seeds sign-extend like a 32-bit `int` to 64-bit `long`; verified against
    /// the independent emulator (seed -42).
    #[test]
    fn rng_matches_reference_for_negative_seed() {
        let mut rng = Rng::new(-42);
        let expected = [2146777753i64, 1020941424, 568173638, 1582039304, 1339548821];
        for (i, &e) in expected.iter().enumerate() {
            assert_eq!(rng.next(), e, "mismatch at index {i}");
        }
    }

    #[test]
    fn rng_is_deterministic_per_seed() {
        let mut a = Rng::new(999);
        let mut b = Rng::new(999);
        for _ in 0..50 {
            assert_eq!(a.next(), b.next());
        }
    }

    #[test]
    fn fixed_damage_does_not_advance_rng() {
        let mut rng = Rng::new(555);
        let before = rng.clone();
        // min == max: no roll, RNG untouched.
        let dmg = roll_base_damage(&mut rng, 100, 100, 1.0, 1.0, 1, &AttackerStats::default());
        assert_eq!(rng.next(), before.clone().next());
        // With default (0 attack) main-weapon multiplier: (0+25)*0.02 = 0.5.
        assert_eq!(dmg, 50);
    }

    #[test]
    fn variable_damage_advances_rng_once() {
        let mut rng = Rng::new(777);
        let mut reference = rng.clone();
        let _ = roll_base_damage(&mut rng, 50, 150, 1.0, 1.0, 8, &AttackerStats::default());
        reference.next(); // one step consumed
        assert_eq!(rng.next(), reference.next());
    }

    #[test]
    fn ability_slot_skips_attack_multiplier() {
        let mut rng = Rng::new(1);
        // Non-main slot (e.g. 10): damage is the raw roll with no multiplier.
        let atk = AttackerStats {
            attack: 75,
            condition: 0,
            exalt_bonus: 1000,
        };
        let dmg = roll_base_damage(&mut rng, 200, 200, 1.0, 1.0, 10, &atk);
        assert_eq!(dmg, 200);
    }

    #[test]
    fn attack_multiplier_scales_main_weapon() {
        // attack 75 => (75+25)*0.02 = 2.0 multiplier on a fixed 100-damage shot.
        let mut rng = Rng::new(1);
        let atk = AttackerStats {
            attack: 75,
            condition: 0,
            exalt_bonus: 1000,
        };
        let dmg = roll_base_damage(&mut rng, 100, 100, 1.0, 1.0, 1, &atk);
        assert_eq!(dmg, 200);
    }

    #[test]
    fn weak_condition_halves_damage() {
        let mut rng = Rng::new(1);
        let atk = AttackerStats {
            attack: 75,
            condition: COND_WEAK,
            exalt_bonus: 1000,
        };
        let dmg = roll_base_damage(&mut rng, 100, 100, 1.0, 1.0, 1, &atk);
        assert_eq!(dmg, 50); // flat 0.5 regardless of attack
    }

    #[test]
    fn damaging_condition_boosts_damage() {
        let mut rng = Rng::new(1);
        // attack 75 => 2.0, *1.25 (damaging) = 2.5 on 100 => 250.
        let atk = AttackerStats {
            attack: 75,
            condition: COND_DAMAGING,
            exalt_bonus: 1000,
        };
        let dmg = roll_base_damage(&mut rng, 100, 100, 1.0, 1.0, 1, &atk);
        assert_eq!(dmg, 250);
    }

    #[test]
    fn enchant_scales_fixed_damage_without_advancing_rng() {
        let mut rng = Rng::new(555);
        let before = rng.clone();
        // min == max: no roll, RNG untouched even with asymmetric enchant mults.
        // Ability slot (10) skips the attack multiplier, so the result is purely
        // the scaled minimum. 100 * 1.05f32 = 104.999.. which truncates to 104,
        // matching the client's float truncation.
        let dmg = roll_base_damage(
            &mut rng,
            100,
            100,
            1.05,
            1.20,
            10,
            &AttackerStats::default(),
        );
        assert_eq!(rng.next(), before.clone().next());
        assert_eq!(dmg, 104);
    }

    #[test]
    fn enchant_variable_advances_rng_once_and_stays_in_scaled_range() {
        let mut rng = Rng::new(777);
        let mut reference = rng.clone();
        // Ability slot to isolate the enchant scaling from the attack multiplier.
        let dmg = roll_base_damage(
            &mut rng,
            100,
            200,
            1.05,
            1.05,
            10,
            &AttackerStats::default(),
        );
        reference.next(); // exactly one step consumed, same as the unscaled roll
        assert_eq!(rng.next(), reference.next());
        // Scaled endpoints: [105, 210); the roll must fall within.
        assert!((105..210).contains(&dmg), "dmg {dmg} outside scaled range");
    }

    #[test]
    fn unit_enchant_mults_match_unscaled_roll() {
        // (1.0, 1.0) must reproduce the plain roll bit-for-bit.
        let atk = AttackerStats {
            attack: 75,
            condition: 0,
            exalt_bonus: 1000,
        };
        let mut a = Rng::new(12345);
        let mut b = Rng::new(12345);
        for _ in 0..20 {
            let scaled = roll_base_damage(&mut a, 50, 150, 1.0, 1.0, 1, &atk);
            let plain = roll_base_damage(&mut b, 50, 150, 1.0, 1.0, 1, &atk);
            assert_eq!(scaled, plain);
        }
    }

    #[test]
    fn defense_reduces_damage_with_floor() {
        // 100 dmg vs 30 def => 70.
        assert_eq!(damage_with_defense(100, false, 30, 0, 0), 70);
        // Huge def => 10% floor of raw damage.
        assert_eq!(damage_with_defense(100, false, 9999, 0, 0), 10);
    }

    #[test]
    fn armor_piercing_ignores_defense() {
        assert_eq!(damage_with_defense(100, true, 9999, 0, 0), 100);
    }

    #[test]
    fn armorbroken_ignores_defense() {
        assert_eq!(
            damage_with_defense(100, false, 9999, COND_ARMORBROKEN, 0),
            100
        );
    }

    #[test]
    fn armored_increases_defense() {
        // def 40 * 1.5 = 60 => 100 - 60 = 40.
        assert_eq!(damage_with_defense(100, false, 40, COND_ARMORED, 0), 40);
    }

    #[test]
    fn exposed_reduces_defense() {
        // def 30 - 20 = 10 => 90.
        assert_eq!(damage_with_defense(100, false, 30, 0, NEWCON_EXPOSED), 90);
    }

    #[test]
    fn invulnerable_zeroes_damage() {
        assert_eq!(damage_with_defense(100, false, 0, COND_INVULNERABLE, 0), 0);
    }

    #[test]
    fn damage_resistance_enchant_maps_tiers() {
        assert_eq!(damage_resistance_pct(1556), 2); // I
        assert_eq!(damage_resistance_pct(1557), 3); // II
        assert_eq!(damage_resistance_pct(1558), 4); // III
        assert_eq!(damage_resistance_pct(1559), 5); // IV
        assert_eq!(damage_resistance_pct(9999), 0); // not a DR enchant
    }

    #[test]
    fn post_defense_reductions_stack_multiplicatively() {
        // No reductions: passthrough.
        assert_eq!(
            apply_post_defense_reductions(100, &DamageReductions::default()),
            100
        );
        // 5% enchant: 100 * 0.95 = 95.
        let ench = DamageReductions {
            enchant_pct: 5,
            ..Default::default()
        };
        assert_eq!(apply_post_defense_reductions(100, &ench), 95);
        // Armor of Nil 15%: 100 * 0.85 = 85.
        let nil = DamageReductions {
            armor_of_nil: true,
            ..Default::default()
        };
        assert_eq!(apply_post_defense_reductions(100, &nil), 85);
        // Knight shield 25%: 100 * 0.75 = 75.
        let shield = DamageReductions {
            shield_pct: 25,
            ..Default::default()
        };
        assert_eq!(apply_post_defense_reductions(100, &shield), 75);
        // All three compound: 100 * 0.95 * 0.85 * 0.75 = 60.56 -> 60.
        let all = DamageReductions {
            enchant_pct: 5,
            armor_of_nil: true,
            shield_pct: 25,
        };
        assert_eq!(apply_post_defense_reductions(100, &all), 60);
    }

    #[test]
    fn blocked_accounts_for_defense_and_reductions() {
        let none = DamageReductions::default();
        // 100 base vs 30 def => 70 taken, 30 blocked by defense alone.
        assert_eq!(
            damage_taken_and_blocked(100, false, 30, 0, 0, &none),
            (70, 30)
        );
        // Invulnerable: takes 0, blocks all 100.
        assert_eq!(
            damage_taken_and_blocked(100, false, 0, COND_INVULNERABLE, 0, &none),
            (0, 100)
        );
        // Armored doubles the effective block: def 40 * 1.5 = 60 => 40 taken.
        assert_eq!(
            damage_taken_and_blocked(100, false, 40, COND_ARMORED, 0, &none),
            (40, 60)
        );
        // Defense + shield: 100 - 30 = 70, then *0.75 = 52 taken, 48 blocked.
        let shield = DamageReductions {
            shield_pct: 25,
            ..Default::default()
        };
        assert_eq!(
            damage_taken_and_blocked(100, false, 30, 0, 0, &shield),
            (52, 48)
        );
        // Cursed amplifies damage above base: blocked clamps to 0, never negative.
        let (taken, blocked) = damage_taken_and_blocked(100, false, 0, 0, NEWCON_CURSED, &none);
        assert_eq!(taken, 125);
        assert_eq!(blocked, 0);
    }

    #[test]
    fn cursed_and_petrified_scale_result() {
        // 100 dmg, 0 def, cursed => *1.25 = 125.
        assert_eq!(damage_with_defense(100, false, 0, 0, NEWCON_CURSED), 125);
        // petrified => *0.9 = 90.
        assert_eq!(damage_with_defense(100, false, 0, 0, NEWCON_PETRIFIED), 90);
    }

    // T6 Cloak of Ghostly Concealment.
    const T6: LethalStrikeParams = LethalStrikeParams {
        ignore_flat: 70.0,
        ignore_perc: 0.50,
        stat_mod_flat: 3.0,
        stat_mod_perc: 0.005,
        stat_mod_scaling_min: 34.0,
        scaling_stat: ScalingStat::Wisdom,
        duration: 2.4,
        extra_shots: 2,
    };
    // T0 Cloak of Shadows.
    const T0: LethalStrikeParams = LethalStrikeParams {
        ignore_flat: 20.0,
        ignore_perc: 0.20,
        stat_mod_flat: 1.0,
        stat_mod_perc: 0.005,
        stat_mod_scaling_min: 34.0,
        scaling_stat: ScalingStat::Wisdom,
        duration: 2.4,
        extra_shots: 2,
    };

    #[test]
    fn lethal_strike_t6_wis50_vs_def50() {
        // w=16: flat=70+16*3=118, perc=0.50+16*0.005=0.58; 118 + 0.58*50 = 147.
        assert_eq!(lethal_strike_bonus(&T6, 50, 50), 147);
    }

    #[test]
    fn lethal_strike_t6_wis50_vs_def0() {
        assert_eq!(lethal_strike_bonus(&T6, 50, 0), 118);
    }

    #[test]
    fn lethal_strike_t0_wis50_vs_def50() {
        // w=16: flat=20+16=36, perc=0.20+0.08=0.28; 36 + 0.28*50 = 50.
        assert_eq!(lethal_strike_bonus(&T0, 50, 50), 50);
    }

    #[test]
    fn lethal_strike_wis_at_or_below_threshold_has_no_scaling() {
        // w=0: flat=70, perc=0.50; 70 + 0.50*40 = 90.
        assert_eq!(lethal_strike_bonus(&T6, 34, 40), 90);
        // WIS below threshold clamps to the same (no negative scaling).
        assert_eq!(lethal_strike_bonus(&T6, 20, 40), 90);
    }

    #[test]
    fn lethal_strike_floors_fractional_perc() {
        // w=16: perc=0.58; 118 + 0.58*7 = 118 + 4.06 = 122.06 -> floor 122.
        assert_eq!(lethal_strike_bonus(&T6, 50, 7), 122);
    }

    #[test]
    fn lethal_strike_negative_defense_treated_as_zero() {
        assert_eq!(lethal_strike_bonus(&T6, 50, -100), 118);
    }
}

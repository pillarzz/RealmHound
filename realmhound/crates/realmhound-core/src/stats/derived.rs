//! Theoretical / derived character stats: effective stats with gear and
//! enchants, weapon DPS, HP/MP regen, loot boost, and pet contribution.
//!
//! All magnitudes come from the parsed asset data (`equip.xml` /
//! `enchantments.xml`); RotMG gameplay formulas (attack multiplier, defense,
//! fire rate, regen, pet throughput) are documented inline. Results are
//! "theoretical" -- they assume out-of-combat regen and, unless a target
//! defense is supplied, a defenseless target with every projectile landing.

use crate::api::crucible::CrucibleDef;
use crate::assets::{
    AbilityEffect, AssetManager, EnchantEffectKind, ProcEffect, ProcTrigger, ScalingStat,
    StatBonuses, StatKind,
};
use crate::combat::damage::damage_with_defense;

/// The eight base character stats (leveled/pot stats, WITHOUT gear or enchants).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BaseStats {
    pub max_hp: i32,
    pub max_mp: i32,
    pub attack: i32,
    pub defense: i32,
    pub speed: i32,
    pub dexterity: i32,
    pub vitality: i32,
    pub wisdom: i32,
}

/// Effective stats after applying equipped gear and enchant bonuses (floored to
/// integers to match the server-side effective stats the game displays).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct EffectiveStats {
    pub max_hp: i32,
    pub max_mp: i32,
    pub attack: i32,
    pub defense: i32,
    pub speed: i32,
    pub dexterity: i32,
    pub vitality: i32,
    pub wisdom: i32,
}

/// A single attribute's value broken into base, gear+enchant bonus, and exalt
/// bonus, mirroring the game's Attributes tooltip (`total (+gear)(+exalt)`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Attribute {
    pub base: i32,
    /// Combined gear + enchant bonus (may be fractional from enchants).
    pub gear: f32,
    /// Exalt bonus (integer per-stat).
    pub exalt: i32,
    /// Crucible flat stat modifier (integer per-stat), if crucible-active.
    pub crucible: i32,
}

impl Attribute {
    /// Effective total. The relative-stat halving (e.g. Wretched Rags) can yield
    /// a `.5` fraction; the game rounds to nearest for display, so we do too.
    pub fn total(&self) -> i32 {
        (self.base as f32 + self.gear + self.exalt as f32 + self.crucible as f32).round() as i32
    }

    /// Gear bonus for display, derived so that `base + this + exalt + crucible
    /// == total()` (avoids a rounded gear bracket contradicting the total).
    pub fn gear_display(&self) -> i32 {
        self.total() - self.base - self.exalt - self.crucible
    }
}

/// Per-stat attribute breakdown (base / gear / exalt) for all eight stats.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Attributes {
    pub max_hp: Attribute,
    pub max_mp: Attribute,
    pub attack: Attribute,
    pub defense: Attribute,
    pub speed: Attribute,
    pub dexterity: Attribute,
    pub vitality: Attribute,
    pub wisdom: Attribute,
}

/// Out-of-combat regen split into the base (vit/wis) portion and the doubled
/// enchant (flat + percentage) portion, so the UI can show a breakdown.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RegenBreakdown {
    /// Base regen from Vitality (HP) / Wisdom (MP), per second.
    pub base: f32,
    /// Enchant regen bonus (flat + percentage-of-max), already doubled for the
    /// out-of-combat state, per second.
    pub enchant_bonus: f32,
    /// A representative regen-enchant id (for the breakdown icon), if any.
    pub enchant_id: Option<i32>,
}

impl RegenBreakdown {
    pub fn total(&self) -> f32 {
        self.base + self.enchant_bonus
    }
}
/// silent zero when asset data for an equipped item/enchant is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    /// Every equipped item and enchant resolved in the assets.
    Full,
    /// Some equipped item/enchant was unknown; bonuses may be understated.
    Partial,
    /// Assets aren't loaded; nothing could be computed.
    Unavailable,
}

/// A single pet ability's out-of-combat throughput.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PetAbilityContribution {
    pub ability_type: i32,
    pub level: i32,
    /// HP restored per second (Heal ability); 0 otherwise.
    pub hp_per_sec: f32,
    /// MP restored per second (Magic Heal ability); 0 otherwise.
    pub mp_per_sec: f32,
    /// Damage per second (attack abilities: Close/Mid/Far/Electric); 0 otherwise.
    pub dps: f32,
}

/// A resolved native weapon proc contribution to DPS, ready for display. The
/// numbers are already scaled by the current effective stats.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeaponProcReadout {
    /// Effect label (e.g. "Poison").
    pub label: &'static str,
    /// Damage delivered per activation (grand total for poison: impact + DoT).
    pub damage_per_proc: i32,
    /// Cooldown between activations in seconds (0 = every trigger).
    pub cooldown: f32,
    /// Activation chance per eligible trigger (0..1).
    pub proc_rate: f32,
    /// Expected activations per second at the current fire rate.
    pub activations_per_sec: f32,
    /// DPS contribution (`damage_per_proc * activations_per_sec`).
    pub dps: f32,
    /// Whether this proc ignores target defense (poison does).
    pub armor_piercing: bool,
    /// Whether this is a continuously-maintained DoT (e.g. Bleeding) rather than
    /// a discrete per-activation proc. When true, `damage_per_proc` is the
    /// per-second drain of a single stack and `activations_per_sec` is the
    /// steady-state number of concurrent stacks maintained under sustained fire.
    pub continuous: bool,
}

/// Weapon damage/DPS breakdown at a defenseless target (baseline). Apply a
/// target defense with [`WeaponDamage::dps_vs`].
#[derive(Debug, Clone, PartialEq)]
pub struct WeaponDamage {
    pub weapon_id: i32,
    pub min_damage: i32,
    pub max_damage: i32,
    pub armor_piercing: bool,
    /// Expected per-projectile damage at DEF 0 for the primary (index 0) type.
    pub avg_hit: f32,
    pub num_projectiles: i32,
    /// Total projectiles per weapon trigger, accounting for weapons with
    /// multiple distinct projectile types (e.g. Fractal Blades has 6 types).
    pub total_projectiles: i32,
    /// Extra projectile types beyond index 0: `(min, max, armor_piercing)`,
    /// already scaled by enchant/mastery multipliers.
    pub extra_types: Vec<(i32, i32, bool)>,
    pub shots_per_sec: f32,
    /// DPS at a defenseless target (all projectile types, all projectiles hit).
    pub dps: f32,
    /// Attack multiplier folded into `avg_hit`, retained for defense math.
    pub stored_mult: f32,
    /// Combined damage-bonus enchant multiplier applied to the weapon's rolled
    /// damage (e.g. 1.05 for Damage Bonus IV); 1.0 when none.
    pub damage_mult: f32,
    /// Exaltation Mastery weapon-damage bonus, in percent (0-10). Already folded
    /// into `min_damage`/`max_damage` and DPS; retained for display.
    pub mastery_pct: f32,
    /// Native on-shoot/on-hit procs contributing to DPS (e.g. poison bursts),
    /// resolved at the current effective stats. Empty when the weapon has none.
    pub procs: Vec<WeaponProcReadout>,
    /// Aggregate DPS from armor-piercing procs (currently all parsed procs are
    /// poison, which ignores defense). Folded into `dps` and `dps_vs`.
    pub proc_dps: f32,
}

impl WeaponDamage {
    /// Expected DPS versus a target with `target_def` defense, applying the
    /// game's per-shot defense formula to each roll across all projectile types.
    pub fn dps_vs(&self, target_def: i32) -> f32 {
        let primary = self.avg_hit_vs(
            self.min_damage,
            self.max_damage,
            self.armor_piercing,
            target_def,
        );
        let mut total = primary * self.num_projectiles as f32;
        for &(emin, emax, ap) in &self.extra_types {
            total += self.avg_hit_vs(emin, emax, ap, target_def) * self.num_projectiles as f32;
        }
        // Poison procs are armor-piercing, so their DPS is defense-independent.
        total * self.shots_per_sec + self.proc_dps
    }

    /// Expected per-projectile damage vs a given DEF for one projectile type.
    fn avg_hit_vs(&self, min: i32, max: i32, ap: bool, target_def: i32) -> f32 {
        let (lo, hi) = if max > min {
            (min, max - 1)
        } else {
            (min, min)
        };
        let n = (hi - lo + 1) as f64;
        let mut sum = 0.0f64;
        for d in lo..=hi {
            let shot = (d as f32 * self.stored_mult) as i32;
            let after = damage_with_defense(shot, ap, target_def, 0, 0);
            sum += after as f64;
        }
        (sum / n) as f32
    }
}

/// Theoretical self-computed damage from the equipped ability item (orb, etc.)
/// for ONE ability use, scaled by the current effective set. Guaranteed poison
/// lands on every use; the hex-detonation blast only fires against enemies
/// carrying hex stacks (built up by weapon hits), so it is reported separately.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AbilityDamage {
    pub item_id: i32,
    /// The stat that scales this ability (e.g. MAXMP for Command Cornea).
    pub scaling_label: &'static str,
    /// Effective value of the scaling stat (post-gear/exalt).
    pub scaling_value: i32,
    /// Immediate armor-piercing poison impact on every use (0 if none).
    pub poison_impact: i32,
    /// Poison damage-over-time on every use (0 if none), over `poison_duration`.
    pub poison_dot: i32,
    /// Poison DoT window in seconds.
    pub poison_duration: f32,
    /// Hex-detonation base blast (0 stacks), if the ability detonates hex.
    pub detonate_base: Option<i32>,
    /// Hex-detonation extra damage per consumed hex stack, if applicable.
    pub detonate_per_stack: Option<i32>,
}

impl AbilityDamage {
    /// Total guaranteed poison per ability use (impact + full DoT).
    pub fn poison_total(&self) -> i32 {
        self.poison_impact + self.poison_dot
    }
    /// Whether any guaranteed poison lands on use.
    pub fn has_poison(&self) -> bool {
        self.poison_impact > 0 || self.poison_dot > 0
    }
}

/// One styled portion of an ability line's value. When `stat` is set, the text
/// is a stat-scaling bonus and the UI colors it with that stat's potion color;
/// when `None` it is plain/base text rendered in the default color.
#[derive(Debug, Clone, PartialEq)]
pub struct AbilitySegment {
    pub text: String,
    pub stat: Option<ScalingStat>,
}

impl AbilitySegment {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            stat: None,
        }
    }
    pub fn scaled(text: impl Into<String>, stat: ScalingStat) -> Self {
        Self {
            text: text.into(),
            stat: Some(stat),
        }
    }
}

/// One human-readable line describing an ability effect for the stats page,
/// e.g. label "Chain dmg" value "324 (+174)  -30/extra target" where "(+174)"
/// is colored by the scaling stat. `segments` render left-to-right.
#[derive(Debug, Clone, PartialEq)]
pub struct AbilityLine {
    pub label: String,
    pub segments: Vec<AbilitySegment>,
    /// Whether this line is part of the damage breakdown (folded into the
    /// "Potential ability damage per use" summary and shown on hover) versus a
    /// non-damage effect (heal/buff/decoy/teleport) shown inline.
    pub is_damage: bool,
}

impl AbilityLine {
    /// Convenience for a single plain-text, non-damage value (no stat scaling).
    fn plain(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            segments: vec![AbilitySegment::plain(value)],
            is_damage: false,
        }
    }

    /// A damage-breakdown line (contributes to the potential-damage summary).
    fn dmg(label: impl Into<String>, segments: Vec<AbilitySegment>) -> Self {
        Self {
            label: label.into(),
            segments,
            is_damage: true,
        }
    }

    /// A non-damage effect line (heal/MP/buff/etc.), shown inline.
    fn info(label: impl Into<String>, segments: Vec<AbilitySegment>) -> Self {
        Self {
            label: label.into(),
            segments,
            is_damage: false,
        }
    }
}

/// Summary of an ability's total potential damage for one use. Two figures are
/// provided: `total`/`scaled` assume every component lands on its maximum number
/// of targets (best case AoE); `single_target`/`single_scaled` assume everything
/// lands on a single enemy (boss case). Each `scaled` list attributes how much
/// of its total each stat contributes so the UI can color those portions.
#[derive(Debug, Clone, PartialEq)]
pub struct AbilityDamagePerUse {
    /// Total potential damage across the maximum number of targets.
    pub total: i32,
    /// Per-stat scaled contribution to `total`, in scales-off order.
    pub scaled: Vec<(ScalingStat, i32)>,
    /// Total potential damage if every component lands on one target.
    pub single_target: i32,
    /// Per-stat scaled contribution to `single_target`, in scales-off order.
    pub single_scaled: Vec<(ScalingStat, i32)>,
}

/// Theoretical hex-detonation blast at the maximum hex-stack cap. Unlike
/// guaranteed on-use damage, this is the best case against a fully hexed enemy
/// (stacks are built by weapon hits), so it is reported on its own.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HexBlastReadout {
    /// Stat the blast scales off (e.g. WIS), if any.
    pub scaling_stat: Option<ScalingStat>,
    /// Effective value of the scaling stat.
    pub scaling_value: i32,
    /// Scaling floor below which the stat mods contribute nothing.
    pub scaling_min: i32,
    /// Base blast damage at 0 stacks (current stat).
    pub base: i32,
    /// Added damage per consumed hex stack (current stat).
    pub per_stack: i32,
    /// Multiplicative per-stack bonus (`stackMultiplier`).
    pub stack_multiplier: f32,
    /// Hex stacks assumed for the maximum (the per-enemy cap).
    pub max_stacks: u32,
    /// Blast damage at `max_stacks` (current stat).
    pub max_damage: i32,
    /// Portion of `max_damage` attributable to the scaling stat above its floor.
    pub max_damage_bonus: i32,
}

/// Lethal Strike readout: the pre-computed flat and percentage components at the
/// current effective stats. The UI combines these with the target DEF input to
/// produce a final per-shot bonus and total DPS contribution.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LethalStrikeReadout {
    /// Flat bonus damage per weapon shot (after stat scaling).
    pub flat: i32,
    /// Portion of `flat` attributable to the scaling stat above its floor.
    pub flat_bonus: i32,
    /// Percentage of target DEF added as bonus damage (0-100+ scale).
    pub perc: f32,
    /// Portion of `perc` attributable to the scaling stat.
    pub perc_bonus: f32,
    /// The stat this scales off (WIS for most, DEF for Cubic Enigma, etc.).
    pub scaling_stat: ScalingStat,
    /// Number of extra projectiles fired per weapon shot during the LS window
    /// (from `OnPlayerShootActivate BulletCreate` count, typically 2).
    pub extra_shots: i32,
    /// Duration of the Lethal Strike window in seconds.
    pub duration: f32,
}

impl LethalStrikeReadout {
    /// Per-shot bonus damage against a target with `target_def` defense.
    pub fn bonus_vs(&self, target_def: i32) -> i32 {
        (self.flat as f32 + self.perc * target_def.max(0) as f32)
            .floor()
            .max(0.0) as i32
    }
}

/// Computed, display-ready readout for the equipped ability item: the stat(s)
/// it scales off plus one line per effect (damage/heal/buff/mobility), each
/// already scaled by the current effective set.
#[derive(Debug, Clone, PartialEq)]
pub struct AbilityReadout {
    pub item_id: i32,
    /// Stats this ability scales off (e.g. ["WIS"], ["DEF"]), de-duplicated in
    /// first-seen order. Empty when the ability has no stat scaling.
    pub scales_off: Vec<&'static str>,
    /// One display line per computed effect.
    pub lines: Vec<AbilityLine>,
    /// Total potential damage for one use, if the ability deals self-computable
    /// damage. When set, the damage lines (`is_damage`) fold into this summary.
    pub damage_per_use: Option<AbilityDamagePerUse>,
    /// Theoretical hex-blast max damage per use, if the ability detonates hex.
    /// When set, the "Hex blast" line folds into this summary + its hover.
    pub hex_blast: Option<HexBlastReadout>,
    /// Lethal Strike per-shot bonus (rogue cloaks). Rendered separately in the
    /// UI because the final number depends on the user's target DEF input.
    pub lethal_strike: Option<LethalStrikeReadout>,
}

impl AbilityReadout {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

/// The full derived-stats bundle for one character loadout.
#[derive(Debug, Clone)]
pub struct DerivedStats {
    pub effective: EffectiveStats,
    /// Per-stat base / gear / exalt breakdown for the Attributes display.
    pub attributes: Attributes,
    pub provenance: Provenance,
    /// Combined loot-drop bonus from enchants, in percent.
    pub loot_boost_pct: f32,
    /// Combined XP bonus from enchants, in percent.
    pub xp_boost_pct: f32,
    /// Combined base XP-gain bonus from equipped gear (`<XPBonus>` tooltips), in
    /// percent. Only the equipped slots contribute.
    pub xp_base_gear_pct: f32,
    /// Combined dust bonus from enchants, in percent.
    pub dust_boost_pct: f32,
    /// Out-of-combat HP regeneration, HP/sec.
    pub hp_regen: f32,
    /// Out-of-combat MP regeneration, MP/sec.
    pub mp_regen: f32,
    /// HP regen breakdown (base vs enchant bonus).
    pub hp_regen_breakdown: RegenBreakdown,
    /// MP regen breakdown (base vs enchant bonus).
    pub mp_regen_breakdown: RegenBreakdown,
    pub weapon: Option<WeaponDamage>,
    pub pet: Vec<PetAbilityContribution>,
    /// Equipped pet's skin/type id for the regen breakdown icon, if any.
    pub pet_skin: Option<i32>,
    /// Equipped pet's type id (for name resolution), if any.
    pub pet_type: Option<i32>,
    /// Equipped pet's display name, if resolved.
    pub pet_name: Option<String>,
    /// Theoretical ability-item damage for one use, scaled by the set.
    pub ability: Option<AbilityDamage>,
    /// Display-ready ability readout (all effect types) for the stats page.
    pub ability_readout: Option<AbilityReadout>,
    /// Active Crucible definition applied to this loadout, if the character
    /// carries the crucible marker and a definition is known.
    pub crucible: Option<CrucibleDef>,
}

impl DerivedStats {
    /// Total pet HP/sec across all healing abilities.
    pub fn pet_hp_per_sec(&self) -> f32 {
        self.pet.iter().map(|p| p.hp_per_sec).sum()
    }
    /// Total pet MP/sec across all healing abilities.
    pub fn pet_mp_per_sec(&self) -> f32 {
        self.pet.iter().map(|p| p.mp_per_sec).sum()
    }
    /// Total pet DPS across all attack abilities.
    pub fn pet_dps(&self) -> f32 {
        self.pet.iter().map(|p| p.dps).sum()
    }
}

/// A character loadout: base stats plus equipped items, their enchants, pet
/// abilities, and (optionally) the exalt bonus-damage points.
#[derive(Debug, Clone, Default)]
pub struct Loadout {
    pub base: BaseStats,
    /// Equipped item IDs; index 0 is the weapon slot.
    pub equipment: Vec<i32>,
    /// Enchant type IDs per equipment slot (parallel to `equipment`).
    pub enchants: Vec<Vec<u16>>,
    /// Pet abilities as `(ability_type, level)`.
    pub pet: Vec<(i32, i32)>,
    /// Exalt bonus-damage points (server stat 113). `None` when offline; the
    /// attack multiplier then treats exalt as neutral (1000/1000).
    pub exalt_damage_bonus: Option<i32>,
    /// Per-stat exalt bonuses to add to effective stats (caller computes these
    /// from exalt levels: +1/tier for the six primary stats, +5/tier HP/MP).
    pub exalt_bonus: BaseStats,
    /// Exaltation Mastery weapon-damage bonus, in percent (0-10), derived from
    /// the class's minimum stat exalt level. Multiplies the weapon's raw damage.
    pub mastery_dmg_pct: f32,
    /// Active Crucible definition to apply, populated only when the character
    /// carries the crucible marker. Flat stat mods fold into effective stats.
    pub crucible: Option<CrucibleDef>,
}

// --- RotMG formula helpers -------------------------------------------------

/// Out-of-combat HP regen (HP/sec): `2 * (1 + 0.12 * VIT)`.
pub fn hp_regen_per_sec(vitality: i32) -> f32 {
    2.0 * (1.0 + 0.12 * vitality as f32)
}

/// Out-of-combat MP regen (MP/sec): `2 * (0.5 + 0.06 * WIS)`.
pub fn mp_regen_per_sec(wisdom: i32) -> f32 {
    2.0 * (0.5 + 0.06 * wisdom as f32)
}

/// Combat Trigger: minimum post-defense damage from one hit needed to enter the
/// in-combat state (which halves VIT/WIS and pet regen). Scales with total DEF
/// in diminishing brackets, capped at 60 (DEF >= 125). Matches the game's
/// sword-icon tooltip; the result is floored to a whole number.
///
/// Brackets (per RealmEye's Vital Combat page):
/// - 0-15 DEF: +1.0 per DEF
/// - 16-35 DEF: +0.75 per DEF
/// - 36-65 DEF: +0.5 per DEF
/// - 66-125 DEF: +0.25 per DEF
/// - >125 DEF: no further increase
pub fn combat_trigger(defense: i32) -> i32 {
    let def = defense.max(0) as f32;
    let mut t = def.min(15.0);
    if def > 15.0 {
        t += (def.min(35.0) - 15.0) * 0.75;
    }
    if def > 35.0 {
        t += (def.min(65.0) - 35.0) * 0.5;
    }
    if def > 65.0 {
        t += (def.min(125.0) - 65.0) * 0.25;
    }
    t.floor() as i32
}

/// Shots per second: `(1.5 + 6.5 * DEX/75) * weapon_rate_of_fire`.
pub fn shots_per_sec(dexterity: i32, rate_of_fire: f32) -> f32 {
    (1.5 + 6.5 * (dexterity as f32 / 75.0)) * rate_of_fire
}

/// Attack multiplier for a main-weapon shot (no Weak/Damaging conditions):
/// `(ATT + 25) * 0.02 * (1000 + exalt_bonus) / 1000`.
fn attack_multiplier(attack: i32, exalt_bonus: Option<i32>) -> f32 {
    let base = (attack + 25) as f32 * 0.02;
    let exalt = 1000.0 + exalt_bonus.unwrap_or(0) as f32;
    base * (exalt / 1000.0)
}

// --- Effective stats -------------------------------------------------------

#[cfg(test)]
fn add_stat(e: &mut [f32; 8], stat: StatKind, amount: f32) {
    e[stat_index(stat)] += amount;
}

fn stat_index(stat: StatKind) -> usize {
    match stat {
        StatKind::MaxHp => 0,
        StatKind::MaxMp => 1,
        StatKind::Attack => 2,
        StatKind::Defense => 3,
        StatKind::Speed => 4,
        StatKind::Dexterity => 5,
        StatKind::Vitality => 6,
        StatKind::Wisdom => 7,
    }
}

fn base_array(b: &BaseStats) -> [f32; 8] {
    [
        b.max_hp as f32,
        b.max_mp as f32,
        b.attack as f32,
        b.defense as f32,
        b.speed as f32,
        b.dexterity as f32,
        b.vitality as f32,
        b.wisdom as f32,
    ]
}

fn add_bonuses(e: &mut [f32; 8], b: &StatBonuses) {
    e[0] += b.max_hp;
    e[1] += b.max_mp;
    e[2] += b.attack;
    e[3] += b.defense;
    e[4] += b.speed;
    e[5] += b.dexterity;
    e[6] += b.vitality;
    e[7] += b.wisdom;
}

// --- Pet ability throughput tables -----------------------------------------

/// Piecewise-linear interpolation of `(level, value)` anchors at `level`.
fn interp(anchors: &[(i32, f32)], level: i32) -> f32 {
    if level <= anchors[0].0 {
        return anchors[0].1;
    }
    if let Some(last) = anchors.last() {
        if level >= last.0 {
            return last.1;
        }
    }
    for w in anchors.windows(2) {
        let (l0, v0) = w[0];
        let (l1, v1) = w[1];
        if level >= l0 && level <= l1 {
            let t = (level - l0) as f32 / (l1 - l0) as f32;
            return v0 + (v1 - v0) * t;
        }
    }
    anchors.last().map(|a| a.1).unwrap_or(0.0)
}

// Delay-between-uses anchors shared by Heal and Magic Heal (seconds).
const HEAL_DELAY: &[(i32, f32)] = &[
    (1, 10.0),
    (30, 3.82),
    (50, 2.66),
    (70, 1.96),
    (90, 1.42),
    (100, 1.0),
];
const HEAL_AMOUNT: &[(i32, f32)] = &[
    (1, 10.0),
    (30, 14.0),
    (50, 23.0),
    (70, 40.0),
    (90, 69.0),
    (100, 90.0),
];
const MAGIC_HEAL_AMOUNT: &[(i32, f32)] = &[
    (1, 1.0),
    (30, 3.0),
    (50, 8.0),
    (70, 17.0),
    (90, 33.0),
    (100, 45.0),
];
// Attack-ability firing frequency anchors (seconds between shots), shared by
// Attack Close/Mid/Far and Electric.
const ATTACK_FREQ: &[(i32, f32)] = &[
    (1, 5.0),
    (30, 2.0),
    (50, 1.0),
    (70, 0.6),
    (90, 0.4),
    (100, 0.2),
];
const ATTACK_CLOSE_POWER: &[(i32, f32)] = &[
    (1, 7.0),
    (30, 17.0),
    (50, 39.0),
    (70, 80.0),
    (90, 130.0),
    (100, 200.0),
];
const ATTACK_MID_POWER: &[(i32, f32)] = &[
    (1, 5.0),
    (30, 12.0),
    (50, 25.0),
    (70, 60.0),
    (90, 121.0),
    (100, 150.0),
];
const ATTACK_FAR_POWER: &[(i32, f32)] = &[
    (1, 3.0),
    (30, 8.0),
    (50, 19.0),
    (70, 40.0),
    (90, 76.0),
    (100, 100.0),
];
// Electric high-level power is approximate (wiki table truncated above 70).
const ELECTRIC_POWER: &[(i32, f32)] =
    &[(1, 5.0), (30, 20.0), (50, 54.0), (70, 117.0), (100, 200.0)];

fn pet_contribution(ability_type: i32, level: i32) -> PetAbilityContribution {
    let mut c = PetAbilityContribution {
        ability_type,
        level,
        hp_per_sec: 0.0,
        mp_per_sec: 0.0,
        dps: 0.0,
    };
    if level <= 0 {
        return c;
    }
    match ability_type {
        407 => {
            // Heal
            c.hp_per_sec = interp(HEAL_AMOUNT, level) / interp(HEAL_DELAY, level);
        }
        408 => {
            // Magic Heal
            c.mp_per_sec = interp(MAGIC_HEAL_AMOUNT, level) / interp(HEAL_DELAY, level);
        }
        404 => c.dps = interp(ATTACK_CLOSE_POWER, level) / interp(ATTACK_FREQ, level),
        405 => c.dps = interp(ATTACK_MID_POWER, level) / interp(ATTACK_FREQ, level),
        409 => c.dps = interp(ATTACK_FAR_POWER, level) / interp(ATTACK_FREQ, level),
        406 => c.dps = interp(ELECTRIC_POWER, level) / interp(ATTACK_FREQ, level),
        _ => {}
    }
    c
}

// --- Public entry point ----------------------------------------------------

/// Derive theoretical stats/DPS for a loadout, reading magnitudes from `assets`.
pub fn derive(assets: &AssetManager, loadout: &Loadout) -> DerivedStats {
    let base = base_array(&loadout.base);
    // Gear + enchant bonus accumulator (bonus portion only, base excluded).
    let mut gear = [0.0f32; 8];
    let mut provenance = if assets.is_loaded() {
        Provenance::Full
    } else {
        Provenance::Unavailable
    };
    let mut loot = 0.0f32;
    let mut xp = 0.0f32;
    let mut dust = 0.0f32;
    // Base per-item XP-gain bonus (equip.xml `<XPBonus>`), summed across the
    // equipped slots. Separate from `xp` (enchant XP bonuses) so the UI can
    // break the two sources out.
    let mut base_gear_xp = 0.0f32;

    // Regen enchant accumulators (out-of-combat doubling applied later).
    let mut flat_hp_regen = 0.0f32;
    let mut flat_mp_regen = 0.0f32;
    let mut pct_hp_regen = 0.0f32; // fraction of effective max HP per second
    let mut pct_mp_regen = 0.0f32;
    // Lowest-tier regen enchant id per type (for the breakdown icon): the
    // weakest tier worn is shown so the user sees room to improve. Flat and
    // Percentage enchants of the same type (HP/MP) compete on tier; HP and MP
    // never compare against each other. Untiered enchants sort last (u8::MAX)
    // but are still shown when they are the only one of their type.
    let mut hp_regen_ench: Option<i32> = None;
    let mut mp_regen_ench: Option<i32> = None;
    let mut hp_regen_tier = u8::MAX;
    let mut mp_regen_tier = u8::MAX;

    // Weapon damage-bonus enchant multipliers (min/max); 1.0 = none.
    let mut dmg_min_mult = 1.0f32;
    let mut dmg_max_mult = 1.0f32;

    // Enchant bonus-relative effects (relative to the BONUS portion), deferred.
    let mut enchant_relatives: Vec<(StatKind, f32, StatKind)> = Vec::new();
    // Item IncrementStatRelative effects (relative to the TOTAL stat), deferred.
    let mut item_relatives: Vec<(StatKind, f32, StatKind)> = Vec::new();

    for (slot, &item_id) in loadout.equipment.iter().enumerate() {
        if item_id <= 0 {
            continue;
        }
        match assets.item_stat_bonuses(item_id) {
            Some(b) => add_bonuses(&mut gear, &b),
            None => provenance = downgrade(provenance),
        }
        base_gear_xp += assets.item_fame_bonus(item_id) as f32;
        for (stat, pct, rel) in assets.item_stat_relatives(item_id) {
            item_relatives.push((stat, pct, rel));
        }
        if let Some(enchants) = loadout.enchants.get(slot) {
            for &ench_id in enchants {
                if ench_id == 0 {
                    continue;
                }
                let effects = assets.enchant_effects(ench_id);
                if effects.is_empty() {
                    // Only a genuinely unknown enchant id (absent from the asset
                    // DB) may hide a stat bonus, so flag the totals as possibly
                    // understated. A *known* enchant with no attribute-affecting
                    // mutator (FireRate, MP-cost reduction, OnAbility procs,
                    // projectile speed, etc.) contributes nothing to the 8
                    // attributes and must not trigger the warning.
                    if assets.enchant_name(ench_id).is_none() {
                        provenance = downgrade(provenance);
                    }
                    continue;
                }
                for eff in effects {
                    match eff.kind {
                        EnchantEffectKind::IncrementStat => {
                            if let Some(s) = eff.stat {
                                gear[stat_index(s)] += eff.amount;
                            }
                        }
                        EnchantEffectKind::BonusStatRelative => {
                            if let (Some(s), Some(rel)) = (eff.stat, eff.stat_relative_to) {
                                enchant_relatives.push((s, eff.amount, rel));
                            }
                        }
                        EnchantEffectKind::FlatRegen => match eff.stat {
                            Some(StatKind::MaxHp) => {
                                flat_hp_regen += eff.amount;
                                let tier = assets.enchant_tier_num(ench_id).unwrap_or(u8::MAX);
                                if hp_regen_ench.is_none() || tier < hp_regen_tier {
                                    hp_regen_tier = tier;
                                    hp_regen_ench = Some(ench_id as i32);
                                }
                            }
                            Some(StatKind::MaxMp) => {
                                flat_mp_regen += eff.amount;
                                let tier = assets.enchant_tier_num(ench_id).unwrap_or(u8::MAX);
                                if mp_regen_ench.is_none() || tier < mp_regen_tier {
                                    mp_regen_tier = tier;
                                    mp_regen_ench = Some(ench_id as i32);
                                }
                            }
                            _ => {}
                        },
                        EnchantEffectKind::PercentageRegen => match eff.stat {
                            Some(StatKind::MaxHp) => {
                                pct_hp_regen += eff.amount;
                                let tier = assets.enchant_tier_num(ench_id).unwrap_or(u8::MAX);
                                if hp_regen_ench.is_none() || tier < hp_regen_tier {
                                    hp_regen_tier = tier;
                                    hp_regen_ench = Some(ench_id as i32);
                                }
                            }
                            Some(StatKind::MaxMp) => {
                                pct_mp_regen += eff.amount;
                                let tier = assets.enchant_tier_num(ench_id).unwrap_or(u8::MAX);
                                if mp_regen_ench.is_none() || tier < mp_regen_tier {
                                    mp_regen_tier = tier;
                                    mp_regen_ench = Some(ench_id as i32);
                                }
                            }
                            _ => {}
                        },
                        EnchantEffectKind::MultiplyMinDamage => dmg_min_mult *= eff.amount,
                        EnchantEffectKind::MultiplyMaxDamage => dmg_max_mult *= eff.amount,
                        EnchantEffectKind::LootBonus => loot += eff.amount,
                        EnchantEffectKind::XpBonus => xp += eff.amount,
                        EnchantEffectKind::DustBonus => dust += eff.amount,
                        EnchantEffectKind::Cosmetic | EnchantEffectKind::Other => {}
                    }
                }
            }
        }
    }

    // Exalt bonuses (integer per-stat) supplied by the caller. Computed before
    // relatives because in-game relative references use the effective stat
    // (base + flat gear + exalt), evaluated prior to any relative effects.
    let exalt = base_array(&loadout.exalt_bonus);

    // Freeze the flat gear bonus, then apply relative bonuses off it.
    let flat_gear = gear;
    // Enchant relatives: percentage of the BONUS portion of the reference stat
    // (flat gear + exalt, matching in-game "Bonus <Stat>" semantics).
    for (stat, pct, rel) in &enchant_relatives {
        let reference = flat_gear[stat_index(*rel)] + exalt[stat_index(*rel)];
        gear[stat_index(*stat)] += reference * *pct / 100.0;
    }
    // Item relatives: percentage of the effective reference stat
    // (base + flat gear + exalt), matching in-game behaviour.
    for (stat, pct, rel) in &item_relatives {
        let reference =
            base[stat_index(*rel)] + flat_gear[stat_index(*rel)] + exalt[stat_index(*rel)];
        gear[stat_index(*stat)] += reference * *pct / 100.0;
    }

    // Crucible flat stat modifiers (applied only when the character carries the
    // crucible marker; the caller sets `loadout.crucible` accordingly).
    let mut crucible_mods = [0.0f32; 8];
    if let Some(def) = &loadout.crucible {
        for i in 0..8 {
            crucible_mods[i] = def.flat_stat_mod(i);
        }
    }

    let attributes = Attributes {
        max_hp: attr(&base, &gear, &exalt, &crucible_mods, 0),
        max_mp: attr(&base, &gear, &exalt, &crucible_mods, 1),
        attack: attr(&base, &gear, &exalt, &crucible_mods, 2),
        defense: attr(&base, &gear, &exalt, &crucible_mods, 3),
        speed: attr(&base, &gear, &exalt, &crucible_mods, 4),
        dexterity: attr(&base, &gear, &exalt, &crucible_mods, 5),
        vitality: attr(&base, &gear, &exalt, &crucible_mods, 6),
        wisdom: attr(&base, &gear, &exalt, &crucible_mods, 7),
    };

    let effective = EffectiveStats {
        max_hp: attributes.max_hp.total(),
        max_mp: attributes.max_mp.total(),
        attack: attributes.attack.total(),
        defense: attributes.defense.total(),
        speed: attributes.speed.total(),
        dexterity: attributes.dexterity.total(),
        vitality: attributes.vitality.total(),
        wisdom: attributes.wisdom.total(),
    };

    let weapon = derive_weapon(assets, loadout, &effective, dmg_min_mult, dmg_max_mult);
    let ability = derive_ability(assets, loadout, &effective);
    let ability_readout = derive_ability_readout(assets, loadout, &effective);

    let pet = loadout
        .pet
        .iter()
        .map(|&(t, lvl)| pet_contribution(t, lvl))
        .collect();

    // Out-of-combat regen: base (vit/wis) plus doubled enchant bonus.
    let hp_regen_breakdown = RegenBreakdown {
        base: hp_regen_per_sec(effective.vitality),
        enchant_bonus: 2.0 * (flat_hp_regen + pct_hp_regen * effective.max_hp as f32),
        enchant_id: hp_regen_ench,
    };
    let mp_regen_breakdown = RegenBreakdown {
        base: mp_regen_per_sec(effective.wisdom),
        enchant_bonus: 2.0 * (flat_mp_regen + pct_mp_regen * effective.max_mp as f32),
        enchant_id: mp_regen_ench,
    };

    DerivedStats {
        effective,
        attributes,
        provenance,
        loot_boost_pct: loot,
        xp_boost_pct: xp,
        xp_base_gear_pct: base_gear_xp,
        dust_boost_pct: dust,
        hp_regen: hp_regen_breakdown.total(),
        mp_regen: mp_regen_breakdown.total(),
        hp_regen_breakdown,
        mp_regen_breakdown,
        weapon,
        pet,
        pet_skin: None,
        pet_type: None,
        pet_name: None,
        ability,
        ability_readout,
        crucible: loadout.crucible.clone(),
    }
}

fn attr(
    base: &[f32; 8],
    gear: &[f32; 8],
    exalt: &[f32; 8],
    crucible: &[f32; 8],
    i: usize,
) -> Attribute {
    Attribute {
        base: base[i] as i32,
        gear: gear[i],
        exalt: exalt[i] as i32,
        crucible: crucible[i] as i32,
    }
}

fn derive_weapon(
    assets: &AssetManager,
    loadout: &Loadout,
    eff: &EffectiveStats,
    dmg_min_mult: f32,
    dmg_max_mult: f32,
) -> Option<WeaponDamage> {
    let weapon_id = *loadout.equipment.first()?;
    if weapon_id <= 0 {
        return None;
    }
    let (min_damage, max_damage, armor_piercing, _slot) = assets.weapon_projectile(weapon_id, 0)?;
    let (rof, num) = assets.weapon_fire(weapon_id).unwrap_or((1.0, 1));

    let mastery = 1.0 + loadout.mastery_dmg_pct / 100.0;
    let bmin = ((min_damage as f32 * dmg_min_mult).round() * mastery).floor() as i32;
    let bmax = ((max_damage as f32 * dmg_max_mult).round() * mastery).floor() as i32;

    let mult = attack_multiplier(eff.attack, loadout.exalt_damage_bonus);
    let sps = shots_per_sec(eff.dexterity, rof);

    // Compute avg_hit for the primary (index 0) projectile type.
    let avg_hit = avg_hit_for_range(bmin, bmax, mult);

    // Gather all projectile types for accurate multi-type DPS.
    let proj_types = assets.projectile_type_count(weapon_id);
    let total_proj = num * proj_types;
    let mut extra_types: Vec<(i32, i32, bool)> = Vec::new();
    let mut total_avg_hit = avg_hit * num as f32;
    for i in 1..proj_types as usize {
        if let Some((pmin, pmax, ap, _)) = assets.weapon_projectile(weapon_id, i) {
            let emin = ((pmin as f32 * dmg_min_mult).round() * mastery).floor() as i32;
            let emax = ((pmax as f32 * dmg_max_mult).round() * mastery).floor() as i32;
            total_avg_hit += avg_hit_for_range(emin, emax, mult) * num as f32;
            extra_types.push((emin, emax, ap));
        }
    }

    let (procs, proc_dps) = derive_weapon_procs(assets, weapon_id, eff, sps);

    Some(WeaponDamage {
        weapon_id,
        min_damage: bmin,
        max_damage: bmax,
        armor_piercing,
        avg_hit,
        num_projectiles: num,
        total_projectiles: total_proj,
        extra_types,
        shots_per_sec: sps,
        dps: total_avg_hit * sps + proc_dps,
        stored_mult: mult,
        damage_mult: dmg_max_mult,
        mastery_pct: loadout.mastery_dmg_pct,
        procs,
        proc_dps,
    })
}

/// Resolve an item's native procs into DPS contributions at the current stats.
/// Only unconditional, damage-producing procs are counted; conditional procs
/// (Lethal Strike, Berserk, `mustWear`, etc.) are excluded from steady-state
/// DPS. `shots_per_sec` is the weapon's current fire rate, used as the proc
/// trigger rate.
fn derive_weapon_procs(
    assets: &AssetManager,
    weapon_id: i32,
    eff: &EffectiveStats,
    shots_per_sec: f32,
) -> (Vec<WeaponProcReadout>, f32) {
    let mut readouts = Vec::new();
    let mut total = 0.0f32;
    for proc in assets.weapon_procs(weapon_id) {
        if proc.required_conditions.is_some() {
            continue;
        }
        // On-shoot procs trigger on the weapon's fire rate; on-hit procs also
        // trigger at most once per shot (each shot can hit), so both use the
        // weapon fire rate as the trigger rate.
        let trigger_rate = match proc.trigger {
            ProcTrigger::OnShoot | ProcTrigger::OnHit => shots_per_sec,
        };
        match &proc.effect {
            ProcEffect::Poison(g) => {
                let stat = g.scaling_stat.map_or(0, |s| scaling_value(s, eff));
                let damage = g.delivered_total_for(stat);
                if damage <= 0 {
                    continue;
                }
                let rate = proc_activation_rate(proc.proc_rate, proc.cooldown, trigger_rate);
                let dps = damage as f32 * rate;
                total += dps;
                readouts.push(WeaponProcReadout {
                    label: "Poison",
                    damage_per_proc: damage,
                    cooldown: proc.cooldown,
                    proc_rate: proc.proc_rate,
                    activations_per_sec: rate,
                    dps,
                    armor_piercing: true,
                    continuous: false,
                });
            }
            ProcEffect::Bleeding(b) => {
                if b.dmg_per_sec <= 0.0 || b.duration_s <= 0.0 {
                    continue;
                }
                // Bleeding stacks: every landing shot adds a fresh stack that
                // drains `dmg_per_sec` HP/s for `duration_s`, and stacks add
                // together. Under sustained fire the steady-state number of live
                // stacks equals the shots landed within one duration window, so
                // the maintained drain is `dmg_per_sec * shots_per_sec *
                // duration_s`. Bleeding is armor-piercing and does not scale
                // with stats. (The game's ~50%-max-HP bleed cap is target-
                // specific, so it does not bind on this generic weapon readout.)
                let stacks = shots_per_sec * b.duration_s;
                let dps = b.dmg_per_sec * stacks;
                if dps <= 0.0 {
                    continue;
                }
                total += dps;
                readouts.push(WeaponProcReadout {
                    label: "Bleeding",
                    damage_per_proc: b.dmg_per_sec.round() as i32,
                    cooldown: 0.0,
                    proc_rate: 1.0,
                    activations_per_sec: stacks,
                    dps,
                    armor_piercing: true,
                    continuous: true,
                });
            }
        }
    }
    (readouts, total)
}

/// Expected activations per second for a cooldown-gated probabilistic proc that
/// rolls on every eligible trigger. Uses a renewal model: after a successful
/// activation the effect is locked out for `cooldown` seconds (at least one
/// trigger interval); once eligible again, a geometric number of failed rolls
/// (`(1 - p) / p` extra intervals) precede the next success.
fn proc_activation_rate(proc_rate: f32, cooldown: f32, shots_per_sec: f32) -> f32 {
    let p = proc_rate.clamp(0.0, 1.0);
    if p <= 0.0 || shots_per_sec <= 0.0 {
        return 0.0;
    }
    if cooldown <= 0.0 {
        return shots_per_sec * p;
    }
    // Trigger intervals blocked by the cooldown after a success (>= 1).
    let locked = (cooldown * shots_per_sec).ceil().max(1.0);
    let waiting = (1.0 - p) / p; // expected extra intervals until a success
    shots_per_sec / (locked + waiting)
}

/// Expected per-projectile damage at DEF 0 for a given damage range, matching
/// the client's per-roll integer truncation.
fn avg_hit_for_range(bmin: i32, bmax: i32, mult: f32) -> f32 {
    let (lo, hi) = if bmax > bmin {
        (bmin, bmax - 1)
    } else {
        (bmin, bmin)
    };
    let n = (hi - lo + 1) as f64;
    let mut sum = 0.0f64;
    for d in lo..=hi {
        sum += (d as f32 * mult) as i32 as f64;
    }
    (sum / n) as f32
}

/// Effective value of an ability's scaling stat, from the post-gear stats.
fn scaling_value(stat: ScalingStat, e: &EffectiveStats) -> i32 {
    match stat {
        ScalingStat::Wisdom => e.wisdom,
        ScalingStat::Attack => e.attack,
        ScalingStat::Defense => e.defense,
        ScalingStat::Dexterity => e.dexterity,
        ScalingStat::Vitality => e.vitality,
        ScalingStat::Speed => e.speed,
        ScalingStat::MaxMp => e.max_mp,
    }
}

fn scaling_label(stat: ScalingStat) -> &'static str {
    match stat {
        ScalingStat::Wisdom => "WIS",
        ScalingStat::Attack => "ATT",
        ScalingStat::Defense => "DEF",
        ScalingStat::Dexterity => "DEX",
        ScalingStat::Vitality => "VIT",
        ScalingStat::Speed => "SPD",
        ScalingStat::MaxMp => "MP",
    }
}

/// Theoretical damage of the equipped ability item (slot 1) for one use,
/// scaled by the current effective set. `None` when the ability deals no
/// self-computable damage (non-damaging abilities, unknown items).
fn derive_ability(
    assets: &AssetManager,
    loadout: &Loadout,
    eff: &EffectiveStats,
) -> Option<AbilityDamage> {
    let item_id = *loadout.equipment.get(1)?;
    if item_id <= 0 {
        return None;
    }
    let effects = assets.ability_effects(item_id)?;
    ability_damage_from(&effects, item_id, eff)
}

/// Core ability-damage aggregation, split out so it can be tested without a
/// loaded asset manager.
fn ability_damage_from(
    effects: &crate::assets::AbilityEffects,
    item_id: i32,
    eff: &EffectiveStats,
) -> Option<AbilityDamage> {
    if !effects.deals_damage() {
        return None;
    }

    // Guaranteed on-use poison: sum every grenade thrown per use, using each
    // grenade's own scaling stat (they may differ, e.g. Murky Toxin DEX+ATT).
    let mut poison_impact = 0;
    let mut poison_dot = 0;
    let mut poison_duration = 0.0f32;
    for g in &effects.poison_grenades {
        let sv = g.scaling_stat.map(|s| scaling_value(s, eff)).unwrap_or(0);
        poison_impact += g.impact_for(sv);
        poison_dot += g.dot_over_time_for(sv);
        poison_duration = poison_duration.max(g.duration_s);
    }

    // For display label, use the first available scaling stat.
    let scaling = effects
        .poison_grenades
        .first()
        .and_then(|g| g.scaling_stat)
        .or_else(|| effects.detonate_hex.and_then(|d| d.scaling_stat));
    let sv = scaling.map(|s| scaling_value(s, eff)).unwrap_or(0);

    let (detonate_base, detonate_per_stack) = match effects.detonate_hex {
        Some(d) => {
            // The blast may scale off a different stat than the poison; resolve
            // its own (falling back to the poison's stat if unspecified).
            let dv = d.scaling_stat.map(|s| scaling_value(s, eff)).unwrap_or(sv);
            (Some(d.base_for(dv)), Some(d.per_stack_for(dv)))
        }
        None => (None, None),
    };

    Some(AbilityDamage {
        item_id,
        scaling_label: scaling.map(scaling_label).unwrap_or(""),
        scaling_value: sv,
        poison_impact,
        poison_dot,
        poison_duration,
        detonate_base,
        detonate_per_stack,
    })
}

/// Round a projectile-damage average for a compact display total.
fn avg_i(min: i32, max: i32) -> i32 {
    (min + max + 1) / 2
}

/// Per-enemy hex-stack cap. Weapon hits build hex on an enemy up to this many
/// stacks; the hex-detonation blast's theoretical maximum is measured here.
const MAX_HEX_STACKS: u32 = 100;

/// Build the display-ready ability readout for the equipped ability item (slot
/// 1), computing every parsed effect against the current effective stats.
fn derive_ability_readout(
    assets: &AssetManager,
    loadout: &Loadout,
    eff: &EffectiveStats,
) -> Option<AbilityReadout> {
    let item_id = *loadout.equipment.get(1)?;
    if item_id <= 0 {
        return None;
    }
    let effects = assets.ability_effects(item_id)?;
    let mut readout = ability_readout_from(&effects, item_id, eff);

    // Lethal Strike bonus (rogue cloaks): per-weapon-shot bonus damage during
    // the 2.4s window after exiting sneak. Shown as an info line since it
    // modifies weapon hits, not ability damage itself.
    if let Some(ls) = assets.lethal_strike_params(item_id) {
        let stat = ls.scaling_stat;
        let stat_val = scaling_value(stat, eff) as f32;
        let w = (stat_val - ls.stat_mod_scaling_min).max(0.0);
        let flat = (ls.ignore_flat + ls.stat_mod_flat * w).floor() as i32;
        let flat_bonus = (ls.stat_mod_flat * w).floor() as i32;
        let perc = ls.ignore_perc + ls.stat_mod_perc * w;
        let perc_bonus = ls.stat_mod_perc * w;

        readout.lethal_strike = Some(LethalStrikeReadout {
            flat,
            flat_bonus,
            perc,
            perc_bonus,
            scaling_stat: stat,
            extra_shots: ls.extra_shots,
            duration: ls.duration,
        });
        let label = scaling_label(stat);
        if !readout.scales_off.contains(&label) {
            readout.scales_off.push(label);
        }
    }

    if readout.is_empty() {
        None
    } else {
        Some(readout)
    }
}

/// Core readout builder, split out for testing without a loaded asset manager.
fn ability_readout_from(
    effects: &crate::assets::AbilityEffects,
    item_id: i32,
    eff: &EffectiveStats,
) -> AbilityReadout {
    use self::AbilitySegment as Seg;

    let mut scales_off: Vec<&'static str> = Vec::new();
    let mut note_scale = |s: ScalingStat, scales_off: &mut Vec<&'static str>| {
        let label = scaling_label(s);
        if !scales_off.contains(&label) {
            scales_off.push(label);
        }
    };
    let mut lines: Vec<AbilityLine> = Vec::new();

    // Build "<total> (+<bonus>)<suffix>" segments, coloring the bonus by `stat`.
    let scaled = |total: i32, bonus: i32, stat: ScalingStat, suffix: &str| -> Vec<Seg> {
        let mut v = vec![Seg::plain(total.to_string())];
        if bonus > 0 {
            v.push(Seg::scaled(format!(" (+{})", bonus), stat));
        }
        if !suffix.is_empty() {
            v.push(Seg::plain(suffix.to_string()));
        }
        v
    };

    // Guaranteed on-use poison (Assassin/orb family).
    if !effects.poison_grenades.is_empty() {
        for g in &effects.poison_grenades {
            if let Some(s) = g.scaling_stat {
                note_scale(s, &mut scales_off);
            }
        }
        let mut impact = 0;
        let mut impact_base = 0;
        let mut dot = 0;
        let mut dot_base = 0;
        let mut dur = 0.0f32;
        for g in &effects.poison_grenades {
            let sv = g.scaling_stat.map(|s| scaling_value(s, eff)).unwrap_or(0);
            let floor = g.stat_mod_scaling_min as i32;
            impact += g.impact_for(sv);
            impact_base += g.impact_for(floor);
            dot += g.dot_over_time_for(sv);
            dot_base += g.dot_over_time_for(floor);
            dur = dur.max(g.duration_s);
        }
        let impact_bonus = impact - impact_base;
        let dot_bonus = dot - dot_base;
        let total_bonus = impact_bonus + dot_bonus;
        // Use the first grenade's stat for coloring the bonus (most items
        // share a single stat); multi-stat poisons still show correct totals.
        let primary_stat = effects.poison_grenades.iter().find_map(|g| g.scaling_stat);
        let mut segs = match primary_stat {
            Some(s) => scaled(impact, impact_bonus, s, " impact + "),
            None => vec![Seg::plain(format!("{} impact + ", impact))],
        };
        let dot_segs = match primary_stat {
            Some(s) if total_bonus > 0 => scaled(dot, dot_bonus, s, &format!(" over {:.0}s", dur)),
            _ => vec![Seg::plain(format!("{} over {:.0}s", dot, dur))],
        };
        segs.extend(dot_segs);
        lines.push(AbilityLine::dmg("Poison", segs));
    }

    // Projectile bursts (Wizard spell, orb hex-consume bursts). For orbs prefer
    // the lowest hex-threshold burst as the representative on-consume damage.
    if let Some(pb) = effects
        .effects
        .iter()
        .filter_map(|e| match e {
            AbilityEffect::ProjectileBurst { min_hex, burst, .. } => Some((*min_hex, burst)),
            _ => None,
        })
        .min_by_key(|(min_hex, _)| *min_hex)
        .map(|(_, b)| *b)
    {
        let sv = pb.scaling_stat.map(|s| scaling_value(s, eff)).unwrap_or(0);
        let floor = pb.stat_mod_scaling_min as i32;
        let (mn, mx, shots) = (pb.min_for(sv), pb.max_for(sv), pb.shots_for(sv));
        if mx > 0 && shots > 0 {
            let total = avg_i(mn, mx) * shots;
            let ap = if pb.armor_piercing { " AP" } else { "" };
            let mut segs = vec![Seg::plain(format!("{}-{} x{}", mn, mx, shots))];
            if let Some(s) = pb.scaling_stat {
                note_scale(s, &mut scales_off);
                let base_total = avg_i(pb.min_for(floor), pb.max_for(floor)) * pb.shots_for(floor);
                segs.push(Seg::plain("  (~".to_string()));
                segs.push(Seg::plain(total.to_string()));
                let bonus = total - base_total;
                if bonus > 0 {
                    segs.push(Seg::scaled(format!(" (+{})", bonus), s));
                }
                segs.push(Seg::plain(format!(" total{})", ap)));
            } else {
                segs.push(Seg::plain(format!("  (~{} total{})", total, ap)));
            }
            lines.push(AbilityLine::dmg("Damage", segs));
        }
    }

    for e in &effects.effects {
        match e {
            AbilityEffect::ProjectileBurst { .. } => {} // handled above
            AbilityEffect::Restore(r) => {
                let s = r.scaling_stat;
                if let Some(s) = s {
                    note_scale(s, &mut scales_off);
                }
                let sv = s.map(|s| scaling_value(s, eff)).unwrap_or(0);
                let amt = r.amount_for(sv);
                if amt > 0 {
                    let unit = if r.is_mp { " MP" } else { " HP" };
                    let segs = match s {
                        Some(s) => {
                            let base = r.amount.round().max(0.0) as i32;
                            scaled(amt, amt - base, s, unit)
                        }
                        None => vec![Seg::plain(format!("{}{}", amt, unit))],
                    };
                    lines.push(AbilityLine::info(if r.is_mp { "MP" } else { "Heal" }, segs));
                }
            }
            AbilityEffect::Lightning(l) => {
                lightning_lines(l, eff, &mut lines, &mut note_scale, &mut scales_off);
            }
            AbilityEffect::Trap(t) => {
                let s = t.scaling_stat;
                if let Some(s) = s {
                    note_scale(s, &mut scales_off);
                }
                let sv = s.map(|s| scaling_value(s, eff)).unwrap_or(0);
                let floor = t.stat_mod_scaling_min as i32;
                let trap = t.trap_damage_for(sv);
                let bomb = t.bomb_damage_for(sv);
                let mut segs = match s {
                    Some(s) => scaled(trap, trap - t.trap_damage_for(floor), s, " dmg"),
                    None => vec![Seg::plain(format!("{} dmg", trap))],
                };
                if bomb > 0 {
                    segs.push(Seg::plain("  +".to_string()));
                    match s {
                        Some(s) => {
                            segs.extend(scaled(bomb, bomb - t.bomb_damage_for(floor), s, " bomb"))
                        }
                        None => segs.push(Seg::plain(format!("{} bomb", bomb))),
                    }
                }
                lines.push(AbilityLine::dmg("Trap", segs));
            }
            AbilityEffect::VampireBlast(vb) => {
                let scales = vb.wis_damage_base > 0.0;
                if scales {
                    note_scale(ScalingStat::Wisdom, &mut scales_off);
                }
                let dmg = vb.damage_for(eff.wisdom);
                let base = vb.total_damage.round().max(0.0) as i32;
                let mut segs = if scales {
                    scaled(dmg, dmg - base, ScalingStat::Wisdom, " dmg")
                } else {
                    vec![Seg::plain(format!("{} dmg", dmg))]
                };
                if vb.ignore_def > 0.0 {
                    segs.push(Seg::plain(format!(" (-{} DEF)", vb.ignore_def as i32)));
                }
                if vb.heal > 0.0 {
                    segs.push(Seg::plain(format!("  +{} HP", vb.heal as i32)));
                }
                lines.push(AbilityLine::dmg("Blast", segs));
            }
            AbilityEffect::DamageNova(n) => {
                let (segs, stat_val) = match n.scaling_stat {
                    Some(s) => {
                        note_scale(s, &mut scales_off);
                        let sv = scaling_value(s, eff);
                        let per = n.per_hit(sv);
                        (scaled(per, n.per_hit_bonus(sv), s, ""), sv)
                    }
                    None => (vec![Seg::plain(n.per_hit(0).to_string())], 0),
                };
                let mut segs = segs;
                segs.push(Seg::plain(format!(
                    " x{} in {:.0} sq",
                    n.activations(stat_val),
                    n.radius(stat_val).round()
                )));
                lines.push(AbilityLine::dmg("Nova dmg", segs));
            }
            AbilityEffect::StatBoost(b) => {
                let sign = if b.amount >= 0 { "+" } else { "" };
                lines.push(AbilityLine::plain(
                    "Buff",
                    format!("{}{} {} for {:.0}s", sign, b.amount, b.stat, b.duration),
                ));
            }
            AbilityEffect::Condition(c) => {
                lines.push(AbilityLine::plain(
                    c.effect.clone(),
                    format!("{:.0}s", c.duration),
                ));
            }
            AbilityEffect::Decoy { duration, .. } => {
                lines.push(AbilityLine::plain("Decoy", format!("{:.0}s", duration)));
            }
            AbilityEffect::Teleport { max_distance } => {
                lines.push(AbilityLine::plain(
                    "Teleport",
                    format!("{:.0} tiles", max_distance),
                ));
            }
            AbilityEffect::EffectBlast(eb) => {
                // Collect hex-stack-gated durations for this condition.
                let mut durations: Vec<f32> = vec![eb.base_duration];
                for tier in &effects.condition_hex_tiers {
                    if tier.condition == eb.condition && !durations.contains(&tier.duration) {
                        durations.push(tier.duration);
                    }
                }
                durations.sort_by(|a, b| a.partial_cmp(b).unwrap());
                durations.dedup();

                // WIS scaling on duration (often negligible, e.g. wisPerDuration=1000).
                let wis = eff.wisdom as f32;
                let wis_extra = if eb.wis_per_duration > 0.0 {
                    (wis - eb.wis_min).max(0.0) / eb.wis_per_duration
                } else {
                    0.0
                };

                let dur_str = if durations.len() > 1 {
                    durations
                        .iter()
                        .map(|d| format!("{:.0}", d + wis_extra))
                        .collect::<Vec<_>>()
                        .join("/")
                } else {
                    format!("{:.1}", durations[0] + wis_extra)
                };
                let mut segs = vec![Seg::plain(format!("{}s", dur_str))];

                // Only show WIS scaling when it materially affects duration.
                if wis_extra >= 0.1 {
                    if !scales_off.contains(&"WIS") {
                        scales_off.push("WIS");
                    }
                    segs.push(Seg::scaled(
                        format!(" (+{:.1}s from WIS)", wis_extra),
                        ScalingStat::Wisdom,
                    ));
                }

                // EffectBlast shares the orb's DetonateHex radius scaling.
                let effective_radius = if let Some(d) = effects.detonate_hex {
                    let sv = d.scaling_stat.map(|s| scaling_value(s, eff)).unwrap_or(0);
                    d.radius_for(sv)
                } else {
                    eb.radius
                };
                segs.push(Seg::plain(format!("  r{:.1}", effective_radius)));
                lines.push(AbilityLine::info(eb.condition.clone(), segs));
            }
            AbilityEffect::SpawnCreep(sc) => {
                if let Some(s) = sc.scaling_stat {
                    note_scale(s, &mut scales_off);
                }
                let sv = sc.scaling_stat.map(|s| scaling_value(s, eff)).unwrap_or(0);
                let bonus =
                    (sc.stat_mod_damage * (sv as f32 - sc.stat_mod_scaling_min).max(0.0)).floor();

                for proj in &sc.projectiles {
                    let min_total = (proj.min_damage + bonus).round() as i32;
                    let max_total = (proj.max_damage + bonus).round() as i32;

                    let mut segs = vec![Seg::plain(format!("{}-{}", min_total, max_total))];
                    if bonus > 0.0 {
                        if let Some(s) = sc.scaling_stat {
                            segs.push(Seg::scaled(format!(" (+{})", bonus as i32), s));
                        }
                    }
                    if proj.armor_piercing {
                        segs.push(Seg::plain(" AP".to_string()));
                    }

                    let label = if proj.name.is_empty() {
                        sc.display_name.clone()
                    } else {
                        format!("{} ({})", sc.display_name, proj.name)
                    };
                    lines.push(AbilityLine::info(label, segs));
                }
            }
            AbilityEffect::Shuriken(sh) => {
                if let Some(s) = sh.scaling_stat {
                    note_scale(s, &mut scales_off);
                }
                let sv = sh.scaling_stat.map(|s| scaling_value(s, eff)).unwrap_or(0);
                let bonus =
                    (sh.stat_mod_damage * (sv as f32 - sh.stat_mod_scaling_min).max(0.0)).floor();
                let min_total = (sh.min_damage + bonus).round() as i32;
                let max_total = (sh.max_damage + bonus).round() as i32;

                let mut segs = vec![Seg::plain(format!("{}-{}", min_total, max_total))];
                if bonus > 0.0 {
                    if let Some(s) = sh.scaling_stat {
                        segs.push(Seg::scaled(format!(" (+{})", bonus as i32), s));
                    }
                }
                if sh.armor_piercing {
                    segs.push(Seg::plain(" AP".to_string()));
                }
                if sh.num_projectiles > 1 {
                    segs.push(Seg::plain(format!(" x{}", sh.num_projectiles)));
                }
                lines.push(AbilityLine::dmg("Shuriken", segs));
            }
            AbilityEffect::DashTrail { damage } => {
                lines.push(AbilityLine::dmg(
                    "Dash trail",
                    vec![Seg::plain(damage.to_string())],
                ));
            }
        }
    }

    // Hex-detonation blast (orb family), reported after the burst.
    let mut hex_blast = None;
    if let Some(d) = effects.detonate_hex {
        let s = d.scaling_stat;
        if let Some(s) = s {
            note_scale(s, &mut scales_off);
        }
        let dv = s.map(|s| scaling_value(s, eff)).unwrap_or(0);
        let floor = d.stat_mod_scaling_min as i32;
        let base = d.base_for(dv);
        let per = d.per_stack_for(dv);
        if base > 0 || per > 0 {
            let mut segs = match s {
                Some(s) => scaled(base, base - d.base_for(floor), s, " base"),
                None => vec![Seg::plain(format!("{} base", base))],
            };
            segs.push(Seg::plain(format!("  +{}/stack", per)));
            lines.push(AbilityLine::dmg("Hex blast", segs));

            let max_damage = d.damage_for(dv, MAX_HEX_STACKS);
            let max_damage_bonus = (max_damage - d.damage_for(floor, MAX_HEX_STACKS)).max(0);
            hex_blast = Some(HexBlastReadout {
                scaling_stat: s,
                scaling_value: dv,
                scaling_min: floor,
                base,
                per_stack: per,
                stack_multiplier: d.stack_multiplier,
                max_stacks: MAX_HEX_STACKS,
                max_damage,
                max_damage_bonus,
            });
        }
    }

    let total = ability_potential_damage(effects, eff, false);
    let damage_per_use = if total > 0 {
        Some(AbilityDamagePerUse {
            total,
            scaled: ability_stat_shares(effects, eff, &scales_off, false),
            single_target: ability_potential_damage(effects, eff, true),
            single_scaled: ability_stat_shares(effects, eff, &scales_off, true),
        })
    } else {
        None
    };

    AbilityReadout {
        item_id,
        scales_off,
        lines,
        damage_per_use,
        hex_blast,
        lethal_strike: None,
    }
}

/// Attribute each scaling stat's share of the potential-damage total by
/// re-computing with that stat removed. Only damage-affecting stats (positive
/// share) appear, in scales-off order.
fn ability_stat_shares(
    effects: &crate::assets::AbilityEffects,
    eff: &EffectiveStats,
    scales_off: &[&'static str],
    single_target: bool,
) -> Vec<(ScalingStat, i32)> {
    let total = ability_potential_damage(effects, eff, single_target);
    let mut scaled: Vec<(ScalingStat, i32)> = Vec::new();
    for &label in scales_off {
        if let Some(stat) = stat_from_label(label) {
            let share = total
                - ability_potential_damage(effects, &with_stat_zeroed(eff, stat), single_target);
            if share > 0 {
                scaled.push((stat, share));
            }
        }
    }
    scaled
}

/// Total potential damage for one ability use. When `single_target` is false,
/// every component lands on its maximum number of targets (best-case AoE); when
/// true, everything lands on one enemy (boss case). Excludes the hex-detonation
/// blast, whose damage depends on weapon-built stacks.
fn ability_potential_damage(
    effects: &crate::assets::AbilityEffects,
    eff: &EffectiveStats,
    single_target: bool,
) -> i32 {
    let mut total = 0;

    // Guaranteed on-use poison (impact + full DoT), summed across grenades.
    // Poison lands on the primary target, so it is the same in either case.
    for g in &effects.poison_grenades {
        let sv = g.scaling_stat.map(|s| scaling_value(s, eff)).unwrap_or(0);
        total += g.impact_for(sv) + g.dot_over_time_for(sv);
    }

    // Representative projectile burst (lowest hex threshold). All shots are
    // assumed to land on one enemy, identical in both cases.
    if let Some(pb) = effects
        .effects
        .iter()
        .filter_map(|e| match e {
            AbilityEffect::ProjectileBurst { min_hex, burst, .. } => Some((*min_hex, *burst)),
            _ => None,
        })
        .min_by_key(|(min_hex, _)| *min_hex)
        .map(|(_, b)| b)
    {
        let sv = pb.scaling_stat.map(|s| scaling_value(s, eff)).unwrap_or(0);
        total += avg_i(pb.min_for(sv), pb.max_for(sv)) * pb.shots_for(sv);
    }

    for e in &effects.effects {
        match e {
            AbilityEffect::Lightning(l) => {
                let stat = l.scaling_stat.unwrap_or(ScalingStat::Wisdom);
                let sv = scaling_value(stat, eff);
                if single_target {
                    // One enemy: only the first chain hit, plus every shockblast
                    // trigger landing on that same target.
                    total += l.chain_damage(eff.wisdom);
                    if l.has_shockblast() {
                        total += l.shock_damage(sv);
                    }
                } else {
                    total += l.chain_total(eff.wisdom);
                    total += l.shock_total(sv, eff.wisdom);
                }
            }
            AbilityEffect::Trap(t) => {
                let sv = t.scaling_stat.map(|s| scaling_value(s, eff)).unwrap_or(0);
                total += t.trap_damage_for(sv) + t.bomb_damage_for(sv);
            }
            AbilityEffect::VampireBlast(vb) => {
                total += vb.damage_for(eff.wisdom);
            }
            AbilityEffect::DamageNova(n) => {
                // Radius burst on one enemy; no target cap, so single == multi.
                let sv = n.scaling_stat.map(|s| scaling_value(s, eff)).unwrap_or(0);
                total += n.total(sv);
            }
            AbilityEffect::Shuriken(sh) => {
                let sv = sh.scaling_stat.map(|s| scaling_value(s, eff)).unwrap_or(0);
                let bonus =
                    (sh.stat_mod_damage * (sv as f32 - sh.stat_mod_scaling_min).max(0.0)).floor();
                let avg = ((sh.min_damage + sh.max_damage) / 2.0 + bonus).round() as i32;
                total += avg * sh.num_projectiles;
            }
            AbilityEffect::DashTrail { damage } => {
                total += *damage;
            }
            _ => {}
        }
    }

    // Hex-detonation blast at the per-enemy stack cap: the best case against a
    // fully-hexed target. Folded into the max-potential estimate (the enemy is
    // assumed to carry `MAX_HEX_STACKS`, built up by weapon hits).
    if let Some(d) = effects.detonate_hex {
        let sv = d.scaling_stat.map(|s| scaling_value(s, eff)).unwrap_or(0);
        total += d.damage_for(sv, MAX_HEX_STACKS);
    }

    total
}

/// Reverse of `scaling_label`, mapping a scales-off label back to its stat.
fn stat_from_label(label: &str) -> Option<ScalingStat> {
    match label {
        "WIS" => Some(ScalingStat::Wisdom),
        "ATT" => Some(ScalingStat::Attack),
        "DEF" => Some(ScalingStat::Defense),
        "DEX" => Some(ScalingStat::Dexterity),
        "VIT" => Some(ScalingStat::Vitality),
        "SPD" => Some(ScalingStat::Speed),
        "MP" => Some(ScalingStat::MaxMp),
        _ => None,
    }
}

/// A copy of `eff` with one scaling stat set to zero, for isolating that stat's
/// contribution to a damage total.
fn with_stat_zeroed(eff: &EffectiveStats, stat: ScalingStat) -> EffectiveStats {
    let mut e = *eff;
    match stat {
        ScalingStat::Wisdom => e.wisdom = 0,
        ScalingStat::Attack => e.attack = 0,
        ScalingStat::Defense => e.defense = 0,
        ScalingStat::Dexterity => e.dexterity = 0,
        ScalingStat::Vitality => e.vitality = 0,
        ScalingStat::Speed => e.speed = 0,
        ScalingStat::MaxMp => e.max_mp = 0,
    }
    e
}

/// Emit the chain + shockblast lines for a `Lightning` scepter. Chain damage and
/// all target counts scale off WIS; only the shockblast damage scales off the
/// item's `scaling_stat` (WIS on most scepters, ATT on e.g. Scepter of
/// Devastation).
fn lightning_lines(
    l: &crate::assets::LightningEffect,
    eff: &EffectiveStats,
    lines: &mut Vec<AbilityLine>,
    note_scale: &mut impl FnMut(ScalingStat, &mut Vec<&'static str>),
    scales_off: &mut Vec<&'static str>,
) {
    use self::AbilitySegment as Seg;
    let wis = eff.wisdom;

    // Chain damage (off WIS).
    let cdmg = l.chain_damage(wis);
    if cdmg > 0 {
        note_scale(ScalingStat::Wisdom, scales_off);
        let mut segs = vec![Seg::plain(cdmg.to_string())];
        let cbonus = l.chain_damage_bonus(wis);
        if cbonus > 0 {
            segs.push(Seg::scaled(format!(" (+{})", cbonus), ScalingStat::Wisdom));
        }
        if l.decr_damage > 0.0 {
            segs.push(Seg::plain(format!(
                "  -{}/extra target",
                l.decr_damage as i32
            )));
        }
        lines.push(AbilityLine::dmg("Chain dmg", segs));
    }

    // Chain targets (off WIS).
    let ctargets = l.chain_targets(wis);
    let mut tsegs = vec![Seg::plain(ctargets.to_string())];
    let ctbonus = l.chain_targets_bonus(wis);
    if ctbonus > 0 {
        note_scale(ScalingStat::Wisdom, scales_off);
        tsegs.push(Seg::scaled(format!(" (+{})", ctbonus), ScalingStat::Wisdom));
    }
    tsegs.push(Seg::plain(format!(" target{}", plural(ctargets))));
    lines.push(AbilityLine::dmg("Chain targets", tsegs));

    // Shockblast (aoe): damage off the scaling stat, targets/radius off WIS.
    if l.has_shockblast() {
        let stat = l.scaling_stat.unwrap_or(ScalingStat::Wisdom);
        let sv = scaling_value(stat, eff);
        let sdmg = l.shock_damage(sv);
        let sbonus = l.shock_damage_bonus(sv);
        if sdmg > 0 {
            note_scale(stat, scales_off);
            let mut segs = vec![Seg::plain(sdmg.to_string())];
            if sbonus > 0 {
                segs.push(Seg::scaled(format!(" (+{})", sbonus), stat));
            }
            segs.push(Seg::plain(format!(
                "  x{} over {:.2} sq",
                l.shock_triggers(),
                l.shock_radius(wis)
            )));
            lines.push(AbilityLine::dmg("Shockblast dmg", segs));
        }

        let starg = l.shock_targets(wis);
        let mut segs = vec![Seg::plain(starg.to_string())];
        let stbonus = l.shock_targets_bonus(wis);
        if stbonus > 0 {
            note_scale(ScalingStat::Wisdom, scales_off);
            segs.push(Seg::scaled(format!(" (+{})", stbonus), ScalingStat::Wisdom));
        }
        segs.push(Seg::plain(format!(" target{}", plural(starg))));
        lines.push(AbilityLine::dmg("Shockblast targets", segs));
    }
}

fn plural(n: i32) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn downgrade(p: Provenance) -> Provenance {
    match p {
        Provenance::Full => Provenance::Partial,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combat_trigger_brackets() {
        // RealmEye reference examples.
        assert_eq!(combat_trigger(12), 12);
        assert_eq!(combat_trigger(25), 22); // 15 + 10*0.75 = 22.5 -> 22
        assert_eq!(combat_trigger(45), 35); // 30 + 10*0.5 = 35
        assert_eq!(combat_trigger(77), 48); // 45 + 12*0.25 = 48
                                            // Wizard 2142 (51 effective DEF) matches the in-game 38.
        assert_eq!(combat_trigger(51), 38); // 30 + 16*0.5 = 38
                                            // Cap at 60 for DEF >= 125.
        assert_eq!(combat_trigger(125), 60);
        assert_eq!(combat_trigger(200), 60);
        assert_eq!(combat_trigger(0), 0);
    }

    #[test]
    fn regen_matches_reference_values() {
        // 75 VIT / 75 WIS out of combat: 20 HP/s, 10 MP/s.
        assert!((hp_regen_per_sec(75) - 20.0).abs() < 1e-4);
        assert!((mp_regen_per_sec(75) - 10.0).abs() < 1e-4);
        assert!((hp_regen_per_sec(0) - 2.0).abs() < 1e-4);
        assert!((mp_regen_per_sec(0) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn fire_rate_scales_with_dex() {
        assert!((shots_per_sec(0, 1.0) - 1.5).abs() < 1e-4);
        assert!((shots_per_sec(75, 1.0) - 8.0).abs() < 1e-4);
        assert!((shots_per_sec(75, 2.0) - 16.0).abs() < 1e-4);
    }

    #[test]
    fn ability_poison_matches_cornea_tooltip() {
        // Command Cornea poison scales off MAXMP above 300: the in-game tooltip
        // gives impact = 1000 + 1.5*(MP-300) and total = 1500 + 5*(MP-300).
        use crate::assets::{AbilityEffects, PoisonGrenadeEffect, ScalingStat};
        let grenade = PoisonGrenadeEffect {
            radius: 5.0,
            impact_damage: 1000.0,
            total_damage: 1500.0,
            duration_s: 5.0,
            scaling_stat: Some(ScalingStat::MaxMp),
            stat_mod_scaling_min: 300.0,
            stat_mod_damage: 50.0,
            stat_mod_impact_damage: 15.0,
            stat_mod_radius: 0.0,
        };
        let effects = AbilityEffects {
            applies_hex: true,
            detonate_hex: None,
            poison_grenades: vec![grenade],
            on_detonate_poison: vec![],
            condition_hex_tiers: vec![],
            effects: vec![],
        };
        let mut eff = EffectiveStats::default();
        eff.max_mp = 765; // sb = 465
        let ab = ability_damage_from(&effects, 0x5cbe, &eff).expect("cornea damages");
        // impact = 1000 + 1.5*465 = 1697.5 -> 1698
        assert_eq!(ab.poison_impact, 1698);
        // total = 1500 + 5*465 = 3825; dot = 3825 - 1698 = 2127
        assert_eq!(ab.poison_dot, 2127);
        assert_eq!(ab.poison_total(), 3825);
        assert_eq!(ab.scaling_label, "MP");
        assert_eq!(ab.scaling_value, 765);
    }

    #[test]
    fn ability_none_when_no_damage() {
        use crate::assets::AbilityEffects;
        let effects = AbilityEffects::default();
        let ab = ability_damage_from(&effects, 1, &EffectiveStats::default());
        assert!(ab.is_none());
    }

    #[test]
    fn conquest_projectile_burst_scales_off_def() {
        // Orb of Conquest: <Projectile>120-180</Projectile>, BulletNova numShots=4,
        // scalingStat=DEF statModScalingMin=20 statModDamage=2. The in-game tooltip
        // "540 base damage" is the max per-shot at DEF 200 (180 + 2*(200-20)).
        use crate::assets::ProjectileBurstEffect;
        let b = ProjectileBurstEffect {
            min_damage: 120.0,
            max_damage: 180.0,
            num_shots: 4,
            armor_piercing: false,
            scaling_stat: Some(ScalingStat::Defense),
            stat_mod_scaling_min: 20.0,
            stat_mod_damage: 2.0,
            stat_mod_num_shots: 0.00001,
        };
        assert_eq!(b.min_for(200), 480);
        assert_eq!(b.max_for(200), 540);
        assert_eq!(b.shots_for(200), 4);
        // At the scaling floor there is no bonus.
        assert_eq!(b.max_for(20), 180);
    }

    #[test]
    fn tome_heal_scales_off_wis() {
        // Tome Heal: amount=100 statModAmount=0.77 scalingStat=WIS statModScalingMin=70.
        // Direct (no /10 divisor), matching in-game tome tooltips.
        use crate::assets::RestoreEffect;
        let r = RestoreEffect {
            amount: 100.0,
            is_mp: false,
            scaling_stat: Some(ScalingStat::Wisdom),
            stat_mod_scaling_min: 70.0,
            stat_mod_amount: 0.77,
        };
        assert_eq!(r.amount_for(70), 100);
        assert_eq!(r.amount_for(170), 177); // 100 + 0.77*100
    }

    #[test]
    fn scepter_lightning_first_target_scales() {
        // Scepter of Fulmination (verified vs. in-game tooltip at WIS 79):
        // totalDamage=150 wisDamageBase=60 maxTargets=5 wisPerTarget=6
        // aoeDamage=60 aoeActivationCount=3 aoeMaxTargets=2 aoeRange=1.5
        // scalingStat=WIS statModScalingMin=50 statModDamage=14
        // statModPerTarget=10 statModRadius=0.05.
        use crate::assets::LightningEffect;
        let l = LightningEffect {
            total_damage: 150.0,
            decr_damage: 30.0,
            max_targets: 5,
            wis_damage_base: 60.0,
            wis_min: 0.0,
            wis_per_target: 6.0,
            aoe_damage: 60.0,
            aoe_activation_count: 3,
            aoe_max_targets: 2,
            aoe_range: 1.5,
            scaling_stat: Some(ScalingStat::Wisdom),
            stat_mod_scaling_min: 50.0,
            stat_mod_damage: 14.0,
            stat_mod_per_target: 10.0,
            stat_mod_radius: 0.05,
        };
        // Below the floor: base values only.
        assert_eq!(l.chain_damage(50), 150);
        assert_eq!(l.chain_targets(50), 5);
        // WIS 79 -> sb = 29.
        assert_eq!(l.chain_damage(79), 324); // 150 + 6*29
        assert_eq!(l.chain_targets(79), 9); // 5 + floor(29/6)
        assert!(l.has_shockblast());
        assert_eq!(l.shock_damage(79), 300); // floor(60 + 1.4*29) * 3
        assert_eq!(l.shock_targets(79), 4); // 2 + floor(29*10/100)
        assert!((l.shock_radius(79) - 2.95).abs() < 1e-4);
    }

    #[test]
    fn scepter_potential_damage_summary() {
        // Same Scepter of Fulmination at WIS 79. The potential-damage summary is
        // chain_total + shock_total, with the WIS share isolated by zeroing WIS.
        use crate::assets::{AbilityEffect, AbilityEffects, LightningEffect};
        let l = LightningEffect {
            total_damage: 150.0,
            decr_damage: 30.0,
            max_targets: 5,
            wis_damage_base: 60.0,
            wis_min: 0.0,
            wis_per_target: 6.0,
            aoe_damage: 60.0,
            aoe_activation_count: 3,
            aoe_max_targets: 2,
            aoe_range: 1.5,
            scaling_stat: Some(ScalingStat::Wisdom),
            stat_mod_scaling_min: 50.0,
            stat_mod_damage: 14.0,
            stat_mod_per_target: 10.0,
            stat_mod_radius: 0.05,
        };
        // Chain: 324,294,...,84 over 9 targets = 1836. Shock: 300 * 4 = 1200.
        assert_eq!(l.chain_total(79), 1836);
        assert_eq!(l.shock_total(79, 79), 1200); // shock_damage(79)=300 * 4 targets

        let effects = AbilityEffects {
            effects: vec![AbilityEffect::Lightning(l)],
            ..Default::default()
        };
        let mut eff = EffectiveStats::default();
        eff.wisdom = 79;
        let ro = ability_readout_from(&effects, 7, &eff);
        let dmg = ro.damage_per_use.expect("scepter deals damage");
        assert_eq!(dmg.total, 3036); // 1836 + 1200
                                     // At WIS 0: chain 450 + shock 360 = 810, so WIS share = 3036 - 810.
        assert_eq!(dmg.scaled, vec![(ScalingStat::Wisdom, 2226)]);
        // Single target: first chain hit 324 + all shock triggers on it 300.
        assert_eq!(dmg.single_target, 624);
        // At WIS 0: chain 150 + shock 180 = 330, so WIS share = 624 - 330.
        assert_eq!(dmg.single_scaled, vec![(ScalingStat::Wisdom, 294)]);
        // Damage lines stay in `lines` but are tagged for the hover breakdown.
        assert!(ro
            .lines
            .iter()
            .any(|l| l.label == "Chain dmg" && l.is_damage));
    }

    #[test]
    fn damage_nova_scales_off_wis() {
        // Real Sporous Spray (Wizard, WIS 77): activationCount=4 min=max=800
        // radius=3 scalingStat=WIS statModScalingMin=42 statModDamage=9
        // statModActivationCount=0.0002 statModRadius=0.035. sb=35.
        use crate::assets::{AbilityEffect, AbilityEffects, DamageNovaEffect};
        let n = DamageNovaEffect {
            activation_count: 4.0,
            time: 1.2,
            min_damage: 800.0,
            max_damage: 800.0,
            radius: 3.0,
            scaling_stat: Some(ScalingStat::Wisdom),
            stat_mod_scaling_min: 42.0,
            stat_mod_damage: 9.0,
            stat_mod_activation_count: 0.0002,
            stat_mod_radius: 0.035,
        };
        assert_eq!(n.per_hit(77), 1115); // 800 + 9*35
        assert_eq!(n.per_hit_bonus(77), 315);
        assert_eq!(n.activations(77), 4); // 4 + floor(0.007)
        assert_eq!(n.total(77), 4460); // 1115 * 4

        let effects = AbilityEffects {
            effects: vec![AbilityEffect::DamageNova(n)],
            ..Default::default()
        };
        let mut eff = EffectiveStats::default();
        eff.wisdom = 77;
        let ro = ability_readout_from(&effects, 2142, &eff);
        assert_eq!(ro.scales_off, vec!["WIS"]);
        let dmg = ro.damage_per_use.expect("nova deals damage");
        assert_eq!(dmg.total, 4460);
        // No target cap: single-target equals multi-target.
        assert_eq!(dmg.single_target, 4460);
        // At WIS 0: per_hit 800 * 4 = 3200, so WIS share = 4460 - 3200 = 1260.
        assert_eq!(dmg.scaled, vec![(ScalingStat::Wisdom, 1260)]);
        assert!(ro
            .lines
            .iter()
            .any(|l| l.label == "Nova dmg" && l.is_damage));
    }

    #[test]
    fn damage_nova_scales_off_spd() {
        // Slurpian Sea Scroll / Depth Charge scale off SPD, verifying the new
        // Speed scaling stat is recognized and routed to EffectiveStats.speed.
        use crate::assets::{AbilityEffect, AbilityEffects, DamageNovaEffect};
        let n = DamageNovaEffect {
            activation_count: 2.0,
            time: 0.6,
            min_damage: 650.0,
            max_damage: 950.0,
            radius: 3.0,
            scaling_stat: Some(ScalingStat::Speed),
            stat_mod_scaling_min: 42.0,
            stat_mod_damage: 6.0,
            stat_mod_activation_count: 0.04,
            stat_mod_radius: 0.035,
        };
        // sb = 75 - 42 = 33. per_hit = avg(650,950)=800 + 6*33 = 998.
        assert_eq!(n.per_hit(75), 998);
        // activations = 2 + floor(0.04*33=1.32) = 3.
        assert_eq!(n.activations(75), 3);

        let effects = AbilityEffects {
            effects: vec![AbilityEffect::DamageNova(n)],
            ..Default::default()
        };
        let mut eff = EffectiveStats::default();
        eff.speed = 75;
        let ro = ability_readout_from(&effects, 0xdead, &eff);
        assert_eq!(ro.scales_off, vec!["SPD"]);
        let dmg = ro.damage_per_use.expect("nova deals damage");
        assert_eq!(dmg.total, 2994); // 998 * 3
        assert_eq!(dmg.scaled, vec![(ScalingStat::Speed, dmg.total - 800 * 2)]);
    }

    #[test]
    fn tome_heal_scales_off_vit() {
        // Real Tome of Purification: Heal amount=150 statModAmount=0.935
        // scalingStat="VIT" statModScalingMin=45. Verifies the Vitality scaling
        // stat is recognized and routed to EffectiveStats.vitality.
        use crate::assets::{AbilityEffect, AbilityEffects, RestoreEffect};
        let effects = AbilityEffects {
            effects: vec![AbilityEffect::Restore(RestoreEffect {
                amount: 150.0,
                is_mp: false,
                scaling_stat: Some(ScalingStat::Vitality),
                stat_mod_scaling_min: 45.0,
                stat_mod_amount: 0.935,
            })],
            ..Default::default()
        };
        let mut eff = EffectiveStats::default();
        eff.vitality = 145;
        let ro = ability_readout_from(&effects, 0xffee, &eff);
        assert_eq!(ro.scales_off, vec!["VIT"]);
        assert_eq!(ro.lines[0].label, "Heal");
        // 150 + 0.935 * (145 - 45) = 243.5 -> 244, bonus +94 colored by VIT.
        let text: String = ro.lines[0]
            .segments
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(text, "244 (+94) HP");
        assert_eq!(ro.lines[0].segments[1].stat, Some(ScalingStat::Vitality));
    }

    #[test]
    fn readout_lists_scaling_stat_and_lines() {
        use crate::assets::{AbilityEffect, AbilityEffects, RestoreEffect};
        let effects = AbilityEffects {
            effects: vec![AbilityEffect::Restore(RestoreEffect {
                amount: 100.0,
                is_mp: false,
                scaling_stat: Some(ScalingStat::Wisdom),
                stat_mod_scaling_min: 70.0,
                stat_mod_amount: 0.77,
            })],
            ..Default::default()
        };
        let mut eff = EffectiveStats::default();
        eff.wisdom = 170;
        let ro = ability_readout_from(&effects, 42, &eff);
        assert_eq!(ro.scales_off, vec!["WIS"]);
        assert_eq!(ro.lines.len(), 1);
        assert_eq!(ro.lines[0].label, "Heal");
        // 100 + 0.77 * (170 - 70) = 177, bonus +77 colored by WIS.
        let text: String = ro.lines[0]
            .segments
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(text, "177 (+77) HP");
        assert_eq!(ro.lines[0].segments[1].stat, Some(ScalingStat::Wisdom));
    }

    #[test]
    fn crucible_stat_mods_fold_into_attributes() {
        use crate::api::crucible::{CrucibleDef, CrucibleStatMod};
        use crate::assets::StatKind;
        let def = CrucibleDef {
            id: "s30".into(),
            title: "S30 Crucible".into(),
            stat_mods: vec![
                CrucibleStatMod {
                    stat: StatKind::Attack,
                    value: 5.0,
                    percent: false,
                },
                CrucibleStatMod {
                    stat: StatKind::Defense,
                    value: 10.0,
                    percent: false,
                },
            ],
            damage_mult: 1.0,
            ..Default::default()
        };
        let loadout = Loadout {
            base: BaseStats {
                attack: 70,
                defense: 20,
                ..Default::default()
            },
            crucible: Some(def),
            ..Default::default()
        };
        let d = derive(&AssetManager::new(), &loadout);
        // +5 ATT / +10 DEF fold into the totals and show in the crucible bracket.
        assert_eq!(d.attributes.attack.total(), 75);
        assert_eq!(d.attributes.attack.crucible, 5);
        assert_eq!(d.attributes.defense.total(), 30);
        assert_eq!(d.attributes.defense.crucible, 10);
        // Gear bracket stays clean (crucible is not folded into gear).
        assert_eq!(d.attributes.attack.gear_display(), 0);
        assert!(d.crucible.is_some());
    }

    #[test]
    fn crucible_absent_leaves_stats_unchanged() {
        let loadout = Loadout {
            base: BaseStats {
                attack: 70,
                defense: 20,
                ..Default::default()
            },
            crucible: None,
            ..Default::default()
        };
        let d = derive(&AssetManager::new(), &loadout);
        assert_eq!(d.attributes.attack.total(), 70);
        assert_eq!(d.attributes.attack.crucible, 0);
        assert_eq!(d.attributes.defense.total(), 20);
        assert!(d.crucible.is_none());
    }

    #[test]
    fn attack_multiplier_reference() {
        // 75 ATT, no exalt: (75+25)*0.02 = 2.0.
        assert!((attack_multiplier(75, None) - 2.0).abs() < 1e-4);
        // +250 exalt bonus: 2.0 * 1.25 = 2.5.
        assert!((attack_multiplier(75, Some(250)) - 2.5).abs() < 1e-4);
    }

    #[test]
    fn pet_heal_anchors() {
        // Divine heal level 100 = 90 HP / 1.0s = 90 HP/s.
        let c = pet_contribution(407, 100);
        assert!((c.hp_per_sec - 90.0).abs() < 1e-3);
        // Magic heal level 30 = 3 MP / 3.82s.
        let m = pet_contribution(408, 30);
        assert!((m.mp_per_sec - 3.0 / 3.82).abs() < 1e-3);
    }

    #[test]
    fn interp_between_anchors() {
        // Heal amount at level 40 is between 14 (30) and 23 (50): 18.5.
        assert!((interp(HEAL_AMOUNT, 40) - 18.5).abs() < 1e-4);
    }

    #[test]
    fn relative_bonus_reads_frozen_flat_snapshot() {
        // Base 50 ATT, +10 flat gear ATT, then a relative enchant granting 50%
        // of bonus ATT -> +5 (from the 10 flat), floored effective 65.
        let base = base_array(&BaseStats {
            attack: 50,
            ..Default::default()
        });
        let mut flat = base;
        add_stat(&mut flat, StatKind::Attack, 10.0);
        let flat_bonus: [f32; 8] = std::array::from_fn(|i| flat[i] - base[i]);
        let mut eff = flat;
        eff[stat_index(StatKind::Attack)] +=
            flat_bonus[stat_index(StatKind::Attack)] * 50.0 / 100.0;
        assert!((eff[stat_index(StatKind::Attack)] - 65.0).abs() < 1e-4);
    }

    #[test]
    fn weapon_roll_range_is_max_exclusive() {
        // "10-14" weapon rolls 10..=13 (max exclusive). With a 1.0 multiplier
        // the mean is (10+11+12+13)/4 = 11.5, NOT (10+14)/2 = 12.
        let w = WeaponDamage {
            weapon_id: 1,
            min_damage: 10,
            max_damage: 14,
            armor_piercing: false,
            avg_hit: 11.5,
            num_projectiles: 1,
            total_projectiles: 1,
            extra_types: Vec::new(),
            shots_per_sec: 1.0,
            dps: 11.5,
            stored_mult: 1.0,
            damage_mult: 1.0,
            mastery_pct: 0.0,
            procs: Vec::new(),
            proc_dps: 0.0,
        };
        // At 0 defense dps_vs equals the mean roll.
        assert!((w.dps_vs(0) - 11.5).abs() < 1e-4);
        // Defense clips each roll but keeps AP off; 5 DEF -> rolls 5,6,7,8 -> 6.5.
        assert!((w.dps_vs(5) - 6.5).abs() < 1e-4);
    }

    #[test]
    fn fixed_damage_weapon_rolls_single_value() {
        let w = WeaponDamage {
            weapon_id: 1,
            min_damage: 100,
            max_damage: 100,
            armor_piercing: false,
            avg_hit: 100.0,
            num_projectiles: 2,
            total_projectiles: 2,
            extra_types: Vec::new(),
            shots_per_sec: 1.0,
            dps: 200.0,
            stored_mult: 1.0,
            damage_mult: 1.0,
            mastery_pct: 0.0,
            procs: Vec::new(),
            proc_dps: 0.0,
        };
        assert!((w.dps_vs(0) - 200.0).abs() < 1e-4);
    }

    #[test]
    fn mastery_floors_raw_weapon_damage() {
        // Wand of Retribution base 165-205 with Exaltation Mastery +10% shows
        // 181-225 in-game: floor(165*1.10)=181, floor(205*1.10)=225.
        let mastery = 1.0 + 10.0 / 100.0;
        let bmin = ((165.0f32 * 1.0).round() * mastery).floor() as i32;
        let bmax = ((205.0f32 * 1.0).round() * mastery).floor() as i32;
        assert_eq!((bmin, bmax), (181, 225));
    }

    #[test]
    fn proc_activation_rate_cooldown_limits_guaranteed_proc() {
        // proc=1, cd=3s, 2 shots/s -> locked = ceil(6) = 6 intervals -> 2/6.
        let r = proc_activation_rate(1.0, 3.0, 2.0);
        assert!((r - 2.0 / 6.0).abs() < 1e-4, "got {r}");
        // Cooldown shorter than the shot interval -> fires every shot.
        let r2 = proc_activation_rate(1.0, 0.4, 2.0);
        assert!((r2 - 2.0).abs() < 1e-4, "got {r2}");
    }

    #[test]
    fn proc_activation_rate_edge_cases() {
        // No cooldown -> every shot rolls the proc chance.
        assert!((proc_activation_rate(0.5, 0.0, 4.0) - 2.0).abs() < 1e-4);
        // Zero proc chance or zero fire rate -> no activations.
        assert_eq!(proc_activation_rate(0.0, 1.0, 4.0), 0.0);
        assert_eq!(proc_activation_rate(1.0, 1.0, 0.0), 0.0);
        // Probabilistic + cooldown: p=0.5, cd=1s, 4 shots/s ->
        // locked = ceil(4) = 4, waiting = (1-0.5)/0.5 = 1 -> 4/5 = 0.8.
        assert!((proc_activation_rate(0.5, 1.0, 4.0) - 0.8).abs() < 1e-4);
    }

    #[test]
    fn proc_dps_is_defense_independent() {
        // A 1000-damage poison proc every 3s at 2 shots/s -> ~333 dps, folded
        // into both dps and dps_vs, and unchanged by target defense.
        let proc_dps = 1000.0 * (2.0 / 6.0);
        let w = WeaponDamage {
            weapon_id: 1,
            min_damage: 10,
            max_damage: 14,
            armor_piercing: false,
            avg_hit: 11.5,
            num_projectiles: 1,
            total_projectiles: 1,
            extra_types: Vec::new(),
            shots_per_sec: 1.0,
            dps: 11.5 + proc_dps,
            stored_mult: 1.0,
            damage_mult: 1.0,
            mastery_pct: 0.0,
            procs: vec![WeaponProcReadout {
                label: "Poison",
                damage_per_proc: 1000,
                cooldown: 3.0,
                proc_rate: 1.0,
                activations_per_sec: 2.0 / 6.0,
                dps: proc_dps,
                armor_piercing: true,
                continuous: false,
            }],
            proc_dps,
        };
        // At 0 DEF: projectile mean 11.5 + poison.
        assert!((w.dps_vs(0) - (11.5 + proc_dps)).abs() < 1e-3);
        // At 5 DEF the projectile part drops to 6.5 but poison is unchanged.
        assert!((w.dps_vs(5) - (6.5 + proc_dps)).abs() < 1e-3);
    }
}

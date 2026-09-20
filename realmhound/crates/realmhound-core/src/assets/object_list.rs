//! Object and tile asset definitions.
//!
//! Parses ObjectID.list and TileID.list files generated from RotMG game assets.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use super::stat_bonus::{StatBonuses, StatKind};

/// Display-name overrides, keyed by item ID.
///
/// Some items share an in-game `display_name` with another item despite being
/// distinct:
/// - id 37689: the rare "troll" Fool's Prism variant (id_name "Fooled Prism")
///   shares display_name "Fool's Prism" with the real item.
/// - id 1230: a legacy grey-release Sword of the Colossus Shiny, no longer
///   obtainable, shares display_name with the current Sword of the Colossus Shiny.
/// - id 37518: a legacy non-droppable Trap of the Vile Spirit variant,
///   mistakenly released as a craftable shiny and no longer obtainable,
///   shares display_name with the current Trap of the Vile Spirit.
/// - id 5338: Vial of Soul Extract Shiny shares display_name with non-shiny 23743.
/// - ids 17564/5346/17565: Killer Bee Queen's Beehemoth Quiver (+ shiny) and
///   Armor drop the plain "Beehemoth" display_name in-game, but are commonly
///   known as "Green Beehemoth" (id_name still tags them "EH Green ...").
fn name_override(item_id: i32) -> Option<&'static str> {
    match item_id {
        37689 => Some("Fooled Prism"),
        1230 => Some("Legacy Sword of the Colossus Shiny"),
        37518 => Some("Not Shiny Trap of the Vile Spirit"),
        5338 => Some("Vial of Soul Extract Shiny"),
        17564 => Some("Green Beehemoth Quiver"),
        5346 => Some("Green Beehemoth Quiver (Shiny)"),
        17565 => Some("Green Beehemoth Armor"),
        _ => None,
    }
}

/// Sprite-texture overrides, keyed by object id, for objects whose intended
/// display sprite is NOT their first listed texture.
///
/// RotMG flattens every texture an object references into one list, and
/// RealmHound renders the first entry ([`ObjectAsset::primary_texture`]). A few
/// setpiece bosses list their grave / spawner art first and the actual
/// character sprite last, so the first texture is the wrong art:
/// - id 21943 (New Kage Kami): lists its graveyard/spawner tiles first; the
///   real character sprite is `deadChurchChars16x16` at the end of the list.
/// - ids 22019 / 3423 (New Pentaract / Pentaract encounter): render as the tiny
///   Pentaract Eye by default; show the Pentaract Tower sprite instead
///   (`lofiChar216x16:0x38`, matching object 3422 "Pentaract Tower").
/// - id 23509 (Overseer Eyesmall / SpecPen Overseer Minion 1): its texture list
///   starts with `invisible:0`, so the aggregated row renders blank; show the
///   first real eyesmall body frame (`specPenObjects8x8:316`) instead.
/// - id 33120 (Tablet of the Monarchy / Shatters Gauntlet Objective): its
///   primary texture is the first "Rise" animation frame (a barely-risen
///   sliver, `theShattersObjects16x16:0x2be`); show the fully-risen "Full"
///   frame (`0x2c7`) that matches the tablet's in-game/RealmEye look.
///
/// Returns the `(sheet, index)` to render instead.
pub fn sprite_texture_override(id: i32) -> Option<(&'static str, i32)> {
    match id {
        21943 => Some(("deadChurchChars16x16", 3)),
        22019 | 3423 => Some(("lofiChar216x16", 0x38)),
        23509 => Some(("specPenObjects8x8", 316)),
        33120 => Some(("theShattersObjects16x16", 0x2c7)),
        _ => None,
    }
}

/// Texture reference within a sprite atlas.
#[derive(Debug, Clone)]
pub struct ObjectTexture {
    /// Sprite sheet name (e.g., "lofiObj", "lofiObj2")
    pub name: String,
    /// Index within the sprite sheet
    pub index: i32,
}

/// Projectile damage data for weapons.
#[derive(Debug, Clone)]
pub struct Projectile {
    /// Minimum damage
    pub min_damage: i32,
    /// Maximum damage
    pub max_damage: i32,
    /// Whether projectile pierces armor
    pub armor_piercing: bool,
    /// Slot type for the weapon
    pub slot_type: i32,
    /// Display name from XML `displayId` attribute (summon projectile variants).
    pub display_id: Option<String>,
    /// Whether this projectile is hidden from the in-game tooltip.
    pub ignore_on_tooltip: bool,
}

/// Lethal Strike proc parameters from a cloak's `OnConditionEndActivate`
/// element in equip.xml. Present only on rogue cloaks that grant the buff.
///
/// The buff adds bonus damage to each shot fired during the 2.4s window after
/// exiting sneak, and spawns two side projectiles that each deal that same
/// bonus. The bonus vs a target with defense `d`, at effective wisdom `wis`, is:
///
/// ```text
/// w    = max(0, wis - stat_mod_scaling_min)
/// flat = ignore_flat + stat_mod_flat * w
/// perc = ignore_perc + stat_mod_perc * w
/// bonus = flat + perc * d
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LethalStrikeParams {
    pub ignore_flat: f32,
    pub ignore_perc: f32,
    pub stat_mod_flat: f32,
    pub stat_mod_perc: f32,
    pub stat_mod_scaling_min: f32,
    /// The stat that drives the scaling bonus (usually WIS, but some UT cloaks
    /// use DEF or other stats).
    pub scaling_stat: ScalingStat,
    /// Duration of the Lethal Strike window in seconds (from the
    /// `OnConditionEndActivate` `duration` attribute).
    pub duration: f32,
    /// Number of extra projectiles fired per weapon shot during the LS window
    /// (count of `OnPlayerShootActivate BulletCreate` entries on this item).
    pub extra_shots: i32,
}

/// The player stat that scales an ability effect, from the equip.xml
/// `scalingStat` attribute. Read from assets so stat dependencies are never
/// hardcoded (e.g. tiered orbs scale WIS, Command Cornea scales MAXMP, Orb of
/// Conflict scales ATT, Enchantment Orb scales DEX, Orb of Conquest scales DEF).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalingStat {
    Wisdom,
    Attack,
    Defense,
    Dexterity,
    Vitality,
    Speed,
    MaxMp,
}

impl ScalingStat {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "WIS" | "WISDOM" => Some(Self::Wisdom),
            "ATT" | "ATTACK" => Some(Self::Attack),
            "DEF" | "DEFENSE" => Some(Self::Defense),
            "DEX" | "DEXTERITY" => Some(Self::Dexterity),
            "VIT" | "VITALITY" => Some(Self::Vitality),
            "SPD" | "SPEED" => Some(Self::Speed),
            "MAXMP" | "MAXMANA" => Some(Self::MaxMp),
            _ => None,
        }
    }

    /// Short display label matching the stat potion abbreviations.
    pub fn label(self) -> &'static str {
        match self {
            Self::Wisdom => "WIS",
            Self::Attack => "ATT",
            Self::Defense => "DEF",
            Self::Dexterity => "DEX",
            Self::Vitality => "VIT",
            Self::Speed => "SPD",
            Self::MaxMp => "MP",
        }
    }
}

/// `DetonateHex` AoE from an orb's `<Activate>` element. On ability use, every
/// enemy within `radius` of the target takes damage scaled by the hex stacks it
/// carries. This blast is NOT armor-piercing (regular orb AoE respects defense).
///
/// Damage vs an enemy carrying `stacks` hex, at effective scaling stat `s`:
///
/// ```text
/// sb   = max(0, s - stat_mod_scaling_min)
/// base = base_damage  + stat_mod_base_damage  * sb
/// per  = stack_damage + stat_mod_stack_damage * sb
/// dmg  = (base + per * stacks) * (1 + stack_multiplier * stacks)
/// r    = radius + stat_mod_radius * sb
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetonateHexEffect {
    pub radius: f32,
    pub base_damage: f32,
    pub stack_damage: f32,
    pub stack_multiplier: f32,
    pub scaling_stat: Option<ScalingStat>,
    pub stat_mod_scaling_min: f32,
    pub stat_mod_base_damage: f32,
    pub stat_mod_stack_damage: f32,
    pub stat_mod_radius: f32,
}

impl DetonateHexEffect {
    fn stat_bonus(&self, stat: i32) -> f32 {
        (stat as f32 - self.stat_mod_scaling_min).max(0.0)
    }

    /// Effective blast radius (tiles) at scaling stat `stat`.
    pub fn radius_for(&self, stat: i32) -> f32 {
        self.radius + self.stat_mod_radius * self.stat_bonus(stat)
    }

    /// Pre-defense AoE damage to one enemy carrying `stacks` hex.
    pub fn damage_for(&self, stat: i32, stacks: u32) -> i32 {
        let sb = self.stat_bonus(stat);
        let base = self.base_damage + self.stat_mod_base_damage * sb;
        let per = self.stack_damage + self.stat_mod_stack_damage * sb;
        let n = stacks as f32;
        ((base + per * n) * (1.0 + self.stack_multiplier * n))
            .round()
            .max(0.0) as i32
    }

    /// Base blast damage (0 hex stacks) at scaling stat `stat`.
    pub fn base_for(&self, stat: i32) -> i32 {
        (self.base_damage + self.stat_mod_base_damage * self.stat_bonus(stat))
            .round()
            .max(0.0) as i32
    }

    /// Additional blast damage per hex stack at scaling stat `stat` (the
    /// `stackMultiplier` compounding, if any, is not folded in here).
    pub fn per_stack_for(&self, stat: i32) -> i32 {
        (self.stack_damage + self.stat_mod_stack_damage * self.stat_bonus(stat))
            .round()
            .max(0.0) as i32
    }
}

/// `PoisonGrenade` from an orb's `<Activate>` (or `<OnDetonateHexActivate>`)
/// element. `total_damage` is the GRAND total dealt to each enemy in radius,
/// of which `impact_damage` lands immediately and the remainder is a
/// damage-over-time across `duration_s` seconds. Poison is armor-piercing and,
/// if applied before an enemy turns invulnerable, keeps ticking through invuln.
///
/// Stat scaling (verified against Command Cornea's in-game tooltip): the two
/// DAMAGE stat mods are applied at 1/10 per stat point above
/// `stat_mod_scaling_min`, while the radius stat mod is applied directly:
///
/// ```text
/// sb     = max(0, stat - stat_mod_scaling_min)
/// impact = impact_damage + (stat_mod_impact_damage / 10) * sb
/// total  = total_damage  + (stat_mod_damage        / 10) * sb   // impact included
/// dot    = total - impact                                       // over duration_s
/// radius = radius + stat_mod_radius * sb
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PoisonGrenadeEffect {
    pub radius: f32,
    pub impact_damage: f32,
    pub total_damage: f32,
    pub duration_s: f32,
    pub scaling_stat: Option<ScalingStat>,
    pub stat_mod_scaling_min: f32,
    pub stat_mod_damage: f32,
    pub stat_mod_impact_damage: f32,
    pub stat_mod_radius: f32,
}

/// Divisor applied to `PoisonGrenade` damage stat mods (per stat point). The
/// game's Command Cornea tooltip computes impact `1000 + 1.5*(MP-300)` from
/// `statModImpactDamage=15` and total `1500 + 5*(MP-300)` from
/// `statModDamage=50`, i.e. the raw XML stat mod divided by 10.
const POISON_STAT_MOD_DIVISOR: f32 = 10.0;

/// Interval between poison damage-over-time ticks (one server game tick).
const POISON_TICK_S: f32 = 0.2;

impl PoisonGrenadeEffect {
    fn stat_bonus(&self, stat: i32) -> f32 {
        (stat as f32 - self.stat_mod_scaling_min).max(0.0)
    }

    pub fn radius_for(&self, stat: i32) -> f32 {
        self.radius + self.stat_mod_radius * self.stat_bonus(stat)
    }

    /// Immediate impact damage (armor-piercing) at scaling stat `stat`.
    pub fn impact_for(&self, stat: i32) -> i32 {
        (self.impact_damage
            + (self.stat_mod_impact_damage / POISON_STAT_MOD_DIVISOR) * self.stat_bonus(stat))
        .round()
        .max(0.0) as i32
    }

    /// Grand total damage (impact + DoT) delivered to one enemy, at stat `stat`.
    pub fn total_for(&self, stat: i32) -> i32 {
        (self.total_damage
            + (self.stat_mod_damage / POISON_STAT_MOD_DIVISOR) * self.stat_bonus(stat))
        .round()
        .max(0.0) as i32
    }

    /// The damage-over-time portion (grand total minus the immediate impact),
    /// spread across `duration_s`.
    pub fn dot_over_time_for(&self, stat: i32) -> i32 {
        (self.total_for(stat) - self.impact_for(stat)).max(0)
    }

    /// Damage a single application actually DELIVERS to one enemy at `stat`.
    ///
    /// The DoT ticks once per 200ms server tick and the final expiry tick never
    /// lands, so a poison of `duration_s` fires `ceil(duration/0.2) - 1` ticks
    /// instead of the nominal `duration/0.2`. A 1.0s poison therefore delivers
    /// only 4 of its 5 nominal DoT ticks (4/5 of the DoT). Verified against live
    /// captures: Toxic Toad at ATT 74 delivers 868, not the nominal 990.
    pub fn delivered_total_for(&self, stat: i32) -> i32 {
        let impact = self.impact_for(stat);
        let dot_nominal = (self.total_for(stat) - impact).max(0);
        if self.duration_s <= 0.0 || dot_nominal == 0 {
            return impact + dot_nominal;
        }
        // Ticks at k*0.2 for k >= 1 while k*0.2 < duration (expiry tick dropped).
        let ticks_fired = ((self.duration_s / POISON_TICK_S) - 1e-4).floor().max(0.0);
        let delivered_dot = dot_nominal as f32 * (POISON_TICK_S * ticks_fired / self.duration_s);
        impact + delivered_dot.round().max(0.0) as i32
    }
}

/// What triggers a native item proc (`OnPlayerShootActivate` /
/// `OnEnemyHitActivate` in equip.xml).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcTrigger {
    /// Fires when the player shoots (`OnPlayerShootActivate`).
    OnShoot,
    /// Fires when a shot hits an enemy (`OnEnemyHitActivate`).
    OnHit,
}

/// A `Bleeding` condition applied by a weapon's projectile: an armor-piercing
/// damage-over-time that drains a fixed `dmg_per_sec` HP while active. Bleeding
/// is a binary on/off condition -- it does NOT stack and does NOT scale with
/// stats -- so under sustained fire (reapplied faster than it expires) it just
/// contributes a flat `dmg_per_sec` to weapon DPS.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BleedingEffect {
    /// HP drained per second while the condition is active (default 20 when the
    /// XML omits `amount`).
    pub dmg_per_sec: f32,
    /// How long a single application keeps the condition active, in seconds.
    pub duration_s: f32,
}

/// The damaging payload of a native item proc. Source-agnostic so the same
/// model can back both native weapon procs (equip.xml) and, later, enchantment
/// procs (the `Add*Activate` mutators in enchantments.xml).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProcEffect {
    /// A thrown poison burst (armor-piercing impact + DoT).
    Poison(PoisonGrenadeEffect),
    /// A projectile-applied `Bleeding` condition (armor-piercing flat DoT).
    Bleeding(BleedingEffect),
}

/// A native item proc parsed from an `OnPlayerShootActivate` /
/// `OnEnemyHitActivate` element: a damaging effect that fires on a trigger,
/// throttled by `proc_rate` (0..1 activation chance) and `cooldown` (seconds).
#[derive(Debug, Clone, PartialEq)]
pub struct WeaponProc {
    pub trigger: ProcTrigger,
    /// Activation chance per eligible trigger, clamped to `[0, 1]`.
    pub proc_rate: f32,
    /// Seconds between activations; `0` means it can fire on every trigger.
    pub cooldown: f32,
    /// Gating condition (e.g. `LethalStrike`, `Berserk`); `None` for an
    /// unconditional proc. Conditional procs are excluded from steady-state DPS.
    pub required_conditions: Option<String>,
    pub effect: ProcEffect,
}

/// A `PoisonGrenade` that fires only when a `DetonateHex` consumes a hex count in
/// `[min_hex, max_hex]` on an enemy (Command Cornea's per-threshold poison
/// bursts).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HexThresholdPoison {
    pub min_hex: u32,
    pub max_hex: u32,
    pub grenade: PoisonGrenadeEffect,
}

/// A projectile-burst ability (`Shoot` / `BulletNova`) whose per-shot damage is
/// the item's `<Projectile>` range plus a flat stat-mod bonus per point of the
/// scaling stat above a floor. Covers Wizard's spell bomb, orb hex-consume
/// bursts (e.g. Orb of Conquest's spiraling projectiles), etc.
///
/// Per-shot damage at effective scaling stat `s`:
/// ```text
/// sb    = max(0, s - stat_mod_scaling_min)
/// dmg   = base_damage + stat_mod_damage * sb          // base = min..max projectile
/// shots = num_shots  + floor(stat_mod_num_shots * sb)
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProjectileBurstEffect {
    pub min_damage: f32,
    pub max_damage: f32,
    pub num_shots: i32,
    pub armor_piercing: bool,
    pub scaling_stat: Option<ScalingStat>,
    pub stat_mod_scaling_min: f32,
    pub stat_mod_damage: f32,
    pub stat_mod_num_shots: f32,
}

impl ProjectileBurstEffect {
    fn sb(&self, s: i32) -> f32 {
        (s as f32 - self.stat_mod_scaling_min).max(0.0)
    }
    /// Minimum per-shot damage at scaling stat `s`.
    pub fn min_for(&self, s: i32) -> i32 {
        (self.min_damage + self.stat_mod_damage * self.sb(s))
            .round()
            .max(0.0) as i32
    }
    /// Maximum per-shot damage at scaling stat `s`.
    pub fn max_for(&self, s: i32) -> i32 {
        (self.max_damage + self.stat_mod_damage * self.sb(s))
            .round()
            .max(0.0) as i32
    }
    /// Number of projectiles fired at scaling stat `s`.
    pub fn shots_for(&self, s: i32) -> i32 {
        (self.num_shots as f32 + (self.stat_mod_num_shots * self.sb(s)).floor()).max(0.0) as i32
    }
}

/// A `Heal` / `Magic` (MP) restore ability. `amount` is the base restore; some
/// tomes/seals scale it by `stat_mod_amount` per point of the scaling stat above
/// `stat_mod_scaling_min` (WIS on nearly all). Unlike poison, the stat mod is
/// applied directly (no /10 divisor), matching in-game tome tooltips.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RestoreEffect {
    pub amount: f32,
    pub is_mp: bool,
    pub scaling_stat: Option<ScalingStat>,
    pub stat_mod_scaling_min: f32,
    pub stat_mod_amount: f32,
}

impl RestoreEffect {
    /// Amount restored at scaling stat `s`.
    pub fn amount_for(&self, s: i32) -> i32 {
        let sb = (s as f32 - self.stat_mod_scaling_min).max(0.0);
        (self.amount + self.stat_mod_amount * sb).round().max(0.0) as i32
    }
}

/// A `Lightning` chain (Sorcerer scepter). It has two damage components:
///
/// - The **chain** ("Lightning") hits up to `max_targets` enemies for
///   `total_damage`, reduced by `decr_damage` per subsequent target. Its damage
///   scales off WIS via `wis_damage_base` (per 10 WIS points) and its target
///   count off WIS via `wis_per_target` (one extra target per that many points
///   above the floor).
/// - The **shockblast** ("aoe") triggers `aoe_activation_count` times, hitting
///   `aoe_max_targets` enemies within `aoe_range` squares for `aoe_damage` each
///   time. Its damage scales off the item's `scaling_stat` via `stat_mod_damage`
///   (per 10 stat points), its target count off WIS via `stat_mod_per_target`
///   (percent per point), and its radius off WIS via `stat_mod_radius`.
///
/// Formulas verified against the in-game Scepter of Fulmination tooltip
/// (effective WIS 79): chain 150 + 60/10*(79-50) = 324 damage over
/// 5 + floor(29/6) = 9 targets; shockblast floor(60 + 14/10*29) = 100 per
/// trigger * 3 = 300, over 2 + floor(29*10/100) = 4 targets within
/// 1.5 + 0.05*29 = 2.95 squares.
///
/// Every scaling component exposes `_base`/`_bonus` accessors so the UI can
/// render the current total plus its stat-colored scaling portion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightningEffect {
    pub total_damage: f32,
    pub decr_damage: f32,
    pub max_targets: i32,
    // WIS scaling for the chain component.
    pub wis_damage_base: f32,
    pub wis_min: f32,
    pub wis_per_target: f32,
    // Shockblast (aoe) component.
    pub aoe_damage: f32,
    pub aoe_activation_count: i32,
    pub aoe_max_targets: i32,
    pub aoe_range: f32,
    // scaling_stat drives ONLY the shockblast damage; chain damage, all target
    // counts, and radius always scale off WIS.
    pub scaling_stat: Option<ScalingStat>,
    pub stat_mod_scaling_min: f32,
    pub stat_mod_damage: f32,
    pub stat_mod_per_target: f32,
    pub stat_mod_radius: f32,
}

impl LightningEffect {
    /// WIS points above the chain's floor (`wis_min`, falling back to the shared
    /// `stat_mod_scaling_min` when the item omits an explicit `wisMin`).
    fn wis_sb(&self, wis: i32) -> f32 {
        let floor = if self.wis_min > 0.0 {
            self.wis_min
        } else {
            self.stat_mod_scaling_min
        };
        (wis as f32 - floor).max(0.0)
    }

    /// Scaling-stat points above the shockblast floor.
    fn stat_sb(&self, stat: i32) -> f32 {
        (stat as f32 - self.stat_mod_scaling_min).max(0.0)
    }

    /// Base (unscaled) first-target chain damage.
    pub fn chain_base_damage(&self) -> i32 {
        self.total_damage.round().max(0.0) as i32
    }

    /// WIS scaling bonus to first-target chain damage.
    pub fn chain_damage_bonus(&self, wis: i32) -> i32 {
        (self.wis_damage_base / 10.0 * self.wis_sb(wis))
            .floor()
            .max(0.0) as i32
    }

    /// First-target chain damage at effective WIS (base + bonus).
    pub fn chain_damage(&self, wis: i32) -> i32 {
        self.chain_base_damage() + self.chain_damage_bonus(wis)
    }

    /// Base (unscaled) chain target count.
    pub fn chain_base_targets(&self) -> i32 {
        self.max_targets.max(0)
    }

    /// WIS scaling bonus to the chain target count.
    pub fn chain_targets_bonus(&self, wis: i32) -> i32 {
        if self.wis_per_target > 0.0 {
            (self.wis_sb(wis) / self.wis_per_target).floor() as i32
        } else {
            0
        }
    }

    /// Chain target count at effective WIS.
    pub fn chain_targets(&self, wis: i32) -> i32 {
        self.chain_base_targets() + self.chain_targets_bonus(wis)
    }

    /// Whether this ability has a shockblast (aoe) component.
    pub fn has_shockblast(&self) -> bool {
        self.aoe_damage > 0.0 && self.aoe_activation_count > 0
    }

    fn shock_per_trigger(&self, stat: i32) -> i32 {
        (self.aoe_damage + self.stat_mod_damage / 10.0 * self.stat_sb(stat))
            .floor()
            .max(0.0) as i32
    }

    /// Base (unscaled) total shockblast damage across all triggers.
    pub fn shock_base_damage(&self) -> i32 {
        (self.aoe_damage * self.aoe_activation_count as f32)
            .round()
            .max(0.0) as i32
    }

    /// Scaling-stat bonus to total shockblast damage.
    pub fn shock_damage_bonus(&self, stat: i32) -> i32 {
        (self.shock_per_trigger(stat) * self.aoe_activation_count - self.shock_base_damage()).max(0)
    }

    /// Total shockblast damage across all triggers at the scaling stat.
    pub fn shock_damage(&self, stat: i32) -> i32 {
        self.shock_per_trigger(stat) * self.aoe_activation_count
    }

    /// Total chain damage across every target it can hit, applying the
    /// per-subsequent-target falloff (each target floored at 0). Assumes the
    /// chain reaches its maximum target count.
    pub fn chain_total(&self, wis: i32) -> i32 {
        let d = self.chain_damage(wis);
        let n = self.chain_targets(wis).max(0);
        let decr = self.decr_damage.max(0.0) as i32;
        (0..n).map(|i| (d - decr * i).max(0)).sum()
    }

    /// Total shockblast damage assuming it hits the maximum number of targets.
    /// `stat` is the effective value of the shockblast's `scaling_stat`.
    pub fn shock_total(&self, stat: i32, wis: i32) -> i32 {
        if !self.has_shockblast() {
            return 0;
        }
        self.shock_damage(stat) * self.shock_targets(wis).max(0)
    }

    pub fn shock_triggers(&self) -> i32 {
        self.aoe_activation_count.max(0)
    }

    pub fn shock_base_targets(&self) -> i32 {
        self.aoe_max_targets.max(0)
    }

    /// WIS scaling bonus to the shockblast target count.
    pub fn shock_targets_bonus(&self, wis: i32) -> i32 {
        if self.stat_mod_per_target > 0.0 {
            (self.wis_sb(wis) * self.stat_mod_per_target / 100.0).floor() as i32
        } else {
            0
        }
    }

    pub fn shock_targets(&self, wis: i32) -> i32 {
        self.shock_base_targets() + self.shock_targets_bonus(wis)
    }

    /// Shockblast radius (squares) at effective WIS.
    pub fn shock_radius(&self, wis: i32) -> f32 {
        self.aoe_range + self.stat_mod_radius * self.wis_sb(wis)
    }
}

/// A `Trap` (Huntress). `total_damage` lands when the trap triggers; an optional
/// delayed `bomb_damage` follows. Both scale per point above the floor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrapEffect {
    pub total_damage: f32,
    pub radius: f32,
    pub bomb_damage: f32,
    pub bomb_count: f32,
    pub scaling_stat: Option<ScalingStat>,
    pub stat_mod_scaling_min: f32,
    pub stat_mod_trap_damage: f32,
    pub stat_mod_bomb_damage: f32,
}

impl TrapEffect {
    fn sb(&self, s: i32) -> f32 {
        (s as f32 - self.stat_mod_scaling_min).max(0.0)
    }
    pub fn trap_damage_for(&self, s: i32) -> i32 {
        (self.total_damage + self.stat_mod_trap_damage * self.sb(s))
            .round()
            .max(0.0) as i32
    }
    pub fn bomb_damage_for(&self, s: i32) -> i32 {
        (self.bomb_damage + self.stat_mod_bomb_damage * self.sb(s))
            .round()
            .max(0.0) as i32
    }
}

/// A `VampireBlast` (skull-family). Deals `total_damage` and heals `heal`, both
/// scaling with WIS via the older `wis_damage_base` per point above `wis_min`
/// convention. `ignore_def` is armor ignored by the blast.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VampireBlastEffect {
    pub total_damage: f32,
    pub heal: f32,
    pub ignore_def: f32,
    pub radius: f32,
    pub wis_damage_base: f32,
    pub wis_min: f32,
}

impl VampireBlastEffect {
    /// Blast damage at WIS `wis` (older `wisDamageBase` per-point convention).
    pub fn damage_for(&self, wis: i32) -> i32 {
        let sb = (wis as f32 - self.wis_min).max(0.0);
        (self.total_damage + self.wis_damage_base * sb)
            .round()
            .max(0.0) as i32
    }
}

/// A `DamageNova` ability (Wizard spells, Priest tomes): a burst that emits
/// `activation_count` waves, each dealing `per_hit` damage to every enemy in
/// `radius` tiles. Damage/activations/radius scale off `scaling_stat` past
/// `stat_mod_scaling_min`. Unlike lightning shockblast, `stat_mod_damage` is
/// applied directly (no divide-by-ten).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DamageNovaEffect {
    pub activation_count: f32,
    pub time: f32,
    pub min_damage: f32,
    pub max_damage: f32,
    pub radius: f32,
    pub scaling_stat: Option<ScalingStat>,
    pub stat_mod_scaling_min: f32,
    pub stat_mod_damage: f32,
    pub stat_mod_activation_count: f32,
    pub stat_mod_radius: f32,
}

impl DamageNovaEffect {
    /// Stat points above the scaling threshold (never negative).
    fn stat_bonus(&self, stat: i32) -> f32 {
        (stat as f32 - self.stat_mod_scaling_min).max(0.0)
    }

    /// Damage of a single wave against one enemy at `stat`.
    pub fn per_hit(&self, stat: i32) -> i32 {
        let base = (self.min_damage + self.max_damage) / 2.0;
        (base + self.stat_mod_damage * self.stat_bonus(stat))
            .round()
            .max(0.0) as i32
    }

    /// Per-wave damage gained from scaling above the base average.
    pub fn per_hit_bonus(&self, stat: i32) -> i32 {
        (self.stat_mod_damage * self.stat_bonus(stat))
            .floor()
            .max(0.0) as i32
    }

    /// Number of waves emitted per use at `stat`.
    pub fn activations(&self, stat: i32) -> i32 {
        (self.activation_count + (self.stat_mod_activation_count * self.stat_bonus(stat)).floor())
            .max(0.0) as i32
    }

    /// Blast radius (tiles) at `stat`.
    pub fn radius(&self, stat: i32) -> f32 {
        self.radius + self.stat_mod_radius * self.stat_bonus(stat)
    }

    /// Total damage one enemy takes across every wave of a single use.
    pub fn total(&self, stat: i32) -> i32 {
        self.per_hit(stat) * self.activations(stat)
    }
}

/// A self stat buff (`StatBoostSelf`): grants `amount` of `stat` for `duration`
/// seconds. `amount` may be negative (debuff trade-offs).
#[derive(Debug, Clone, PartialEq)]
pub struct StatBoostEffect {
    pub stat: String,
    pub amount: i32,
    pub duration: f32,
}

/// A self condition effect (`ConditionEffectSelf` / `Sneak`): applies `effect`
/// for `duration` seconds (e.g. Berserk, Damaging, Invisible).
#[derive(Debug, Clone, PartialEq)]
pub struct ConditionSelfEffect {
    pub effect: String,
    pub duration: f32,
}

/// An area-of-effect condition blast (`EffectBlast`): applies a condition to
/// enemies in a radius for a WIS-scaled duration (Mystic orbs, some UTs).
#[derive(Debug, Clone, PartialEq)]
pub struct EffectBlastEffect {
    pub condition: String,
    pub base_duration: f32,
    /// WIS per extra second of duration (e.g. 20 = 1s per 20 WIS above floor).
    pub wis_per_duration: f32,
    /// WIS floor below which no scaling applies.
    pub wis_min: f32,
    pub radius: f32,
}

/// A single visible projectile variant of a summoned entity.
#[derive(Debug, Clone, PartialEq)]
pub struct SummonProjectile {
    /// Display name (e.g. "green", "Red") or empty for unnamed.
    pub name: String,
    pub min_damage: f32,
    pub max_damage: f32,
    pub armor_piercing: bool,
}

/// A summoned creature (`SpawnCreep`): spawns an entity that attacks enemies
/// for its lifetime (Summoner maces, some UTs).
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnCreepEffect {
    /// Display name of the summoned entity (e.g. "Bear", "Spider").
    pub display_name: String,
    /// Whether this is an undead (RaiseDead) vs a summon (SpawnCreep).
    pub is_undead: bool,
    /// Summon cost in MP-equivalent resource.
    pub summon_cost: f32,
    /// Visible projectile variants the summon can fire.
    pub projectiles: Vec<SummonProjectile>,
    /// Stat that scales the summon's damage.
    pub scaling_stat: Option<ScalingStat>,
    /// Damage bonus per stat point above the scaling floor.
    pub stat_mod_damage: f32,
    /// Stat floor below which no scaling applies.
    pub stat_mod_scaling_min: f32,
}

/// A parsed ability effect from an item's equip.xml `<Activate>` entries, in
/// addition to the orb-specific hex fields on `AbilityEffects`.
#[derive(Debug, Clone, PartialEq)]
pub enum AbilityEffect {
    /// Projectile burst (`Shoot`/`BulletNova`). For orbs, fired on hex consume
    /// within `[min_hex, max_hex]`; for spells, on every use (`0..=100`).
    ProjectileBurst {
        min_hex: u32,
        max_hex: u32,
        burst: ProjectileBurstEffect,
    },
    Restore(RestoreEffect),
    Lightning(LightningEffect),
    Trap(TrapEffect),
    VampireBlast(VampireBlastEffect),
    DamageNova(DamageNovaEffect),
    StatBoost(StatBoostEffect),
    Condition(ConditionSelfEffect),
    EffectBlast(EffectBlastEffect),
    SpawnCreep(SpawnCreepEffect),
    /// A simple duration-only effect (Decoy) with an optional movement speed.
    Decoy {
        duration: f32,
        speed: f32,
    },
    /// A blink/teleport with a maximum tile distance.
    Teleport {
        max_distance: f32,
    },
    /// Ninja star hold-and-release (`ShurikenAbility`): throws a projectile
    /// whose damage scales with a stat.
    Shuriken(ShurikenEffect),
    /// Kensei sheath dash trail damage (`ChannelDash` + `Dash`).
    DashTrail {
        damage: i32,
    },
}

/// A Ninja star (`ShurikenAbility`): hold to charge, release to throw.
/// Projectile damage = item's base `<Projectile>` range + stat-scaled bonus.
#[derive(Debug, Clone, PartialEq)]
pub struct ShurikenEffect {
    pub min_damage: f32,
    pub max_damage: f32,
    pub num_projectiles: i32,
    pub armor_piercing: bool,
    pub scaling_stat: Option<ScalingStat>,
    pub stat_mod_scaling_min: f32,
    pub stat_mod_damage: f32,
}

/// A hex-stack-gated condition duration override from `OnDetonateHexActivate
/// ApplyCondition`. When a `DetonateHex` consumes at least `min_hex` stacks,
/// this condition is applied for `duration` seconds (overriding the base
/// EffectBlast duration).
#[derive(Debug, Clone, PartialEq)]
pub struct ConditionHexTier {
    pub condition: String,
    pub min_hex: u32,
    pub duration: f32,
}

/// Locally-computable ability effects parsed from an item's equip.xml entry.
/// Used to self-compute AoE ability damage/heals/buffs that never appear in
/// packets, and to display an ability readout on the character stats page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AbilityEffects {
    /// Whether equipping this ability applies Hex on the player's enemy hits.
    pub applies_hex: bool,
    /// The on-use `DetonateHex` AoE, if any.
    pub detonate_hex: Option<DetonateHexEffect>,
    /// `PoisonGrenade`s thrown on every ability use.
    pub poison_grenades: Vec<PoisonGrenadeEffect>,
    /// Extra `PoisonGrenade`s fired per-enemy at specific consumed-hex thresholds.
    pub on_detonate_poison: Vec<HexThresholdPoison>,
    /// Hex-stack-gated condition overrides (e.g. Slowed at 25/50 stacks).
    pub condition_hex_tiers: Vec<ConditionHexTier>,
    /// Additional typed effects (damage/heal/buff/mobility) for the readout.
    pub effects: Vec<AbilityEffect>,
}

impl AbilityEffects {
    /// Whether this item self-computes any attributable damage.
    pub fn deals_damage(&self) -> bool {
        self.detonate_hex.is_some()
            || !self.poison_grenades.is_empty()
            || !self.on_detonate_poison.is_empty()
    }

    /// Whether this item produced any readable effect (damage, heal, buff, etc.).
    pub fn has_readout(&self) -> bool {
        self.deals_damage() || !self.effects.is_empty()
    }
}

/// Asset data for a game object (item, enemy, NPC, etc.).
#[derive(Debug, Clone)]
pub struct ObjectAsset {
    /// Object type ID (hex value from game)
    pub id: i32,
    /// Internal ID name (e.g., "SwordOfExample")
    pub id_name: String,
    /// Display name shown to players (e.g., "Sword of Example")
    pub display_name: String,
    /// Object class (Equipment, Character, Portal, etc.)
    pub class: String,
    /// Object group (Weapon, Armor, Ring, etc.)
    pub group: String,
    /// Labels/tags (UT, ST, etc.)
    pub labels: String,
    /// Texture references for sprite rendering
    pub textures: Vec<ObjectTexture>,
    /// Projectile data for weapons
    pub projectiles: Vec<Projectile>,
    /// Mask texture for dye/cloth rendering (applied with tex1/tex2 color)
    pub mask: Option<ObjectTexture>,
    /// Tex1 color value (clothing dye/cloth pattern, 0 = none)
    pub tex1: u32,
    /// Tex2 color value (accessory dye/cloth pattern, 0 = none)
    pub tex2: u32,
    /// SlotType from game data (determines equipment category: 1=Sword, 9=Ring, etc.)
    pub slot_type: i32,
    /// Base defense from game data (enemy DEF; reduces our self-computed hits, 0 = none/unknown)
    pub defense: i32,
    /// Enemy collision hitbox scale (`CustomHitbox scale` in the object XML;
    /// Deca's per-enemy hitbox multiplier). Defaults to 1.0 when absent. Used to
    /// size the target circle when testing whether Lethal Strike side procs land.
    pub hitbox_scale: f32,
    /// Equipment tier (0-14 for tiered items, None for UT/ST)
    pub tier: Option<i32>,
    /// Feed power value (from XML)
    pub feed_power: i32,
    /// Fame bonus percentage (XPBonus from XML)
    pub fame_bonus: i32,
    /// Bag type for loot drops (1-9)
    pub bag_type: i32,
    /// ST set name (from equip.xml setName attribute)
    pub set_name: Option<String>,
    /// Lethal Strike proc params (rogue cloaks only; from equip.xml)
    pub lethal_strike: Option<LethalStrikeParams>,
    /// Forge/tooltip category icon frame index into the CollectionIcon sheet
    /// (from equip.xml `collectionIcon` attribute). Groups items into in-game
    /// forge categories; `None` if the item has no category icon.
    pub collection_icon: Option<i32>,
    /// Primary-attack fire-rate multiplier (`<RateOfFire>` in equip.xml).
    /// Defaults to 1.0 when absent. 1.0 = the weapon's base rate.
    pub rate_of_fire: f32,
    /// Projectiles fired per shot (`<NumProjectiles>` in equip.xml). Defaults
    /// to 1 when absent.
    pub num_projectiles: i32,
    /// Flat stat bonuses granted while equipped (`<ActivateOnEquip stat=..
    /// amount=..>IncrementStat` in equip.xml).
    pub stat_bonuses: StatBonuses,
    /// Native procs from `OnPlayerShootActivate`/`OnEnemyHitActivate` that add
    /// damage beyond the weapon's base projectiles (e.g. Doku No Ken's poison
    /// burst). Empty for items without damaging procs.
    pub weapon_procs: Vec<WeaponProc>,
}

impl ObjectAsset {
    /// Get the best display name for this object.
    /// Falls back to a humanized `id_name` if `display_name` is empty or is an
    /// unresolved localization key (e.g., `{textiles.Small_Gemsbok_Cloth}`).
    ///
    /// Some item IDs share the same in-game `display_name` as a different,
    /// distinguishable item (e.g. a rare "troll" variant, or a legacy
    /// non-obtainable reskin). `name_override` disambiguates those so they
    /// display correctly everywhere (Trophy Hall, vault, characters, etc.).
    pub fn name(&self) -> &str {
        if let Some(overridden) = name_override(self.id) {
            return overridden;
        }
        if self.display_name.is_empty()
            || (self.display_name.starts_with('{') && self.display_name.ends_with('}'))
        {
            &self.id_name
        } else {
            &self.display_name
        }
    }

    /// Get the primary texture for this object.
    pub fn primary_texture(&self) -> Option<&ObjectTexture> {
        self.textures.first()
    }

    /// Check if this is an equipment item.
    pub fn is_equipment(&self) -> bool {
        self.class == "Equipment"
    }

    /// Check if this is a consumable.
    pub fn is_consumable(&self) -> bool {
        self.class == "Consumable" || self.class == "Potion"
    }

    /// Check if labels contain an exact tag (comma-separated).
    fn has_label(&self, tag: &str) -> bool {
        self.labels.split(',').any(|l| l == tag)
    }

    /// Check if this is a shiny item (has SHINY tag in labels).
    pub fn is_shiny(&self) -> bool {
        self.has_label("SHINY")
    }

    /// Check if this is a Legendary rarity item (has LEGENDARY tag in labels).
    pub fn is_legendary(&self) -> bool {
        self.has_label("LEGENDARY")
    }

    /// Check if this is a Divine rarity item (has DIVINE tag in labels).
    pub fn is_divine(&self) -> bool {
        self.has_label("DIVINE")
    }

    /// Check if this is a Legendary+ rarity item (Legendary or Divine).
    pub fn is_legendary_plus(&self) -> bool {
        self.is_legendary() || self.is_divine()
    }

    /// Check if this is a UT (Untiered) equipment item.
    /// Must have UT tag but NOT be a consumable (ichors, effusions, etc. don't count).
    pub fn is_ut(&self) -> bool {
        self.has_label("UT") && !self.has_label("CONSUMABLE")
    }

    /// Check if this is an ST (Set Tier) equipment item.
    pub fn is_st(&self) -> bool {
        self.has_label("ST")
    }

    /// Check if this is a forge Blueprint. Blueprints unlock/craft forge items
    /// and carry the same `collectionIcon` category as the dungeon they belong
    /// to, so they participate in the forge-category filter alongside UT items.
    /// Detected by the game's `Blueprint_N` id naming.
    pub fn is_blueprint(&self) -> bool {
        self.id_name.to_lowercase().contains("blueprint")
    }

    /// Check if this is a dungeon-specific upgrade material (e.g. Vial of Soul
    /// Extract, Kogbold Enhancement Core) - not UT/ST but still a collectible drop.
    pub fn is_upgrade_material(&self) -> bool {
        self.has_label("UPGRADECORE")
    }

    /// Check if this is a tiered equipment item (T0-T14).
    pub fn is_tiered(&self) -> bool {
        self.has_label("TIERED")
    }

    /// Get the equipment tier (0-14) if this is a tiered item.
    /// Returns None for UT/ST items.
    pub fn get_tier(&self) -> Option<i32> {
        if self.tier.is_some() {
            return self.tier;
        }
        // Fallback: parse from labels (T0, T1, ..., T14)
        if !self.is_tiered() {
            return None;
        }
        for label in self.labels.split(',') {
            if label.starts_with('T') && label.len() <= 3 {
                if let Ok(t) = label[1..].parse::<i32>() {
                    if (0..=14).contains(&t) {
                        return Some(t);
                    }
                }
            }
        }
        None
    }

    /// Check if this item has mask/dye rendering data.
    pub fn has_dye_data(&self) -> bool {
        self.mask.is_some() && (self.tex1 != 0 || self.tex2 != 0)
    }

    /// Decode the primary Tex1/Tex2 value into a [`TexInfo`].
    /// Prefers `tex1` when non-zero, otherwise `tex2`.
    pub fn tex_info(&self) -> TexInfo {
        let tex = if self.tex1 != 0 { self.tex1 } else { self.tex2 };
        decode_tex(tex)
    }
}

/// Asset data for a ground tile.
#[derive(Debug, Clone)]
pub struct TileAsset {
    /// Tile type ID
    pub id: i32,
    /// Internal ID name
    pub id_name: String,
    /// Damage when walking on tile (0 for safe tiles)
    pub damage: i32,
    /// Texture references
    pub textures: Vec<ObjectTexture>,
}

/// Collection of loaded object assets with efficient lookup.
#[derive(Debug, Default)]
pub struct ObjectList {
    /// Map from object ID to asset data
    objects: HashMap<i32, ObjectAsset>,
    /// Map from id_name to object ID for name lookups
    name_to_id: HashMap<String, i32>,
    /// Map from lowercased display_name to object ID. Prefers `Character`-class
    /// objects (enemies) so killer-name lookups resolve to the monster sprite.
    display_to_id: HashMap<String, i32>,
    /// Map from a forge Blueprint's object ID to the internal `id` of the item
    /// it unlocks, parsed from `<Activate id="X">UnlockForgeBlueprint</Activate>`
    /// in equip.xml. Authoritative (the blueprint's display name doesn't always
    /// match the unlocked item's name).
    blueprint_unlock: HashMap<i32, String>,
    /// Map from ability-item object ID to its locally-computable damaging
    /// effects (DetonateHex / PoisonGrenade), parsed from equip.xml. Only items
    /// that apply hex or deal ability damage are present.
    ability_effects: HashMap<i32, AbilityEffects>,
    /// Map from item object ID to its `IncrementStatRelative` equip effects,
    /// each `(stat, percent, stat_relative_to)`: adds `percent`% of the wearer's
    /// `stat_relative_to` value to `stat`. Only items with such effects appear.
    stat_relatives: HashMap<i32, Vec<(StatKind, f32, StatKind)>>,
}

impl ObjectList {
    /// Create a new empty object list.
    pub fn new() -> Self {
        Self {
            objects: HashMap::new(),
            name_to_id: HashMap::new(),
            display_to_id: HashMap::new(),
            blueprint_unlock: HashMap::new(),
            ability_effects: HashMap::new(),
            stat_relatives: HashMap::new(),
        }
    }

    /// Build the lowercased display_name -> id index.
    ///
    /// A display name is often shared by many objects (the main enemy, spawned
    /// helpers/clones, NPCs, invisible markers, ...). We score each candidate
    /// so the name resolves to the canonical, renderable enemy sprite (matching
    /// what the loot history shows for that mob), preferring `Character`-class
    /// enemies with a visible texture over spawned/invisible variants.
    fn build_display_index(objects: &HashMap<i32, ObjectAsset>) -> HashMap<String, i32> {
        // value = (score, id); higher score wins, lower id breaks ties.
        let mut best: HashMap<String, (i32, i32)> = HashMap::new();
        for asset in objects.values() {
            if asset.id < 0 {
                continue;
            }
            // Index by the real display name when present; otherwise fall back to
            // the internal id_name (via `name()`), so killer-name lookups still
            // resolve bosses the client shipped without a display name -- e.g. the
            // encounter bosses "Towering Perfection" / "Cube Deity" whose sprite
            // and ENCOUNTER label live on an empty-display object, while the
            // display name (if any) belongs to an invisible marker. Fallback keys
            // carry a small penalty so a genuine display name still wins a shared
            // key, but a renderable boss still beats an invisible same-name marker.
            let has_display = !asset.display_name.is_empty()
                && !(asset.display_name.starts_with('{') && asset.display_name.ends_with('}'));
            let key_src = if has_display {
                asset.display_name.as_str()
            } else {
                asset.name()
            };
            if key_src.is_empty() {
                continue;
            }
            let key = key_src.to_lowercase();
            let score = Self::display_candidate_score(asset) - if has_display { 0 } else { 10 };
            match best.get(&key) {
                None => {
                    best.insert(key, (score, asset.id));
                }
                Some(&(best_score, best_id)) => {
                    if score > best_score || (score == best_score && asset.id < best_id) {
                        best.insert(key, (score, asset.id));
                    }
                }
            }
        }
        best.into_iter().map(|(k, (_, id))| (k, id)).collect()
    }

    /// Score how well an object represents the "canonical" sprite for its
    /// display name. Higher is better.
    fn display_candidate_score(asset: &ObjectAsset) -> i32 {
        let mut score = 0;
        if asset.class == "Character" {
            score += 100;
        }
        let labels = asset.labels.to_uppercase();
        if labels.contains("ENEMY") {
            score += 20;
        }
        // The main boss is labelled BOSS; spawned helpers/clones are BOSS_SPAWNED.
        if labels.contains("BOSS_SPAWNED") {
            score -= 15;
        } else if labels.contains("BOSS") {
            score += 10;
        }
        // Prefer objects whose first texture is actually drawable.
        let renderable = asset
            .textures
            .first()
            .map(|t| t.name != "invisible" && !t.name.is_empty())
            .unwrap_or(false);
        if renderable {
            score += 50;
        } else {
            score -= 50;
        }
        score
    }

    /// Load object list from a file path.
    ///
    /// File format: `id;display;class;group;projectileData;textureData;labels;idName`
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, std::io::Error> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut objects = HashMap::new();
        let mut name_to_id = HashMap::new();

        for line in reader.lines() {
            let line = line?;
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            if let Some(asset) = Self::parse_object_line(&line) {
                name_to_id.insert(asset.id_name.clone(), asset.id);
                objects.insert(asset.id, asset);
            }
        }

        // Add placeholder for unloaded/unknown items
        objects.insert(
            -1,
            ObjectAsset {
                id: -1,
                id_name: "Unknown".to_string(),
                display_name: "Unknown Item".to_string(),
                class: String::new(),
                group: "Unknown".to_string(),
                labels: String::new(),
                textures: Vec::new(),
                projectiles: Vec::new(),
                mask: None,
                tex1: 0,
                tex2: 0,
                slot_type: 0,
                defense: 0,
                hitbox_scale: 1.0,
                tier: None,
                feed_power: 0,
                fame_bonus: 0,
                bag_type: 0,
                set_name: None,
                lethal_strike: None,
                collection_icon: None,
                rate_of_fire: 1.0,
                num_projectiles: 1,
                stat_bonuses: StatBonuses::default(),
                weapon_procs: Vec::new(),
            },
        );

        let display_to_id = Self::build_display_index(&objects);

        Ok(Self {
            objects,
            name_to_id,
            display_to_id,
            blueprint_unlock: HashMap::new(),
            ability_effects: HashMap::new(),
            stat_relatives: HashMap::new(),
        })
    }

    /// Parse a single line from ObjectID.list
    /// Format: `id;display;class;group;projectileData;textureData;labels;idName`
    fn parse_object_line(line: &str) -> Option<ObjectAsset> {
        let parts: Vec<&str> = line.split(';').collect();
        if parts.len() < 8 {
            return None;
        }

        let id = parts[0].parse::<i32>().ok()?;
        let display_name = parts[1].to_string();
        let class = parts[2].to_string();
        let group = parts[3].to_string();
        let projectile_data = parts[4];
        let texture_data = parts[5];
        let labels = parts[6].to_string();
        let id_name = parts[7].to_string();

        // Parse projectiles: slotType,min,max,ap,min,max,ap,...
        let projectiles = Self::parse_projectiles(projectile_data);

        // Parse textures: index,name,index,name,...
        let textures = Self::parse_textures(texture_data);

        // Parse mask texture (field 8, optional)
        let mask = if parts.len() > 8 && !parts[8].is_empty() {
            Self::parse_single_texture(parts[8])
        } else {
            None
        };

        // Parse tex1 color (field 9, optional)
        let tex1 = if parts.len() > 9 && !parts[9].is_empty() {
            parse_hex_u32(parts[9])
        } else {
            0
        };

        // Parse tex2 color (field 10, optional)
        let tex2 = if parts.len() > 10 && !parts[10].is_empty() {
            parse_hex_u32(parts[10])
        } else {
            0
        };

        // Parse slot_type (field 11, optional - added for category routing)
        let slot_type = if parts.len() > 11 && !parts[11].is_empty() {
            parts[11].parse::<i32>().unwrap_or(0)
        } else {
            0
        };

        // Parse defense (field 12, optional - enemy base DEF for self-damage calc)
        let defense = if parts.len() > 12 && !parts[12].is_empty() {
            parts[12].parse::<i32>().unwrap_or(0)
        } else {
            0
        };

        // Parse hitbox scale (field 13, optional - CustomHitbox scale for LS proc geometry)
        let hitbox_scale = if parts.len() > 13 && !parts[13].is_empty() {
            parts[13].parse::<f32>().unwrap_or(1.0)
        } else {
            1.0
        };

        Some(ObjectAsset {
            id,
            id_name,
            display_name,
            class,
            group,
            labels,
            textures,
            projectiles,
            mask,
            tex1,
            tex2,
            slot_type,
            defense,
            hitbox_scale,
            tier: None,
            feed_power: 0,
            fame_bonus: 0,
            bag_type: 0,
            set_name: None,
            lethal_strike: None,
            collection_icon: None,
            rate_of_fire: 1.0,
            num_projectiles: 1,
            stat_bonuses: StatBonuses::default(),
            weapon_procs: Vec::new(),
        })
    }

    /// Parse projectile data string.
    /// Supports two formats:
    /// - Original: `slotType,min,max,ap,...`
    /// - New: `min:max:ap,min:max:ap,...`
    fn parse_projectiles(data: &str) -> Vec<Projectile> {
        if data.is_empty() {
            return Vec::new();
        }

        // Check if using new colon-separated format
        if data.contains(':') {
            // New format: min:max:ap,min:max:ap,...
            return data
                .split(',')
                .filter_map(|proj| {
                    let parts: Vec<&str> = proj.split(':').collect();
                    if parts.len() >= 3 {
                        Some(Projectile {
                            min_damage: parts[0].parse().unwrap_or(0),
                            max_damage: parts[1].parse().unwrap_or(0),
                            armor_piercing: parts[2] == "1",
                            slot_type: 0,
                            display_id: None,
                            ignore_on_tooltip: false,
                        })
                    } else {
                        None
                    }
                })
                .collect();
        }

        // Original format: slotType,min,max,ap,...
        let parts: Vec<&str> = data.split(',').collect();
        if parts.is_empty() || parts[0].is_empty() {
            return Vec::new();
        }

        let slot_type = parts[0].parse::<i32>().unwrap_or(0);
        let mut projectiles = Vec::new();

        // Parse groups of 3: min, max, armor_piercing
        let mut i = 1;
        while i + 2 < parts.len() {
            let min = parts[i].parse::<i32>().unwrap_or(0);
            let max = parts[i + 1].parse::<i32>().unwrap_or(0);
            let ap = parts[i + 2] == "1";

            projectiles.push(Projectile {
                min_damage: min,
                max_damage: max,
                armor_piercing: ap,
                slot_type,
                display_id: None,
                ignore_on_tooltip: false,
            });

            i += 3;
        }

        projectiles
    }

    /// Parse texture data string.
    /// Supports two formats:
    /// - Original: `index,name,index,name,...`
    /// - New: `name:index,name:index,...`
    fn parse_textures(data: &str) -> Vec<ObjectTexture> {
        if data.is_empty() {
            return Vec::new();
        }

        // Check if using new colon-separated format (name:index)
        if data.contains(':') {
            return data
                .split(',')
                .filter_map(|tex| {
                    let parts: Vec<&str> = tex.split(':').collect();
                    if parts.len() >= 2 {
                        // Parse hex index like 0x70
                        let index = if parts[1].starts_with("0x") || parts[1].starts_with("0X") {
                            i32::from_str_radix(&parts[1][2..], 16).unwrap_or(0)
                        } else {
                            parts[1].parse().unwrap_or(0)
                        };
                        Some(ObjectTexture {
                            name: parts[0].to_string(),
                            index,
                        })
                    } else {
                        None
                    }
                })
                .collect();
        }

        // Original format: index,name,index,name,...
        let parts: Vec<&str> = data.split(',').collect();
        let mut textures = Vec::new();

        // Parse pairs: index,name,index,name,...
        let mut i = 0;
        while i + 1 < parts.len() {
            let index = parts[i].parse::<i32>().unwrap_or(0);
            let name = parts[i + 1].to_string();

            if !name.is_empty() {
                textures.push(ObjectTexture { name, index });
            }

            i += 2;
        }

        textures
    }

    /// Parse a single `name:index` texture reference.
    fn parse_single_texture(data: &str) -> Option<ObjectTexture> {
        let parts: Vec<&str> = data.split(':').collect();
        if parts.len() >= 2 {
            let index = if parts[1].starts_with("0x") || parts[1].starts_with("0X") {
                i32::from_str_radix(&parts[1][2..], 16).unwrap_or(0)
            } else {
                parts[1].parse().unwrap_or(0)
            };
            Some(ObjectTexture {
                name: parts[0].to_string(),
                index,
            })
        } else {
            None
        }
    }

    /// Look up an object by ID.
    pub fn get(&self, id: i32) -> Option<&ObjectAsset> {
        self.objects.get(&id)
    }

    /// Look up an object by its id_name (internal name like "Sigma_Werewolf").
    pub fn get_by_name(&self, id_name: &str) -> Option<&ObjectAsset> {
        self.name_to_id
            .get(id_name)
            .and_then(|&id| self.objects.get(&id))
    }

    /// Get the object ID for an id_name, or None if not found.
    pub fn id_for_name(&self, id_name: &str) -> Option<i32> {
        self.name_to_id.get(id_name).copied()
    }

    /// Resolve the object ID of the item a forge Blueprint unlocks, from the
    /// authoritative `UnlockForgeBlueprint` activate target parsed from
    /// equip.xml. Returns `None` when `id` isn't a mapped blueprint or the
    /// target internal id doesn't resolve to a known object.
    pub fn blueprint_unlocked_id(&self, id: i32) -> Option<i32> {
        let target = self.blueprint_unlock.get(&id)?;
        self.id_for_name(target)
    }

    /// Get an object ID for a (case-insensitive) display name, or None.
    ///
    /// Prefers `Character`-class objects (enemies), so this resolves a killer
    /// name to the monster sprite where one exists.
    pub fn id_for_display_name(&self, display_name: &str) -> Option<i32> {
        self.display_to_id
            .get(&display_name.to_lowercase())
            .copied()
    }

    /// Locally-computable damaging ability effects for an ability item, if any
    /// were parsed from equip.xml.
    pub fn ability_effects(&self, id: i32) -> Option<&AbilityEffects> {
        self.ability_effects.get(&id)
    }

    /// Primary-attack fire attributes `(rate_of_fire, num_projectiles)` for a
    /// weapon, from equip.xml. Returns `None` if the item is unknown.
    pub fn weapon_fire(&self, id: i32) -> Option<(f32, i32)> {
        self.objects
            .get(&id)
            .map(|o| (o.rate_of_fire, o.num_projectiles))
    }

    /// Native damaging procs (OnPlayerShootActivate/OnEnemyHitActivate) for an
    /// item. Empty slice when the item is unknown or has no such procs.
    pub fn weapon_procs(&self, id: i32) -> &[WeaponProc] {
        self.objects
            .get(&id)
            .map_or(&[], |o| o.weapon_procs.as_slice())
    }

    /// Flat stat bonuses granted while the item is equipped (equip.xml
    /// `ActivateOnEquip` IncrementStat). Returns `None` if the item is unknown.
    pub fn item_stat_bonuses(&self, id: i32) -> Option<StatBonuses> {
        self.objects.get(&id).map(|o| o.stat_bonuses)
    }

    /// Base XP-gain bonus percentage granted while the item is equipped
    /// (equip.xml `<XPBonus>`; shown on tooltips as "XP Bonus: N%"). Returns 0
    /// when the item is unknown or has no XP bonus.
    pub fn item_fame_bonus(&self, id: i32) -> i32 {
        self.objects.get(&id).map(|o| o.fame_bonus).unwrap_or(0)
    }

    /// `IncrementStatRelative` equip effects for an item, each
    /// `(stat, percent, stat_relative_to)`. Empty slice if the item has none or
    /// is unknown.
    pub fn item_stat_relatives(&self, id: i32) -> &[(StatKind, f32, StatKind)] {
        self.stat_relatives
            .get(&id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Every object ID whose display name matches `display_name`
    /// (case-insensitive). Used to group biome / re-skinned variants of the
    /// same encounter (e.g. the old and "New" Skull Shrine both display as
    /// "Skull Shrine") so they trigger the same notification. Skips the -1
    /// placeholder.
    pub fn ids_for_display_name(&self, display_name: &str) -> Vec<i32> {
        let key = display_name.to_lowercase();
        self.objects
            .values()
            .filter(|a| a.id >= 0 && a.name().to_lowercase() == key)
            .map(|a| a.id)
            .collect()
    }

    /// Get the display name for an object ID, or None if not found.
    pub fn name(&self, id: i32) -> Option<&str> {
        self.objects.get(&id).map(|a| a.name())
    }

    /// Get the number of loaded objects.
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    /// Check if the list is empty.
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// Check if an item ID is a shiny item.
    pub fn is_shiny(&self, id: i32) -> bool {
        self.objects.get(&id).map(|a| a.is_shiny()).unwrap_or(false)
    }

    /// Check if an item ID is a Legendary rarity item.
    pub fn is_legendary(&self, id: i32) -> bool {
        self.objects
            .get(&id)
            .map(|a| a.is_legendary())
            .unwrap_or(false)
    }

    /// Check if an item ID is a Divine rarity item.
    pub fn is_divine(&self, id: i32) -> bool {
        self.objects
            .get(&id)
            .map(|a| a.is_divine())
            .unwrap_or(false)
    }

    /// Check if an item ID is a Legendary+ rarity item (Legendary or Divine).
    pub fn is_legendary_plus(&self, id: i32) -> bool {
        self.objects
            .get(&id)
            .map(|a| a.is_legendary_plus())
            .unwrap_or(false)
    }

    /// Check if an item ID is a UT (Untiered) item.
    pub fn is_ut(&self, id: i32) -> bool {
        self.objects.get(&id).map(|a| a.is_ut()).unwrap_or(false)
    }

    /// Check if an object ID exists.
    pub fn contains(&self, id: i32) -> bool {
        self.objects.contains_key(&id)
    }

    /// Whether the object ID is a Portal-class object.
    pub fn is_portal(&self, id: i32) -> bool {
        self.objects
            .get(&id)
            .map(|a| a.class == "Portal")
            .unwrap_or(false)
    }

    /// Iterate over all objects.
    pub fn iter(&self) -> impl Iterator<Item = &ObjectAsset> {
        self.objects.values()
    }

    /// Iterate over all objects with their IDs.
    pub fn iter_with_ids(&self) -> impl Iterator<Item = (&i32, &ObjectAsset)> {
        self.objects.iter()
    }

    /// Insert an asset directly. Test-only helper for building small object
    /// lists without parsing XML.
    #[cfg(test)]
    pub fn insert_asset(&mut self, id: i32, asset: ObjectAsset) {
        self.objects.insert(id, asset);
    }

    /// Find all Portal objects.
    pub fn portals(&self) -> impl Iterator<Item = &ObjectAsset> {
        self.objects.values().filter(|a| a.class == "Portal")
    }

    /// Find a portal object ID by dungeon name.
    /// Searches for portals whose display_name matches the dungeon name.
    /// Uses a priority system to avoid false positives from substring matches:
    /// 1. Exact match (portal name == dungeon name, ignoring " Portal" suffix)
    /// 2. Starts-with match, preferring the shortest portal name (most specific)
    /// 3. Contains match, preferring the shortest portal name (most specific)
    ///
    /// For example, searching for "Abyss of Demons" must not match
    /// "Infernal Abyss of Demons Portal" or "Legacy Heroic Abyss of Demons Portal".
    pub fn find_portal_for_dungeon(&self, dungeon_name: &str) -> Option<i32> {
        let dungeon_lower = dungeon_name.to_lowercase();
        let dungeon_trimmed = dungeon_lower
            .trim_end_matches(" portal")
            .trim_end_matches(" entrance");

        // Pass 1: Exact match. Prefer portal whose id_name contains the dungeon name
        // (disambiguates duplicates like "The Tavern Portal" vs "Beer God Encounter Portal").
        let mut exact_match: Option<(i32, bool)> = None;
        for portal in self.portals() {
            let name_lower = portal.display_name.to_lowercase();
            let name_trimmed = name_lower
                .trim_end_matches(" portal")
                .trim_end_matches(" entrance");

            if name_trimmed == dungeon_trimmed {
                let id_name_lower = portal.id_name.to_lowercase();
                let id_name_matches = id_name_lower.contains(dungeon_trimmed);
                match exact_match {
                    None => exact_match = Some((portal.id, id_name_matches)),
                    Some((_, false)) if id_name_matches => {
                        exact_match = Some((portal.id, true));
                    }
                    Some((prev_id, prev_matches))
                        if id_name_matches == prev_matches && portal.id < prev_id =>
                    {
                        exact_match = Some((portal.id, id_name_matches));
                    }
                    _ => {}
                }
            }
        }
        if let Some((id, _)) = exact_match {
            return Some(id);
        }

        // Pass 2: starts_with — pick the shortest (most specific) match
        // "Abyss of Demons Portal" is shorter than "Infernal Abyss of Demons Portal"
        let mut best: Option<(i32, usize)> = None;
        for portal in self.portals() {
            let name_lower = portal.display_name.to_lowercase();
            if name_lower.starts_with(&dungeon_lower) {
                let len = name_lower.len();
                if best.map_or(true, |(_, prev_len)| len < prev_len) {
                    best = Some((portal.id, len));
                }
            }
        }
        if let Some((id, _)) = best {
            return Some(id);
        }

        // Pass 3: contains — pick the shortest (most specific) match
        let mut best: Option<(i32, usize)> = None;
        for portal in self.portals() {
            let name_lower = portal.display_name.to_lowercase();
            if name_lower.contains(&dungeon_lower) {
                let len = name_lower.len();
                if best.map_or(true, |(_, prev_len)| len < prev_len) {
                    best = Some((portal.id, len));
                }
            }
        }
        best.map(|(id, _)| id)
    }

    /// Merge equipment data from XML file into existing objects.
    ///
    /// Extracts tier, feedPower, XPBonus (fame_bonus), and BagType from equipment XML.
    /// Updates objects in-place by matching on object type ID.
    pub fn merge_equipment_xml<P: AsRef<Path>>(&mut self, path: P) -> Result<usize, String> {
        use quick_xml::events::Event;
        use quick_xml::Reader;

        let file = File::open(path.as_ref())
            .map_err(|e| format!("Failed to open equipment XML: {}", e))?;
        let reader = BufReader::new(file);
        let mut xml_reader = Reader::from_reader(reader);
        xml_reader.config_mut().trim_text(true);

        let mut buf = Vec::new();
        let mut merged_count = 0;

        // Current object being parsed
        let mut current_type_id: Option<i32> = None;
        let mut current_tier: Option<i32> = None;
        let mut current_feed_power: i32 = 0;
        let mut current_fame_bonus: i32 = 0;
        let mut current_bag_type: i32 = 0;
        let mut current_set_name: Option<String> = None;
        let mut current_lethal_strike: Option<LethalStrikeParams> = None;
        let mut current_collection_icon: Option<i32> = None;
        let mut current_rate_of_fire: f32 = 1.0;
        let mut current_num_projectiles: i32 = 1;
        let mut rof_seen = false;
        let mut np_seen = false;
        let mut current_stat_bonuses = StatBonuses::default();
        let mut current_stat_relatives: Vec<(StatKind, f32, StatKind)> = Vec::new();

        // Element tracking
        let mut in_tier = false;
        let mut in_feed_power = false;
        let mut in_xp_bonus = false;
        let mut in_bag_type = false;
        let mut in_rate_of_fire = false;
        let mut in_num_projectiles = false;
        // Inside an <ActivateOnEquip stat=".." amount=".."> whose text we still
        // need to confirm equals "IncrementStat" before crediting the bonus.
        let mut pending_equip_stat: Option<(StatKind, f32)> = None;
        let mut pending_equip_relative: Option<(StatKind, f32, StatKind)> = None;
        let mut in_activate_on_equip = false;
        // Inside an <OnConditionEndActivate ...> whose text we still need to
        // check equals "LethalStrike" before committing the parsed params.
        let mut in_cond_end_activate = false;
        let mut pending_lethal_strike: Option<LethalStrikeParams> = None;
        // Inside an <Activate id="X">...</Activate> whose text we check for
        // "UnlockForgeBlueprint"; `X` is the unlocked item's internal id.
        let mut in_activate = false;
        let mut pending_activate_id: Option<String> = None;
        let mut current_unlock_target: Option<String> = None;
        let mut unlock_map: HashMap<i32, String> = HashMap::new();

        // Damaging ability effects (DetonateHex / PoisonGrenade / Hex) for the
        // current object, and the attributes of the effect element currently
        // open (attrs live on the Start event; the effect name arrives as Text).
        let mut current_ability = AbilityEffects::default();
        let mut pending_attrs: HashMap<String, String> = HashMap::new();
        let mut in_on_detonate = false;
        let mut in_on_enemy_hit = false;
        let mut ability_map: HashMap<i32, AbilityEffects> = HashMap::new();
        // Ability item `<Projectile>` damage (min, max, armor_piercing) for the
        // current object, used to resolve Shoot/BulletNova per-shot damage.
        let mut pending_proj: Option<(f32, f32, bool)> = None;
        let mut in_projectile = false;
        let mut in_proj_min = false;
        let mut in_proj_max = false;
        let mut proj_ap = false;
        // Deferred Shoot/BulletNova/BulletCreate specs (min_hex, max_hex, attrs,
        // default_shots) resolved against `pending_proj` at Object end (projectile
        // may be parsed after the effect element).
        let mut pending_bursts: Vec<(u32, u32, HashMap<String, String>, i32)> = Vec::new();
        // Deferred BulletCreate entries whose `type` references an external
        // projectile object. Resolved after the full parse loop once all objects
        // (and their projectile data) are loaded.
        // (owner_type_id, external_type_id, attrs, num_shots)
        let mut deferred_bullet_creates: Vec<(i32, i32, HashMap<String, String>, i32)> = Vec::new();
        // Deferred SpawnCreep/RaiseDead entries whose objectId/undeadId
        // references an entity by name. Resolved after the full parse loop.
        // (owner_type_id, entity_name, scaling attrs, is_undead)
        let mut deferred_spawn_creeps: Vec<(i32, String, HashMap<String, String>, bool)> =
            Vec::new();
        // Count of OnPlayerShootActivate BulletCreate entries with
        // requiredConditions="LethalStrike" on the current object.
        let mut ls_extra_shots: i32 = 0;
        // Deferred ShurikenAbility attrs, resolved at Object end once
        // pending_proj is known. At most one per item.
        let mut pending_shuriken: Option<HashMap<String, String>> = None;
        // Deferred Dash trail damage from `<StartUse><Activate>Dash</Activate></StartUse>`.
        let mut pending_dash_damage: Option<i32> = None;
        let mut in_on_player_shoot = false;
        let mut on_player_shoot_is_ls = false;
        // Native damaging procs (OnPlayerShootActivate / OnEnemyHitActivate) for
        // the current object, committed to `obj.weapon_procs` at Object end.
        let mut current_weapon_procs: Vec<WeaponProc> = Vec::new();

        loop {
            match xml_reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    match e.name().as_ref() {
                        b"Object" => {
                            // Reset state for new object
                            current_type_id = None;
                            current_tier = None;
                            current_feed_power = 0;
                            current_fame_bonus = 0;
                            current_bag_type = 0;
                            current_set_name = None;
                            current_lethal_strike = None;
                            current_collection_icon = None;
                            current_unlock_target = None;
                            current_ability = AbilityEffects::default();
                            current_rate_of_fire = 1.0;
                            current_num_projectiles = 1;
                            rof_seen = false;
                            np_seen = false;
                            current_stat_bonuses = StatBonuses::default();
                            current_stat_relatives = Vec::new();
                            pending_proj = None;
                            in_projectile = false;
                            in_proj_min = false;
                            in_proj_max = false;
                            proj_ap = false;
                            pending_bursts.clear();
                            ls_extra_shots = 0;
                            pending_shuriken = None;
                            pending_dash_damage = None;
                            current_weapon_procs.clear();

                            // Parse type and setName attributes
                            for attr in e.attributes().flatten() {
                                match attr.key.as_ref() {
                                    b"type" => {
                                        let type_str = String::from_utf8_lossy(&attr.value);
                                        current_type_id = parse_hex_type(&type_str);
                                    }
                                    b"setName" => {
                                        current_set_name =
                                            Some(String::from_utf8_lossy(&attr.value).to_string());
                                    }
                                    b"collectionIcon" => {
                                        current_collection_icon =
                                            String::from_utf8_lossy(&attr.value).parse().ok();
                                    }
                                    _ => {}
                                }
                            }
                        }
                        b"Tier" => in_tier = true,
                        b"feedPower" => in_feed_power = true,
                        b"XPBonus" => in_xp_bonus = true,
                        b"BagType" => in_bag_type = true,
                        b"RateOfFire" => in_rate_of_fire = true,
                        b"NumProjectiles" => in_num_projectiles = true,
                        b"Projectile" => {
                            in_projectile = true;
                            proj_ap = false;
                        }
                        b"MinDamage" if in_projectile => in_proj_min = true,
                        b"MaxDamage" if in_projectile => in_proj_max = true,
                        b"ArmorPiercing" if in_projectile => proj_ap = true,
                        b"ConditionEffect" if in_projectile => {
                            pending_attrs = attrs_to_map(e);
                        }
                        b"ActivateOnEquip" => {
                            let mut stat: Option<StatKind> = None;
                            let mut amount: f32 = 0.0;
                            let mut rel: Option<StatKind> = None;
                            for attr in e.attributes().flatten() {
                                match attr.key.as_ref() {
                                    b"stat" => {
                                        stat =
                                            StatKind::parse(&String::from_utf8_lossy(&attr.value));
                                    }
                                    b"amount" => {
                                        amount = String::from_utf8_lossy(&attr.value)
                                            .parse()
                                            .unwrap_or(0.0);
                                    }
                                    b"statRelativeTo" => {
                                        rel =
                                            StatKind::parse(&String::from_utf8_lossy(&attr.value));
                                    }
                                    _ => {}
                                }
                            }
                            pending_equip_stat = stat.map(|s| (s, amount));
                            pending_equip_relative = match (stat, rel) {
                                (Some(s), Some(r)) => Some((s, amount, r)),
                                _ => None,
                            };
                            in_activate_on_equip = true;
                        }
                        b"OnConditionEndActivate" => {
                            // Parse the proc params now; only commit them if the
                            // element's text turns out to be "LethalStrike".
                            let mut p = LethalStrikeParams {
                                ignore_flat: 0.0,
                                ignore_perc: 0.0,
                                stat_mod_flat: 0.0,
                                stat_mod_perc: 0.0,
                                stat_mod_scaling_min: 0.0,
                                scaling_stat: ScalingStat::Wisdom,
                                duration: 0.0,
                                extra_shots: 0,
                            };
                            for attr in e.attributes().flatten() {
                                let v = String::from_utf8_lossy(&attr.value);
                                match attr.key.as_ref() {
                                    b"ignoreFlat" => p.ignore_flat = v.parse().unwrap_or(0.0),
                                    b"ignorePerc" => p.ignore_perc = v.parse().unwrap_or(0.0),
                                    b"statModFlat" => p.stat_mod_flat = v.parse().unwrap_or(0.0),
                                    b"statModPerc" => p.stat_mod_perc = v.parse().unwrap_or(0.0),
                                    b"statModScalingMin" => {
                                        p.stat_mod_scaling_min = v.parse().unwrap_or(0.0)
                                    }
                                    b"scalingStat" => {
                                        if let Some(s) = ScalingStat::parse(&v) {
                                            p.scaling_stat = s;
                                        }
                                    }
                                    b"duration" => p.duration = v.parse().unwrap_or(0.0),
                                    _ => {}
                                }
                            }
                            pending_lethal_strike = Some(p);
                            in_cond_end_activate = true;
                        }
                        b"Activate" => {
                            pending_activate_id = e
                                .attributes()
                                .flatten()
                                .find(|a| a.key.as_ref() == b"id")
                                .map(|a| String::from_utf8_lossy(&a.value).to_string());
                            pending_attrs = attrs_to_map(e);
                            in_activate = true;
                        }
                        b"OnDetonateHexActivate" => {
                            pending_attrs = attrs_to_map(e);
                            in_on_detonate = true;
                        }
                        b"OnEnemyHitActivate" => {
                            pending_attrs = attrs_to_map(e);
                            in_on_enemy_hit = true;
                        }
                        b"OnPlayerShootActivate" => {
                            pending_attrs = attrs_to_map(e);
                            on_player_shoot_is_ls = e.attributes().flatten().any(|a| {
                                a.key.as_ref() == b"requiredConditions"
                                    && a.value.as_ref() == b"LethalStrike"
                            });
                            in_on_player_shoot = true;
                        }
                        _ => {}
                    }
                }
                Ok(Event::Text(ref e)) => {
                    let text = e.unescape().unwrap_or_default();
                    if in_tier {
                        current_tier = text.parse().ok();
                    } else if in_feed_power {
                        current_feed_power = text.parse().unwrap_or(0);
                    } else if in_xp_bonus {
                        current_fame_bonus = text.parse().unwrap_or(0);
                    } else if in_bag_type {
                        current_bag_type = text.parse().unwrap_or(0);
                    } else if in_rate_of_fire {
                        if !rof_seen {
                            if let Ok(v) = text.parse::<f32>() {
                                current_rate_of_fire = v;
                                rof_seen = true;
                            }
                        }
                    } else if in_num_projectiles {
                        if !np_seen {
                            if let Ok(v) = text.parse::<i32>() {
                                current_num_projectiles = v;
                                np_seen = true;
                            }
                        }
                    } else if in_proj_min {
                        if let Ok(v) = text.trim().parse::<f32>() {
                            let (_, mx, ap) = pending_proj.unwrap_or((0.0, 0.0, false));
                            pending_proj = Some((v, mx, ap));
                        }
                    } else if in_proj_max {
                        if let Ok(v) = text.trim().parse::<f32>() {
                            let (mn, _, ap) = pending_proj.unwrap_or((0.0, 0.0, false));
                            pending_proj = Some((mn, v, ap));
                        }
                    } else if in_projectile && text.as_ref() == "Bleeding" {
                        // A weapon whose projectile applies `Bleeding` inflicts an
                        // armor-piercing DoT that stacks per landed shot. On a
                        // single target one projectile lands per shot (projectiles
                        // spread), so we model one bleeding source per weapon and
                        // fold the per-hit stacking at DPS time. When a weapon has
                        // several bleeding projectile types, keep the one with the
                        // greatest steady-state contribution (rate x duration).
                        let duration = af(&pending_attrs, "duration", 0.0);
                        // Per-stack HP/s drain: some items specify `bleedDamage`,
                        // others `amount`; RotMG defaults to 20/s when omitted.
                        let dmg_per_sec = if pending_attrs.contains_key("bleedDamage") {
                            af(&pending_attrs, "bleedDamage", 20.0)
                        } else {
                            af(&pending_attrs, "amount", 20.0)
                        };
                        if duration > 0.0 && dmg_per_sec > 0.0 {
                            let bleed = BleedingEffect {
                                dmg_per_sec,
                                duration_s: duration,
                            };
                            if let Some(existing) =
                                current_weapon_procs
                                    .iter_mut()
                                    .find_map(|p| match &mut p.effect {
                                        ProcEffect::Bleeding(b) => Some(b),
                                        _ => None,
                                    })
                            {
                                if bleed.dmg_per_sec * bleed.duration_s
                                    > existing.dmg_per_sec * existing.duration_s
                                {
                                    *existing = bleed;
                                }
                            } else {
                                current_weapon_procs.push(WeaponProc {
                                    trigger: ProcTrigger::OnHit,
                                    proc_rate: 1.0,
                                    cooldown: 0.0,
                                    required_conditions: None,
                                    effect: ProcEffect::Bleeding(bleed),
                                });
                            }
                        }
                    } else if in_activate_on_equip {
                        if text.as_ref() == "IncrementStat" {
                            if let Some((stat, amount)) = pending_equip_stat {
                                current_stat_bonuses.add(stat, amount);
                            }
                        } else if text.as_ref() == "IncrementStatRelative" {
                            if let Some(rel) = pending_equip_relative {
                                current_stat_relatives.push(rel);
                            }
                        }
                    } else if in_cond_end_activate && text.as_ref() == "LethalStrike" {
                        current_lethal_strike = pending_lethal_strike;
                    } else if in_activate && text.as_ref() == "UnlockForgeBlueprint" {
                        current_unlock_target = pending_activate_id.take();
                    } else if in_activate && text.as_ref() == "DetonateHex" {
                        current_ability.detonate_hex = Some(parse_detonate_hex(&pending_attrs));
                    } else if in_activate && text.as_ref() == "PoisonGrenade" {
                        current_ability
                            .poison_grenades
                            .push(parse_poison_grenade(&pending_attrs));
                    } else if in_on_detonate && text.as_ref() == "PoisonGrenade" {
                        current_ability.on_detonate_poison.push(HexThresholdPoison {
                            min_hex: au(&pending_attrs, "minHex", 0),
                            max_hex: au(&pending_attrs, "maxHex", 100),
                            grenade: parse_poison_grenade(&pending_attrs),
                        });
                    } else if (in_on_player_shoot || in_on_enemy_hit)
                        && text.as_ref() == "PoisonGrenade"
                    {
                        let trigger = if in_on_player_shoot {
                            ProcTrigger::OnShoot
                        } else {
                            ProcTrigger::OnHit
                        };
                        // `mustWear` gates the proc on wearing a specific item
                        // (set synergy), so treat it like a required condition
                        // and exclude it from steady-state weapon DPS.
                        let required_conditions = pending_attrs
                            .get("requiredConditions")
                            .or_else(|| pending_attrs.get("mustWear"))
                            .cloned();
                        current_weapon_procs.push(WeaponProc {
                            trigger,
                            proc_rate: af(&pending_attrs, "proc", 1.0).clamp(0.0, 1.0),
                            cooldown: af(&pending_attrs, "cooldown", 0.0).max(0.0),
                            required_conditions,
                            effect: ProcEffect::Poison(parse_poison_grenade(&pending_attrs)),
                        });
                    } else if in_on_detonate && text.as_ref() == "BulletNova" {
                        pending_bursts.push((
                            au(&pending_attrs, "minHex", 0),
                            au(&pending_attrs, "maxHex", 100),
                            pending_attrs.clone(),
                            au(&pending_attrs, "numShots", 1) as i32,
                        ));
                    } else if in_on_detonate && text.as_ref() == "ApplyCondition" {
                        let condition = pending_attrs.get("effect").cloned().unwrap_or_default();
                        if !condition.is_empty() {
                            current_ability.condition_hex_tiers.push(ConditionHexTier {
                                condition,
                                min_hex: au(&pending_attrs, "minHex", 0),
                                duration: af(&pending_attrs, "duration", 0.0),
                            });
                        }
                    } else if in_activate
                        && (text.as_ref() == "Shoot" || text.as_ref() == "BulletNova")
                    {
                        pending_bursts.push((
                            0,
                            100,
                            pending_attrs.clone(),
                            current_num_projectiles.max(1),
                        ));
                    } else if in_activate && text.as_ref() == "BulletCreate" {
                        let dominated = pending_attrs
                            .get("ignoreOnTooltip")
                            .map(|v| v == "true" || v == "1")
                            .unwrap_or(false);
                        if !dominated {
                            let shots = pending_attrs
                                .get("numShots")
                                .and_then(|v| v.parse::<i32>().ok())
                                .unwrap_or(1);
                            match pending_attrs.get("type") {
                                Some(t) => {
                                    if let (Some(ext), Some(owner)) =
                                        (parse_hex_type(t), current_type_id)
                                    {
                                        deferred_bullet_creates.push((
                                            owner,
                                            ext,
                                            pending_attrs.clone(),
                                            shots,
                                        ));
                                    }
                                }
                                None => {
                                    pending_bursts.push((0, 100, pending_attrs.clone(), shots));
                                }
                            }
                        }
                    } else if in_activate && text.as_ref() == "Heal" {
                        current_ability
                            .effects
                            .push(AbilityEffect::Restore(parse_restore(&pending_attrs, false)));
                    } else if in_activate && text.as_ref() == "Magic" {
                        current_ability
                            .effects
                            .push(AbilityEffect::Restore(parse_restore(&pending_attrs, true)));
                    } else if in_activate && text.as_ref() == "Lightning" {
                        current_ability
                            .effects
                            .push(AbilityEffect::Lightning(parse_lightning(&pending_attrs)));
                    } else if in_activate && text.as_ref() == "Trap" {
                        current_ability
                            .effects
                            .push(AbilityEffect::Trap(parse_trap(&pending_attrs)));
                    } else if in_activate && text.as_ref() == "VampireBlast" {
                        current_ability.effects.push(AbilityEffect::VampireBlast(
                            parse_vampire_blast(&pending_attrs),
                        ));
                    } else if in_activate && text.as_ref() == "DamageNova" {
                        current_ability
                            .effects
                            .push(AbilityEffect::DamageNova(parse_damage_nova(&pending_attrs)));
                    } else if in_activate && text.as_ref() == "EffectBlast" {
                        let dominated = pending_attrs
                            .get("ignoreOnTooltip")
                            .map(|v| v == "true" || v == "1")
                            .unwrap_or(false);
                        if !dominated {
                            let condition = pending_attrs
                                .get("condEffect")
                                .or_else(|| pending_attrs.get("effect"))
                                .cloned()
                                .unwrap_or_default();
                            current_ability.effects.push(AbilityEffect::EffectBlast(
                                EffectBlastEffect {
                                    condition,
                                    base_duration: af(&pending_attrs, "condDuration", 0.0),
                                    wis_per_duration: af(&pending_attrs, "wisPerDuration", 0.0),
                                    wis_min: af(&pending_attrs, "wisMin", 0.0),
                                    radius: af(&pending_attrs, "radius", 0.0),
                                },
                            ));
                        }
                    } else if in_activate && text.as_ref() == "SpawnCreep" {
                        let dominated = pending_attrs
                            .get("ignoreOnTooltip")
                            .map(|v| v == "true" || v == "1")
                            .unwrap_or(false);
                        if !dominated {
                            if let (Some(obj_id), Some(owner)) =
                                (pending_attrs.get("objectId").cloned(), current_type_id)
                            {
                                deferred_spawn_creeps.push((
                                    owner,
                                    obj_id,
                                    pending_attrs.clone(),
                                    false,
                                ));
                            }
                        }
                    } else if in_activate && text.as_ref() == "RaiseDead" {
                        let dominated = pending_attrs
                            .get("ignoreOnTooltip")
                            .map(|v| v == "true" || v == "1")
                            .unwrap_or(false);
                        if !dominated {
                            if let (Some(undead_id), Some(owner)) =
                                (pending_attrs.get("undeadId").cloned(), current_type_id)
                            {
                                deferred_spawn_creeps.push((
                                    owner,
                                    undead_id,
                                    pending_attrs.clone(),
                                    true,
                                ));
                            }
                        }
                    } else if in_activate && text.as_ref() == "ShurikenAbility" {
                        // Mechanic-only stars (e.g. Bubbly) use ShurikenAbility
                        // for hold-state conditions, not projectile damage.
                        let is_mechanic_only = pending_attrs.contains_key("effect");
                        if !is_mechanic_only {
                            pending_shuriken = Some(pending_attrs.clone());
                        }
                    } else if in_activate && text.as_ref() == "StatBoostSelf" {
                        current_ability
                            .effects
                            .push(AbilityEffect::StatBoost(StatBoostEffect {
                                stat: pending_attrs.get("stat").cloned().unwrap_or_default(),
                                amount: pending_attrs
                                    .get("amount")
                                    .and_then(|v| v.parse().ok())
                                    .unwrap_or(0),
                                duration: af(&pending_attrs, "duration", 0.0),
                            }));
                    } else if in_activate
                        && (text.as_ref() == "ConditionEffectSelf" || text.as_ref() == "Sneak")
                    {
                        let effect = pending_attrs
                            .get("effect")
                            .or_else(|| pending_attrs.get("conditionEffect"))
                            .cloned()
                            .unwrap_or_default();
                        current_ability.effects.push(AbilityEffect::Condition(
                            ConditionSelfEffect {
                                effect,
                                duration: af(&pending_attrs, "duration", 0.0),
                            },
                        ));
                    } else if in_activate && text.as_ref() == "Decoy" {
                        current_ability.effects.push(AbilityEffect::Decoy {
                            duration: af(&pending_attrs, "duration", 0.0),
                            speed: af(&pending_attrs, "speed", 1.0),
                        });
                    } else if in_activate && text.as_ref() == "Teleport" {
                        current_ability.effects.push(AbilityEffect::Teleport {
                            max_distance: af(&pending_attrs, "maxDistance", 0.0),
                        });
                    } else if in_activate && text.as_ref() == "Dash" {
                        if let Some(d) = pending_attrs
                            .get("damage")
                            .and_then(|v| v.parse::<i32>().ok())
                        {
                            pending_dash_damage = Some(d);
                        }
                    } else if in_on_enemy_hit
                        && text.as_ref() == "ApplyCondition"
                        && pending_attrs.get("effect").map(|s| s.as_str()) == Some("Hex")
                    {
                        current_ability.applies_hex = true;
                    } else if in_on_player_shoot
                        && on_player_shoot_is_ls
                        && text.as_ref() == "BulletCreate"
                    {
                        ls_extra_shots += 1;
                    }
                }
                Ok(Event::End(ref e)) => {
                    match e.name().as_ref() {
                        b"Object" => {
                            // Resolve deferred Shoot/BulletNova bursts now that the
                            // object's <Projectile> damage is known.
                            let proj = pending_proj.unwrap_or((0.0, 0.0, false));
                            for (min_hex, max_hex, attrs, default_shots) in pending_bursts.drain(..)
                            {
                                let burst = parse_projectile_burst(&attrs, proj, default_shots);
                                current_ability
                                    .effects
                                    .push(AbilityEffect::ProjectileBurst {
                                        min_hex,
                                        max_hex,
                                        burst,
                                    });
                            }
                            // Attach the counted LS extra shots to the params.
                            if let Some(ref mut ls) = current_lethal_strike {
                                ls.extra_shots = ls_extra_shots;
                            }
                            // Resolve deferred ShurikenAbility using the item's
                            // projectile damage + NumProjectiles.
                            if let Some(attrs) = pending_shuriken.take() {
                                let (mn, mx, ap) = proj;
                                current_ability.effects.push(AbilityEffect::Shuriken(
                                    ShurikenEffect {
                                        min_damage: mn,
                                        max_damage: mx,
                                        num_projectiles: current_num_projectiles,
                                        armor_piercing: ap,
                                        scaling_stat: attrs
                                            .get("scalingStat")
                                            .and_then(|s| ScalingStat::parse(s)),
                                        stat_mod_scaling_min: af(&attrs, "statModScalingMin", 0.0),
                                        stat_mod_damage: af(&attrs, "statModDamage", 0.0),
                                    },
                                ));
                            }
                            // Resolve deferred dash trail damage.
                            if let Some(dmg) = pending_dash_damage.take() {
                                current_ability
                                    .effects
                                    .push(AbilityEffect::DashTrail { damage: dmg });
                            }
                            // Merge data into existing object
                            if let Some(type_id) = current_type_id {
                                if let Some(obj) = self.objects.get_mut(&type_id) {
                                    obj.tier = current_tier;
                                    obj.feed_power = current_feed_power;
                                    obj.fame_bonus = current_fame_bonus;
                                    obj.bag_type = current_bag_type;
                                    if current_set_name.is_some() {
                                        obj.set_name = current_set_name.take();
                                    }
                                    obj.lethal_strike = current_lethal_strike;
                                    obj.collection_icon = current_collection_icon;
                                    obj.rate_of_fire = current_rate_of_fire;
                                    obj.num_projectiles = current_num_projectiles;
                                    obj.stat_bonuses = current_stat_bonuses;
                                    obj.weapon_procs = std::mem::take(&mut current_weapon_procs);
                                    merged_count += 1;
                                }
                                if !current_stat_relatives.is_empty() {
                                    self.stat_relatives.insert(
                                        type_id,
                                        std::mem::take(&mut current_stat_relatives),
                                    );
                                }
                                if let Some(target) = current_unlock_target.take() {
                                    unlock_map.insert(type_id, target);
                                }
                                if current_ability.has_readout() || current_ability.applies_hex {
                                    ability_map
                                        .insert(type_id, std::mem::take(&mut current_ability));
                                }
                            }
                        }
                        b"Tier" => in_tier = false,
                        b"feedPower" => in_feed_power = false,
                        b"XPBonus" => in_xp_bonus = false,
                        b"BagType" => in_bag_type = false,
                        b"RateOfFire" => in_rate_of_fire = false,
                        b"NumProjectiles" => in_num_projectiles = false,
                        b"Projectile" => {
                            if proj_ap {
                                if let Some((mn, mx, _)) = pending_proj {
                                    pending_proj = Some((mn, mx, true));
                                }
                            }
                            in_projectile = false;
                        }
                        b"MinDamage" => in_proj_min = false,
                        b"MaxDamage" => in_proj_max = false,
                        b"ActivateOnEquip" => {
                            in_activate_on_equip = false;
                            pending_equip_stat = None;
                            pending_equip_relative = None;
                        }
                        b"OnConditionEndActivate" => {
                            in_cond_end_activate = false;
                            pending_lethal_strike = None;
                        }
                        b"Activate" => {
                            in_activate = false;
                            pending_activate_id = None;
                        }
                        b"OnDetonateHexActivate" => in_on_detonate = false,
                        b"OnEnemyHitActivate" => in_on_enemy_hit = false,
                        b"OnPlayerShootActivate" => in_on_player_shoot = false,
                        _ => {}
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => {
                    return Err(format!("XML parse error: {}", e));
                }
                _ => {}
            }
            buf.clear();
        }

        self.blueprint_unlock = unlock_map;

        // Resolve deferred BulletCreate entries that reference external
        // projectile objects (type="0xNNNN"). Now that every object is loaded we
        // can look up the referenced object's <Projectile> damage.
        for (owner, ext_id, attrs, shots) in deferred_bullet_creates {
            let proj = self
                .objects
                .get(&ext_id)
                .and_then(|o| o.projectiles.first())
                .map(|p| (p.min_damage as f32, p.max_damage as f32, p.armor_piercing))
                .unwrap_or((0.0, 0.0, false));
            if proj.0 > 0.0 || proj.1 > 0.0 {
                let burst = parse_projectile_burst(&attrs, proj, shots);
                let entry = ability_map
                    .entry(owner)
                    .or_insert_with(AbilityEffects::default);
                entry.effects.push(AbilityEffect::ProjectileBurst {
                    min_hex: 0,
                    max_hex: 100,
                    burst,
                });
            }
        }

        // Resolve deferred SpawnCreep/RaiseDead entries by looking up the
        // summoned entity's projectile damage from name_to_id + objects.
        // Deduplicate: some items have duplicate Ability blocks referencing
        // the same entity (e.g. Bloodsucker Skull has two RaiseDead activates).
        deferred_spawn_creeps.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
        for (owner, obj_name, attrs, is_undead) in deferred_spawn_creeps {
            let summon = self
                .name_to_id
                .get(&obj_name)
                .and_then(|&id| self.objects.get(&id));

            // Collect visible projectile variants (skip ignoreOnTooltip).
            let projectiles: Vec<SummonProjectile> = summon
                .map(|o| {
                    // If any projectile has a display_id, only show those
                    // (tooltip-visible variants). Otherwise show all with damage.
                    let has_display_ids = o.projectiles.iter().any(|p| p.display_id.is_some());
                    o.projectiles
                        .iter()
                        .filter(|p| {
                            if has_display_ids {
                                p.display_id.is_some() && !p.ignore_on_tooltip
                            } else {
                                !p.ignore_on_tooltip && (p.min_damage > 0 || p.max_damage > 0)
                            }
                        })
                        .map(|p| SummonProjectile {
                            name: p.display_id.clone().unwrap_or_default(),
                            min_damage: p.min_damage as f32,
                            max_damage: p.max_damage as f32,
                            armor_piercing: p.armor_piercing,
                        })
                        .collect()
                })
                .unwrap_or_default();

            // Skip non-damaging summons (visual, fishing, helper objects).
            if projectiles.is_empty() {
                // Fallback: check if ANY projectile has damage (display metadata
                // may not be loaded from allies.xml yet).
                let any_damage = summon
                    .map(|o| {
                        o.projectiles
                            .iter()
                            .any(|p| p.min_damage > 0 || p.max_damage > 0)
                    })
                    .unwrap_or(false);
                if !any_damage {
                    continue;
                }
                // Use first projectile as fallback when display metadata missing.
                let p = summon.unwrap().projectiles.first().unwrap();
                let fallback = vec![SummonProjectile {
                    name: String::new(),
                    min_damage: p.min_damage as f32,
                    max_damage: p.max_damage as f32,
                    armor_piercing: p.armor_piercing,
                }];

                let display = summon
                    .map(|o| &o.display_name)
                    .filter(|n| !n.trim().is_empty())
                    .cloned()
                    .unwrap_or_else(|| obj_name.clone());

                let scaling_stat = attrs.get("scalingStat").and_then(|s| ScalingStat::parse(s));

                let entry = ability_map
                    .entry(owner)
                    .or_insert_with(AbilityEffects::default);
                entry
                    .effects
                    .push(AbilityEffect::SpawnCreep(SpawnCreepEffect {
                        display_name: display,
                        is_undead,
                        summon_cost: af(&attrs, "summonCost", 0.0),
                        projectiles: fallback,
                        scaling_stat,
                        stat_mod_damage: af(&attrs, "statModDamage", 0.0),
                        stat_mod_scaling_min: af(&attrs, "statModScalingMin", 0.0),
                    }));
                continue;
            }

            let display = summon
                .map(|o| &o.display_name)
                .filter(|n| !n.trim().is_empty())
                .cloned()
                .unwrap_or_else(|| obj_name.clone());

            let scaling_stat = attrs.get("scalingStat").and_then(|s| ScalingStat::parse(s));

            let entry = ability_map
                .entry(owner)
                .or_insert_with(AbilityEffects::default);
            entry
                .effects
                .push(AbilityEffect::SpawnCreep(SpawnCreepEffect {
                    display_name: display,
                    is_undead,
                    summon_cost: af(&attrs, "summonCost", 0.0),
                    projectiles,
                    scaling_stat,
                    stat_mod_damage: af(&attrs, "statModDamage", 0.0),
                    stat_mod_scaling_min: af(&attrs, "statModScalingMin", 0.0),
                }));
        }

        self.ability_effects = ability_map;
        Ok(merged_count)
    }

    /// Enrich Summon-class objects' projectile metadata (displayId, ignoreOnTooltip)
    /// from allies.xml. Called before merge_equipment_xml so SpawnCreep resolution
    /// can filter to tooltip-visible projectile variants.
    pub fn merge_allies_xml<P: AsRef<Path>>(&mut self, path: P) -> Result<usize, String> {
        use quick_xml::events::Event;
        use quick_xml::Reader;

        let file =
            File::open(path.as_ref()).map_err(|e| format!("Failed to open allies XML: {}", e))?;
        let reader = BufReader::new(file);
        let mut xml_reader = Reader::from_reader(reader);
        xml_reader.config_mut().trim_text(true);

        let mut buf = Vec::new();
        let mut updated = 0usize;
        let mut current_type_id: Option<i32> = None;
        let mut is_summon = false;
        let mut in_class = false;
        let mut proj_index: usize = 0;
        let mut proj_display_id: Option<String> = None;
        let mut proj_ignore: bool = false;

        loop {
            match xml_reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => match e.name().as_ref() {
                    b"Object" => {
                        current_type_id = None;
                        is_summon = false;
                        proj_index = 0;
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"type" {
                                let val = String::from_utf8_lossy(&attr.value).to_string();
                                current_type_id = parse_hex_type(&val);
                            }
                        }
                    }
                    b"Class" => {
                        in_class = true;
                    }
                    b"Projectile" => {
                        proj_display_id = None;
                        proj_ignore = false;
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"displayId" {
                                proj_display_id =
                                    Some(String::from_utf8_lossy(&attr.value).to_string());
                            } else if attr.key.as_ref() == b"ignoreOnTooltip" {
                                let v = String::from_utf8_lossy(&attr.value);
                                proj_ignore = v.as_ref() == "true" || v.as_ref() == "1";
                            }
                        }
                    }
                    _ => {}
                },
                Ok(Event::Empty(ref e)) => {
                    if e.name().as_ref() == b"MultiHit" || e.name().as_ref() == b"PassesCover" {
                        // skip empty child elements inside Projectile
                    }
                }
                Ok(Event::Text(ref t)) => {
                    if in_class {
                        let text = t.unescape().unwrap_or_default();
                        is_summon = text.as_ref() == "Summon" || text.as_ref() == "Undead";
                    }
                }
                Ok(Event::End(ref e)) => match e.name().as_ref() {
                    b"Class" => {
                        in_class = false;
                    }
                    b"Projectile" => {
                        if is_summon {
                            if let Some(type_id) = current_type_id {
                                if let Some(obj) = self.objects.get_mut(&type_id) {
                                    if let Some(p) = obj.projectiles.get_mut(proj_index) {
                                        p.display_id = proj_display_id.take();
                                        p.ignore_on_tooltip = proj_ignore;
                                        updated += 1;
                                    }
                                }
                            }
                        }
                        proj_index += 1;
                    }
                    b"Object" => {
                        current_type_id = None;
                        is_summon = false;
                    }
                    _ => {}
                },
                Ok(Event::Eof) => break,
                Err(e) => {
                    return Err(format!("Allies XML parse error: {}", e));
                }
                _ => {}
            }
            buf.clear();
        }

        Ok(updated)
    }
}

/// Parse hex type string (e.g., "0xa00" -> 2560).
fn parse_hex_type(s: &str) -> Option<i32> {
    let s = s.trim();
    if s.starts_with("0x") || s.starts_with("0X") {
        i32::from_str_radix(&s[2..], 16).ok()
    } else {
        s.parse().ok()
    }
}

/// Collect an XML element's attributes into a name -> value map.
fn attrs_to_map(e: &quick_xml::events::BytesStart) -> HashMap<String, String> {
    e.attributes()
        .flatten()
        .map(|a| {
            (
                String::from_utf8_lossy(a.key.as_ref()).to_string(),
                String::from_utf8_lossy(&a.value).to_string(),
            )
        })
        .collect()
}

/// Read a float attribute, defaulting when absent/unparsable.
fn af(m: &HashMap<String, String>, k: &str, d: f32) -> f32 {
    m.get(k).and_then(|v| v.parse().ok()).unwrap_or(d)
}

/// Read a u32 attribute, defaulting when absent/unparsable.
fn au(m: &HashMap<String, String>, k: &str, d: u32) -> u32 {
    m.get(k).and_then(|v| v.parse().ok()).unwrap_or(d)
}

fn parse_detonate_hex(m: &HashMap<String, String>) -> DetonateHexEffect {
    DetonateHexEffect {
        radius: af(m, "radius", 0.0),
        base_damage: af(m, "baseDamage", 0.0),
        stack_damage: af(m, "stackDamage", 0.0),
        stack_multiplier: af(m, "stackMultiplier", 0.0),
        scaling_stat: m.get("scalingStat").and_then(|s| ScalingStat::parse(s)),
        stat_mod_scaling_min: af(m, "statModScalingMin", 0.0),
        stat_mod_base_damage: af(m, "statModBaseDamage", 0.0),
        stat_mod_stack_damage: af(m, "statModStackDamage", 0.0),
        stat_mod_radius: af(m, "statModRadius", 0.0),
    }
}

fn parse_poison_grenade(m: &HashMap<String, String>) -> PoisonGrenadeEffect {
    PoisonGrenadeEffect {
        radius: af(m, "radius", 0.0),
        impact_damage: af(m, "impactDamage", 0.0),
        total_damage: af(m, "totalDamage", 0.0),
        duration_s: af(m, "duration", 0.0),
        scaling_stat: m.get("scalingStat").and_then(|s| ScalingStat::parse(s)),
        stat_mod_scaling_min: af(m, "statModScalingMin", 0.0),
        stat_mod_damage: af(m, "statModDamage", 0.0),
        stat_mod_impact_damage: af(m, "statModImpactDamage", 0.0),
        stat_mod_radius: af(m, "statModRadius", 0.0),
    }
}

/// Build a `ProjectileBurst` from a `Shoot`/`BulletNova` `<Activate>` plus the
/// owning item's `<Projectile>` damage range. `default_shots` is used when the
/// effect element has no `numShots` (e.g. a spell using `<NumProjectiles>`).
fn parse_projectile_burst(
    m: &HashMap<String, String>,
    proj: (f32, f32, bool),
    default_shots: i32,
) -> ProjectileBurstEffect {
    ProjectileBurstEffect {
        min_damage: proj.0,
        max_damage: proj.1,
        num_shots: m
            .get("numShots")
            .and_then(|v| v.parse().ok())
            .unwrap_or(default_shots),
        armor_piercing: proj.2,
        scaling_stat: m.get("scalingStat").and_then(|s| ScalingStat::parse(s)),
        stat_mod_scaling_min: af(m, "statModScalingMin", 0.0),
        stat_mod_damage: af(m, "statModDamage", 0.0),
        stat_mod_num_shots: af(m, "statModNumShots", 0.0),
    }
}

fn parse_restore(m: &HashMap<String, String>, is_mp: bool) -> RestoreEffect {
    RestoreEffect {
        amount: af(m, "amount", 0.0),
        is_mp,
        scaling_stat: m.get("scalingStat").and_then(|s| ScalingStat::parse(s)),
        stat_mod_scaling_min: af(m, "statModScalingMin", 0.0),
        stat_mod_amount: af(m, "statModAmount", 0.0),
    }
}

fn parse_lightning(m: &HashMap<String, String>) -> LightningEffect {
    LightningEffect {
        total_damage: af(m, "totalDamage", 0.0),
        decr_damage: af(m, "decrDamage", 0.0),
        max_targets: m
            .get("maxTargets")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1),
        wis_damage_base: af(m, "wisDamageBase", 0.0),
        wis_min: af(m, "wisMin", 0.0),
        wis_per_target: af(m, "wisPerTarget", 0.0),
        aoe_damage: af(m, "aoeDamage", 0.0),
        aoe_activation_count: m
            .get("aoeActivationCount")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        aoe_max_targets: m
            .get("aoeMaxTargets")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        aoe_range: af(m, "aoeRange", 0.0),
        scaling_stat: m.get("scalingStat").and_then(|s| ScalingStat::parse(s)),
        stat_mod_scaling_min: af(m, "statModScalingMin", 0.0),
        stat_mod_damage: af(m, "statModDamage", 0.0),
        stat_mod_per_target: af(m, "statModPerTarget", 0.0),
        stat_mod_radius: af(m, "statModRadius", 0.0),
    }
}

fn parse_trap(m: &HashMap<String, String>) -> TrapEffect {
    TrapEffect {
        total_damage: af(m, "totalDamage", 0.0),
        radius: af(m, "radius", 0.0),
        bomb_damage: af(m, "bombDamage", 0.0),
        bomb_count: af(m, "bombCount", 0.0),
        scaling_stat: m.get("scalingStat").and_then(|s| ScalingStat::parse(s)),
        stat_mod_scaling_min: af(m, "statModScalingMin", 0.0),
        stat_mod_trap_damage: af(m, "statModTrapDamage", 0.0),
        stat_mod_bomb_damage: af(m, "statModBombDamage", 0.0),
    }
}

fn parse_vampire_blast(m: &HashMap<String, String>) -> VampireBlastEffect {
    VampireBlastEffect {
        total_damage: af(m, "totalDamage", 0.0),
        heal: af(m, "heal", 0.0),
        ignore_def: af(m, "ignoreDef", 0.0),
        radius: af(m, "radius", 0.0),
        wis_damage_base: af(m, "wisDamageBase", 0.0),
        wis_min: af(m, "wisMin", 0.0),
    }
}

fn parse_damage_nova(m: &HashMap<String, String>) -> DamageNovaEffect {
    DamageNovaEffect {
        activation_count: af(m, "activationCount", 1.0),
        time: af(m, "time", 0.0),
        min_damage: af(m, "minDamage", 0.0),
        max_damage: af(m, "maxDamage", 0.0),
        radius: af(m, "radius", 0.0),
        scaling_stat: m.get("scalingStat").and_then(|s| ScalingStat::parse(s)),
        stat_mod_scaling_min: af(m, "statModScalingMin", 0.0),
        stat_mod_damage: af(m, "statModDamage", 0.0),
        stat_mod_activation_count: af(m, "statModActivationCount", 0.0),
        stat_mod_radius: af(m, "statModRadius", 0.0),
    }
}

/// Collection of loaded tile assets with efficient lookup.
#[derive(Debug, Default)]
pub struct TileList {
    /// Map from tile ID to asset data
    tiles: HashMap<i32, TileAsset>,
}

impl TileList {
    /// Create a new empty tile list.
    pub fn new() -> Self {
        Self {
            tiles: HashMap::new(),
        }
    }

    /// Load tile list from a file path.
    ///
    /// File format: `id;textureData;damage;idName`
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, std::io::Error> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut tiles = HashMap::new();

        for line in reader.lines() {
            let line = line?;
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            if let Some(asset) = Self::parse_tile_line(&line) {
                tiles.insert(asset.id, asset);
            }
        }

        // Add placeholder for unknown tiles
        tiles.insert(
            -1,
            TileAsset {
                id: -1,
                id_name: "Unknown".to_string(),
                damage: 0,
                textures: Vec::new(),
            },
        );

        Ok(Self { tiles })
    }

    /// Parse a single line from TileID.list
    /// Format: `id;textureData;damageMin,damageMax;idName`
    fn parse_tile_line(line: &str) -> Option<TileAsset> {
        let parts: Vec<&str> = line.split(';').collect();
        if parts.len() < 4 {
            return None;
        }

        let id = parts[0].parse::<i32>().ok()?;
        let texture_data = parts[1];
        let damage_str = parts[2];
        let id_name = parts[3].to_string();

        // Parse damage (min,max - we take min since they're usually equal)
        let damage = if damage_str.is_empty() {
            0
        } else {
            let damage_parts: Vec<&str> = damage_str.split(',').collect();
            damage_parts[0].parse::<i32>().unwrap_or(0)
        };

        // Parse textures
        let textures = ObjectList::parse_textures(texture_data);

        Some(TileAsset {
            id,
            id_name,
            damage,
            textures,
        })
    }

    /// Look up a tile by ID.
    pub fn get(&self, id: i32) -> Option<&TileAsset> {
        self.tiles.get(&id)
    }

    /// Get the name for a tile ID.
    pub fn name(&self, id: i32) -> Option<&str> {
        self.tiles.get(&id).map(|t| t.id_name.as_str())
    }

    /// Get the number of loaded tiles.
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    /// Check if the list is empty.
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }
}

/// Parse a string as a hex (`0x...`) or decimal number into `u32`.
fn parse_hex_u32(s: &str) -> u32 {
    let s = s.trim();
    if s.starts_with("0x") || s.starts_with("0X") {
        u32::from_str_radix(&s[2..], 16).unwrap_or(0)
    } else {
        s.parse().unwrap_or(0)
    }
}

/// Decoded Tex1/Tex2 information.
#[derive(Debug, Clone, PartialEq)]
pub enum TexInfo {
    /// No dye or cloth applied.
    None,
    /// Solid RGB color dye (`0x01RRGGBB`).
    Color(u8, u8, u8),
    /// Textile pattern: sprite sheet name + index within it.
    Textile { sheet: &'static str, index: i32 },
}

/// Decode a RotMG Tex1/Tex2 value.
///
/// Encoding:
/// - `0x00000000` / `0xFFFFFFFF` → [`TexInfo::None`]
/// - `0x01RRGGBB` → [`TexInfo::Color`] (solid dye)
/// - `0x04______` → [`TexInfo::Textile`] (`textile4x4`)
/// - `0x05______` → [`TexInfo::Textile`] (`textile5x5`)
/// - `0x09______` → [`TexInfo::Textile`] (`textile9x9`)
/// - `0x0A______` → [`TexInfo::Textile`] (`textile10x10`)
pub fn decode_tex(tex: u32) -> TexInfo {
    if tex == 0 || tex == 0xFFFF_FFFF {
        return TexInfo::None;
    }

    let high_byte = (tex >> 24) & 0xFF;
    let index = (tex & 0x00FF_FFFF) as i32;

    match high_byte {
        0x01 => TexInfo::Color(
            ((tex >> 16) & 0xFF) as u8,
            ((tex >> 8) & 0xFF) as u8,
            (tex & 0xFF) as u8,
        ),
        0x04 => TexInfo::Textile {
            sheet: "textile4x4",
            index,
        },
        0x05 => TexInfo::Textile {
            sheet: "textile5x5",
            index,
        },
        0x09 => TexInfo::Textile {
            sheet: "textile9x9",
            index,
        },
        0x0A => TexInfo::Textile {
            sheet: "textile10x10",
            index,
        },
        _ => TexInfo::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_object_line() {
        let line = "2827;Sword of the Colossus;Equipment;Weapon;1,200,275,1;0,lofiObj5,;UT;SwordOfTheColossus";
        let asset = ObjectList::parse_object_line(line).unwrap();

        assert_eq!(asset.id, 2827);
        assert_eq!(asset.display_name, "Sword of the Colossus");
        assert_eq!(asset.class, "Equipment");
        assert_eq!(asset.group, "Weapon");
        assert_eq!(asset.labels, "UT");
        assert_eq!(asset.id_name, "SwordOfTheColossus");

        assert_eq!(asset.textures.len(), 1);
        assert_eq!(asset.textures[0].name, "lofiObj5");
        assert_eq!(asset.textures[0].index, 0);

        assert_eq!(asset.projectiles.len(), 1);
        assert_eq!(asset.projectiles[0].min_damage, 200);
        assert_eq!(asset.projectiles[0].max_damage, 275);
        assert!(asset.projectiles[0].armor_piercing);
    }

    #[test]
    fn test_parse_object_line_defense_field() {
        // 13-field line: enemy with base defense in the trailing field.
        let line = "4324;Oryx the Mad God;Character;;;chars8x8dObjects:0x10;ENEMY,BOSS;Oryx the Mad God 3;;;;0;40";
        let asset = ObjectList::parse_object_line(line).unwrap();
        assert_eq!(asset.defense, 40);

        // Back-compat: an 8-field line has no defense field and defaults to 0.
        let old = "2827;Sword of the Colossus;Equipment;Weapon;1,200,275,1;0,lofiObj5,;UT;SwordOfTheColossus";
        assert_eq!(ObjectList::parse_object_line(old).unwrap().defense, 0);
    }

    #[test]
    fn test_parse_object_line_hitbox_scale_field() {
        // 14-field line: enemy with a CustomHitbox scale in the trailing field.
        let line = "3334;Limon the Sprite God;Character;;;spriteWorldChars16x16:0x04;ENEMY,BOSS;Limon the Sprite God;;;;0;16;2";
        let asset = ObjectList::parse_object_line(line).unwrap();
        assert_eq!(asset.defense, 16);
        assert!((asset.hitbox_scale - 2.0).abs() < 1e-6);

        // Back-compat: a 13-field line has no hitbox field and defaults to 1.0.
        let old = "4324;Oryx the Mad God;Character;;;chars8x8dObjects:0x10;ENEMY,BOSS;Oryx the Mad God 3;;;;0;40";
        assert!((ObjectList::parse_object_line(old).unwrap().hitbox_scale - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_parse_object_line_new_format() {
        // Test new format from XML parser: name:0xHEX
        let line = "1;;Projectile;;;lofiObj2:0x70;;Heavy Crossbow Bolt";
        let asset = ObjectList::parse_object_line(line).unwrap();

        assert_eq!(asset.id, 1);
        assert_eq!(asset.class, "Projectile");
        assert_eq!(asset.id_name, "Heavy Crossbow Bolt");

        assert_eq!(asset.textures.len(), 1);
        assert_eq!(asset.textures[0].name, "lofiObj2");
        assert_eq!(asset.textures[0].index, 0x70);
    }

    #[test]
    fn build_display_index_resolves_shared_name_to_canonical_enemy() {
        // A display name shared by the main boss, an invisible spawned clone,
        // and an NPC should resolve to the visible boss (matching loot history).
        let lines = [
            "23681;Soulwarden Murcian;Character;;;specPenChars16x16:0;ENEMY,BOSS,BOSSFIGHT;SpecPen Soulwarden Murcian",
            "23757;Soulwarden Murcian;Character;;;invisible:0,specPenObjects16x16:693;ENEMY,BOSS_SPAWNED,BOSSFIGHT;SpecPen Murcian Clone",
            "23797;Soulwarden Murcian;Character;;;specPenObjects32x32:477;;SpecPen Admin Murcian NPC",
        ];
        let mut objects = HashMap::new();
        for line in lines {
            let asset = ObjectList::parse_object_line(line).unwrap();
            objects.insert(asset.id, asset);
        }

        let index = ObjectList::build_display_index(&objects);
        assert_eq!(index.get("soulwarden murcian"), Some(&23681));
    }

    #[test]
    fn build_display_index_prefers_visible_texture_over_invisible() {
        // Same name on an invisible marker and a visible decoration: pick the
        // one that can actually be drawn.
        let lines = [
            "100;Thing;GameObject;;;invisible:0;ENEMY;Thing Marker",
            "101;Thing;GameObject;;;lofiObj:5;;Thing Visible",
        ];
        let mut objects = HashMap::new();
        for line in lines {
            let asset = ObjectList::parse_object_line(line).unwrap();
            objects.insert(asset.id, asset);
        }

        let index = ObjectList::build_display_index(&objects);
        assert_eq!(index.get("thing"), Some(&101));
    }

    #[test]
    fn build_display_index_indexes_empty_display_encounter_boss() {
        // New encounter bosses ship with an empty display name (their readable
        // name is the id_name) and carry the real sprite + ENCOUNTER label, while
        // a same-named invisible marker holds the display name. The renderable
        // boss must win the name lookup so mission cards get an icon. Cube Deity
        // has no display-name twin at all and must still resolve via its id_name.
        let lines = [
            "47909;;Character;;;sorcEncountersChars16x16:57;ENEMY,ENCOUNTER,ADEPT_ENCOUNTER;Towering Perfection",
            "47923;Towering Perfection;Character;;;invisible:0;;Tower Pattern",
            "47927;;Character;;;sorcEncountersObjects16x16:298;ENEMY,ENCOUNTER,VETERAN_ENCOUNTER;Cube Deity",
        ];
        let mut objects = HashMap::new();
        for line in lines {
            let asset = ObjectList::parse_object_line(line).unwrap();
            objects.insert(asset.id, asset);
        }

        let index = ObjectList::build_display_index(&objects);
        assert_eq!(index.get("towering perfection"), Some(&47909));
        assert_eq!(index.get("cube deity"), Some(&47927));
    }

    #[test]
    fn test_parse_textures() {
        let textures = ObjectList::parse_textures("0,lofiObj,1,lofiObj2,");
        assert_eq!(textures.len(), 2);
        assert_eq!(textures[0].index, 0);
        assert_eq!(textures[0].name, "lofiObj");
        assert_eq!(textures[1].index, 1);
        assert_eq!(textures[1].name, "lofiObj2");
    }

    #[test]
    fn test_parse_textures_new_format() {
        // New format: name:index
        let textures = ObjectList::parse_textures("lofiObj:0x70,lofiObj2:0xde");
        assert_eq!(textures.len(), 2);
        assert_eq!(textures[0].name, "lofiObj");
        assert_eq!(textures[0].index, 0x70);
        assert_eq!(textures[1].name, "lofiObj2");
        assert_eq!(textures[1].index, 0xde);
    }

    #[test]
    fn test_parse_projectiles_new_format() {
        // New format: min:max:ap,min:max:ap
        let projectiles = ObjectList::parse_projectiles("100:200:0,150:300:1");
        assert_eq!(projectiles.len(), 2);
        assert_eq!(projectiles[0].min_damage, 100);
        assert_eq!(projectiles[0].max_damage, 200);
        assert!(!projectiles[0].armor_piercing);
        assert_eq!(projectiles[1].min_damage, 150);
        assert_eq!(projectiles[1].max_damage, 300);
        assert!(projectiles[1].armor_piercing);
    }

    #[test]
    fn test_find_portal_prefers_id_name_match() {
        // When multiple portals share the same display_name, prefer the one
        // whose id_name contains the dungeon name (fixes the Tavern wrong-sprite case).
        let mut list = ObjectList::new();
        list.objects.insert(
            17917,
            ObjectAsset {
                id: 17917,
                id_name: "Beer God Encounter Portal".to_string(),
                display_name: "The Tavern".to_string(),
                class: "Portal".to_string(),
                group: String::new(),
                labels: String::new(),
                textures: Vec::new(),
                projectiles: Vec::new(),
                mask: None,
                tex1: 0,
                tex2: 0,
                slot_type: 0,
                defense: 0,
                hitbox_scale: 1.0,
                tier: None,
                feed_power: 0,
                fame_bonus: 0,
                bag_type: 0,
                set_name: None,
                lethal_strike: None,
                collection_icon: None,
                rate_of_fire: 1.0,
                num_projectiles: 1,
                stat_bonuses: StatBonuses::default(),
                weapon_procs: Vec::new(),
            },
        );
        list.objects.insert(
            17744,
            ObjectAsset {
                id: 17744,
                id_name: "The Tavern Portal".to_string(),
                display_name: "The Tavern".to_string(),
                class: "Portal".to_string(),
                group: String::new(),
                labels: String::new(),
                textures: Vec::new(),
                projectiles: Vec::new(),
                mask: None,
                tex1: 0,
                tex2: 0,
                slot_type: 0,
                defense: 0,
                hitbox_scale: 1.0,
                tier: None,
                feed_power: 0,
                fame_bonus: 0,
                bag_type: 0,
                set_name: None,
                lethal_strike: None,
                collection_icon: None,
                rate_of_fire: 1.0,
                num_projectiles: 1,
                stat_bonuses: StatBonuses::default(),
                weapon_procs: Vec::new(),
            },
        );

        // Must always resolve to 17744 ("The Tavern Portal"), not the encounter portal
        assert_eq!(list.find_portal_for_dungeon("The Tavern"), Some(17744));
    }

    #[test]
    fn test_object_name() {
        let asset = ObjectAsset {
            id: 1,
            id_name: "TestItem".to_string(),
            display_name: "Test Item Display".to_string(),
            class: String::new(),
            group: String::new(),
            labels: String::new(),
            textures: Vec::new(),
            projectiles: Vec::new(),
            mask: None,
            tex1: 0,
            tex2: 0,
            slot_type: 0,
            defense: 0,
            hitbox_scale: 1.0,
            tier: None,
            feed_power: 0,
            fame_bonus: 0,
            bag_type: 0,
            set_name: None,
            lethal_strike: None,
            collection_icon: None,
            rate_of_fire: 1.0,
            num_projectiles: 1,
            stat_bonuses: StatBonuses::default(),
            weapon_procs: Vec::new(),
        };
        assert_eq!(asset.name(), "Test Item Display");

        let asset_no_display = ObjectAsset {
            id: 2,
            id_name: "TestItem2".to_string(),
            display_name: String::new(),
            class: String::new(),
            group: String::new(),
            labels: String::new(),
            textures: Vec::new(),
            projectiles: Vec::new(),
            mask: None,
            tex1: 0,
            tex2: 0,
            slot_type: 0,
            defense: 0,
            hitbox_scale: 1.0,
            tier: None,
            feed_power: 0,
            fame_bonus: 0,
            bag_type: 0,
            set_name: None,
            lethal_strike: None,
            collection_icon: None,
            rate_of_fire: 1.0,
            num_projectiles: 1,
            stat_bonuses: StatBonuses::default(),
            weapon_procs: Vec::new(),
        };
        assert_eq!(asset_no_display.name(), "TestItem2");
    }

    #[test]
    #[ignore] // Requires extraction to have run first
    fn test_load_extracted_object_list() {
        let output_dir = std::env::temp_dir().join("RealmHound_test_assets");
        let list_path = output_dir.join("ObjectID.list");

        if !list_path.exists() {
            println!("ObjectID.list not found - run extraction test first");
            return;
        }

        let object_list =
            ObjectList::load_from_file(&list_path).expect("Failed to load ObjectID.list");

        println!("Loaded {} objects", object_list.len());

        // Verify we loaded a reasonable number
        assert!(object_list.len() > 1000, "Should have loaded many objects");

        // Verify we can look up objects by ID
        if let Some(obj) = object_list.get(1) {
            println!("Object 1: {} ({})", obj.name(), obj.class);
            assert!(!obj.textures.is_empty(), "Object should have textures");
        }
    }

    #[test]
    #[ignore] // Requires ObjectID.list in an explicit assets folder
    fn test_list_beacon_sprites() {
        // Load ObjectID.list from an explicitly configured assets folder; never
        // depend on a developer machine's LOCALAPPDATA.
        let Some(assets_dir) =
            std::env::var_os("REALMHOUND_TEST_ASSETS_DIR").map(std::path::PathBuf::from)
        else {
            println!("set REALMHOUND_TEST_ASSETS_DIR to run this manual beacon-sprite test");
            return;
        };
        let list_path = assets_dir.join("ObjectID.list");

        if !list_path.exists() {
            println!("ObjectID.list not found at {:?}", list_path);
            return;
        }

        let object_list =
            ObjectList::load_from_file(&list_path).expect("Failed to load ObjectID.list");

        println!("\n=== Beacon Sprites (beacons32x32) - 1 per object ===\n");

        // Collect unique beacon sprite indices we've seen
        let mut seen_indices: std::collections::HashSet<i32> = std::collections::HashSet::new();
        let mut beacon_entries: Vec<(i32, String, String, i32)> = Vec::new(); // (id, name, display_name, sprite_index)

        for obj in object_list.iter() {
            // Skip captured beacons
            if obj.display_name.to_lowercase().contains("captured")
                || obj.id_name.to_lowercase().contains("captured")
            {
                continue;
            }

            // Check if any texture uses beacons32x32
            for tex in &obj.textures {
                if tex.name == "beacons32x32" {
                    // Only add if we haven't seen this sprite index yet
                    if !seen_indices.contains(&tex.index) {
                        seen_indices.insert(tex.index);
                        beacon_entries.push((
                            obj.id,
                            obj.id_name.clone(),
                            obj.display_name.clone(),
                            tex.index,
                        ));
                    }
                    break; // Only use first texture per object
                }
            }
        }

        // Sort by sprite index
        beacon_entries.sort_by_key(|(_, _, _, idx)| *idx);

        println!("| Sprite Index | Object ID | ID Name | Display Name |");
        println!("|--------------|-----------|---------|--------------|");
        for (id, id_name, display_name, sprite_idx) in &beacon_entries {
            println!(
                "| {:12} | {:9} | {} | {} |",
                sprite_idx, id, id_name, display_name
            );
        }

        println!("\nTotal unique beacon sprites: {}", beacon_entries.len());
    }

    #[test]
    fn test_merge_parses_blueprint_unlock_target() {
        // A blueprint's unlocked item comes from its
        // <Activate id="X">UnlockForgeBlueprint</Activate>, where X is the
        // target item's internal id (not the blueprint's display name).
        let mut list = ObjectList::new();
        list.insert_asset(
            100,
            ObjectAsset {
                id: 100,
                id_name: "Divinity".to_string(),
                display_name: "Divinity".to_string(),
                class: "Equipment".to_string(),
                group: "Weapon".to_string(),
                labels: String::new(),
                textures: Vec::new(),
                projectiles: Vec::new(),
                mask: None,
                tex1: 0,
                tex2: 0,
                slot_type: 0,
                defense: 0,
                hitbox_scale: 1.0,
                tier: None,
                feed_power: 0,
                fame_bonus: 0,
                bag_type: 0,
                set_name: None,
                lethal_strike: None,
                collection_icon: None,
                rate_of_fire: 1.0,
                num_projectiles: 1,
                stat_bonuses: StatBonuses::default(),
                weapon_procs: Vec::new(),
            },
        );
        list.insert_asset(
            200,
            ObjectAsset {
                id: 200,
                id_name: "Blueprint_1".to_string(),
                display_name: "Divinity Blueprint".to_string(),
                class: "Equipment".to_string(),
                group: String::new(),
                labels: String::new(),
                textures: Vec::new(),
                projectiles: Vec::new(),
                mask: None,
                tex1: 0,
                tex2: 0,
                slot_type: 0,
                defense: 0,
                hitbox_scale: 1.0,
                tier: None,
                feed_power: 0,
                fame_bonus: 0,
                bag_type: 0,
                set_name: None,
                lethal_strike: None,
                collection_icon: None,
                rate_of_fire: 1.0,
                num_projectiles: 1,
                stat_bonuses: StatBonuses::default(),
                weapon_procs: Vec::new(),
            },
        );

        let xml = r#"<Objects>
  <Object type="0x00c8" id="Blueprint_1" collectionIcon="18">
    <DisplayId>Divinity Blueprint</DisplayId>
    <Class>Equipment</Class>
    <Activate id="Divinity">UnlockForgeBlueprint</Activate>
  </Object>
  <Object type="0x0064" id="Divinity">
    <DisplayId>Divinity</DisplayId>
    <Class>Equipment</Class>
    <Activate>Shoot</Activate>
  </Object>
</Objects>"#;

        let dir = std::env::temp_dir().join(format!("rh_bp_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("equip.xml");
        std::fs::write(&path, xml).unwrap();

        list.merge_equipment_xml(&path).unwrap();

        // name_to_id is normally built during load_from_file; populate it so the
        // internal-id -> object-id resolution can run.
        list.name_to_id.insert("Divinity".to_string(), 100);

        // 0x00c8 == 200 (blueprint) -> target internal id "Divinity" -> id 100.
        assert_eq!(list.blueprint_unlocked_id(200), Some(100));
        // A non-blueprint item's Shoot activate must not create a mapping.
        assert_eq!(list.blueprint_unlocked_id(100), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_merge_parses_increment_stat_relative() {
        // Wretched-Rags-shaped entry mixing flat IncrementStat with relative
        // IncrementStatRelative effects.
        let mut list = ObjectList::new();
        list.insert_asset(
            7408,
            ObjectAsset {
                id: 7408,
                id_name: "Wretched Rags Shiny".to_string(),
                display_name: "Wretched Rags Shiny".to_string(),
                class: "Equipment".to_string(),
                group: "Armor".to_string(),
                labels: String::new(),
                textures: Vec::new(),
                projectiles: Vec::new(),
                mask: None,
                tex1: 0,
                tex2: 0,
                slot_type: 0,
                defense: 0,
                hitbox_scale: 1.0,
                tier: None,
                feed_power: 0,
                fame_bonus: 0,
                bag_type: 0,
                set_name: None,
                lethal_strike: None,
                collection_icon: None,
                rate_of_fire: 1.0,
                num_projectiles: 1,
                stat_bonuses: StatBonuses::default(),
                weapon_procs: Vec::new(),
            },
        );

        let xml = r#"<Objects>
  <Object type="0x1CF0" id="Wretched Rags Shiny">
    <Class>Equipment</Class>
    <ActivateOnEquip amount="-50" stat="MAXHP" statRelativeTo="MAXHP">IncrementStatRelative</ActivateOnEquip>
    <ActivateOnEquip amount="100" stat="MAXHP" statRelativeTo="MAXMP">IncrementStatRelative</ActivateOnEquip>
    <ActivateOnEquip amount="5" stat="DEX">IncrementStat</ActivateOnEquip>
    <ActivateOnEquip amount="10" stat="DEF">IncrementStat</ActivateOnEquip>
  </Object>
</Objects>"#;
        let dir = std::env::temp_dir().join(format!("rh_rel_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("equip.xml");
        std::fs::write(&path, xml).unwrap();

        list.merge_equipment_xml(&path).unwrap();

        // Flat stat bonuses parse as before.
        let flat = list.item_stat_bonuses(7408).unwrap();
        assert!((flat.get(StatKind::Dexterity) - 5.0).abs() < 1e-4);
        assert!((flat.get(StatKind::Defense) - 10.0).abs() < 1e-4);

        // Two relative effects, both targeting MaxHp.
        let rel = list.item_stat_relatives(7408);
        assert_eq!(rel.len(), 2);
        assert_eq!(rel[0], (StatKind::MaxHp, -50.0, StatKind::MaxHp));
        assert_eq!(rel[1], (StatKind::MaxHp, 100.0, StatKind::MaxMp));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_merge_parses_detonate_hex_ability() {
        // Command-Cornea-shaped entry: DetonateHex scales per-stack with MAXMP.
        let mut list = ObjectList::new();
        let xml = r#"<Objects>
  <Object type="0x5cbe" id="Command Cornea">
    <Class>Equipment</Class>
    <OnEnemyHitActivate effect="Hex" duration="3">ApplyCondition</OnEnemyHitActivate>
    <Activate radius="5" baseDamage="0" stackDamage="5" stackMultiplier="0" scalingStat="MAXMP" statModScalingMin="300" statModBaseDamage="0" statModStackDamage="0.05" statModRadius="0.005">DetonateHex</Activate>
  </Object>
</Objects>"#;
        let dir = std::env::temp_dir().join(format!("rh_det_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("equip.xml");
        std::fs::write(&path, xml).unwrap();
        list.merge_equipment_xml(&path).unwrap();

        let eff = list.ability_effects(0x5cbe).expect("cornea effects parsed");
        assert!(eff.applies_hex, "on-enemy-hit Hex should be detected");
        assert!(eff.deals_damage());
        let det = eff.detonate_hex.expect("detonate hex parsed");
        assert_eq!(det.scaling_stat, Some(ScalingStat::MaxMp));

        // Realmeye: "5 (+0.05 per MP over 300) damage per stack".
        // At 300 MP (no bonus), 100 stacks -> 100 * 5 = 500.
        assert_eq!(det.damage_for(300, 100), 500);
        // At 1000 MP, per-stack = 5 + 0.05*700 = 40; 100 stacks -> 4000.
        assert_eq!(det.damage_for(1000, 100), 4000);
        // Radius: 5 (+0.005 per MP over 300). At 1000 MP -> 5 + 3.5 = 8.5.
        assert!((det.radius_for(1000) - 8.5).abs() < 1e-4);
        // Below the scaling floor, no negative bonus.
        assert_eq!(det.damage_for(100, 10), 50);

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn weapon_shell(id: i32, id_name: &str) -> ObjectAsset {
        ObjectAsset {
            id,
            id_name: id_name.to_string(),
            display_name: id_name.to_string(),
            class: "Equipment".to_string(),
            group: "Weapon".to_string(),
            labels: String::new(),
            textures: Vec::new(),
            projectiles: Vec::new(),
            mask: None,
            tex1: 0,
            tex2: 0,
            slot_type: 0,
            defense: 0,
            hitbox_scale: 1.0,
            tier: None,
            feed_power: 0,
            fame_bonus: 0,
            bag_type: 0,
            set_name: None,
            lethal_strike: None,
            collection_icon: None,
            rate_of_fire: 1.0,
            num_projectiles: 1,
            stat_bonuses: StatBonuses::default(),
            weapon_procs: Vec::new(),
        }
    }

    #[test]
    fn test_merge_parses_weapon_poison_proc() {
        // Doku-No-Ken-shaped weapon: on-shoot poison burst every 3s.
        let mut list = ObjectList::new();
        list.insert_asset(0xcdc, weapon_shell(0xcdc, "Doku No Ken"));

        let xml = r#"<Objects>
  <Object type="0xcdc" id="Doku No Ken">
    <Class>Equipment</Class>
    <OnPlayerShootActivate proc="1" cooldown="3" radius="6" impactDamage="0" totalDamage="1000" duration="6" throwTime="0">PoisonGrenade</OnPlayerShootActivate>
  </Object>
</Objects>"#;
        let dir = std::env::temp_dir().join(format!("rh_wproc_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("equip.xml");
        std::fs::write(&path, xml).unwrap();
        list.merge_equipment_xml(&path).unwrap();

        let procs = list.weapon_procs(0xcdc);
        assert_eq!(procs.len(), 1);
        let p = &procs[0];
        assert_eq!(p.trigger, ProcTrigger::OnShoot);
        assert!((p.proc_rate - 1.0).abs() < 1e-4);
        assert!((p.cooldown - 3.0).abs() < 1e-4);
        assert!(p.required_conditions.is_none());
        match &p.effect {
            ProcEffect::Poison(g) => {
                assert!((g.total_damage - 1000.0).abs() < 1e-4);
                assert_eq!(g.total_for(0), 1000);
            }
            other => panic!("expected poison, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_merge_parses_weapon_bleeding_proc() {
        // Chain-Dagger-shaped weapon: projectile applies Bleeding 50/s for 4.5s.
        let mut list = ObjectList::new();
        list.insert_asset(0xc21f, weapon_shell(0xc21f, "Chain Dagger"));

        let xml = r#"<Objects>
  <Object type="0xc21f" id="Chain Dagger">
    <Class>Equipment</Class>
    <Projectile>
      <ObjectId>Chain Dagger Projectile</ObjectId>
      <MinDamage>110</MinDamage>
      <MaxDamage>130</MaxDamage>
      <ConditionEffect duration="4.5" amount="50">Bleeding</ConditionEffect>
    </Projectile>
  </Object>
</Objects>"#;
        let dir = std::env::temp_dir().join(format!("rh_bleed_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("equip.xml");
        std::fs::write(&path, xml).unwrap();
        list.merge_equipment_xml(&path).unwrap();

        let procs = list.weapon_procs(0xc21f);
        assert_eq!(procs.len(), 1);
        let p = &procs[0];
        assert_eq!(p.trigger, ProcTrigger::OnHit);
        assert!(p.required_conditions.is_none());
        match &p.effect {
            ProcEffect::Bleeding(b) => {
                assert!((b.dmg_per_sec - 50.0).abs() < 1e-4);
                assert!((b.duration_s - 4.5).abs() < 1e-4);
            }
            other => panic!("expected bleeding, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_bleeding_defaults_amount_and_dedups() {
        // No damage attr -> default 20/s. The parser keeps a single strongest-
        // rate Bleeding per weapon; in-game per-hit stacking is modeled later at
        // DPS-derivation time, not by emitting duplicate procs here.
        let mut list = ObjectList::new();
        list.insert_asset(0x5, weapon_shell(0x5, "Bleeder"));

        let xml = r#"<Objects>
  <Object type="0x5" id="Bleeder">
    <Class>Equipment</Class>
    <Projectile>
      <MinDamage>10</MinDamage>
      <MaxDamage>20</MaxDamage>
      <ConditionEffect duration="3">Bleeding</ConditionEffect>
    </Projectile>
    <Projectile>
      <MinDamage>10</MinDamage>
      <MaxDamage>20</MaxDamage>
      <ConditionEffect duration="3" amount="15">Bleeding</ConditionEffect>
    </Projectile>
  </Object>
</Objects>"#;
        let dir = std::env::temp_dir().join(format!("rh_bleed_def_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("equip.xml");
        std::fs::write(&path, xml).unwrap();
        list.merge_equipment_xml(&path).unwrap();

        let procs = list.weapon_procs(0x5);
        assert_eq!(procs.len(), 1, "parser keeps one strongest-rate Bleeding");
        match &procs[0].effect {
            ProcEffect::Bleeding(b) => assert!((b.dmg_per_sec - 20.0).abs() < 1e-4),
            other => panic!("expected bleeding, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_bleeding_reads_bleeddamage_attr() {
        // Some items specify the drain via `bleedDamage` instead of `amount`;
        // the parser must read it (not fall back to the 20/s default).
        let mut list = ObjectList::new();
        list.insert_asset(0x6, weapon_shell(0x6, "Bleeder2"));

        let xml = r#"<Objects>
  <Object type="0x6" id="Bleeder2">
    <Class>Equipment</Class>
    <Projectile>
      <MinDamage>10</MinDamage>
      <MaxDamage>20</MaxDamage>
      <ArmorPiercing />
      <ConditionEffect bleedDamage="75" duration="4">Bleeding</ConditionEffect>
    </Projectile>
  </Object>
</Objects>"#;
        let dir = std::env::temp_dir().join(format!("rh_bleed_bd_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("equip.xml");
        std::fs::write(&path, xml).unwrap();
        list.merge_equipment_xml(&path).unwrap();

        let procs = list.weapon_procs(0x6);
        assert_eq!(procs.len(), 1);
        match &procs[0].effect {
            ProcEffect::Bleeding(b) => {
                assert!(
                    (b.dmg_per_sec - 75.0).abs() < 1e-4,
                    "bleedDamage=75 -> 75/s"
                );
                assert!((b.duration_s - 4.0).abs() < 1e-4);
            }
            other => panic!("expected bleeding, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_bleeding_dedup_prefers_highest_damage_seconds() {
        // Two bleeding projectile types: 50/s x 2s (=100 dmg-seconds) vs
        // 40/s x 4s (=160). The longer-lived, lower-rate source contributes more
        // sustained damage, so the parser must keep it despite its lower rate.
        let mut list = ObjectList::new();
        list.insert_asset(0x7, weapon_shell(0x7, "Bleeder3"));

        let xml = r#"<Objects>
  <Object type="0x7" id="Bleeder3">
    <Class>Equipment</Class>
    <Projectile>
      <MinDamage>10</MinDamage>
      <MaxDamage>20</MaxDamage>
      <ConditionEffect duration="2" amount="50">Bleeding</ConditionEffect>
    </Projectile>
    <Projectile>
      <MinDamage>10</MinDamage>
      <MaxDamage>20</MaxDamage>
      <ConditionEffect duration="4" amount="40">Bleeding</ConditionEffect>
    </Projectile>
  </Object>
</Objects>"#;
        let dir = std::env::temp_dir().join(format!("rh_bleed_ds_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("equip.xml");
        std::fs::write(&path, xml).unwrap();
        list.merge_equipment_xml(&path).unwrap();

        let procs = list.weapon_procs(0x7);
        assert_eq!(procs.len(), 1);
        match &procs[0].effect {
            ProcEffect::Bleeding(b) => {
                assert!(
                    (b.dmg_per_sec - 40.0).abs() < 1e-4,
                    "keep 40/s x 4s (more dmg-seconds)"
                );
                assert!((b.duration_s - 4.0).abs() < 1e-4);
            }
            other => panic!("expected bleeding, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delivered_total_drops_the_poison_expiry_tick() {
        // Toxic-Toad-shaped poison: impact 350, total 750, ATT-scaled, 1.0s.
        // At ATT 74 the nominal total is 990 (impact 379 + DoT 611), but the
        // DoT ticks every 200ms and drops the expiry tick -> 4 of 5 ticks land,
        // delivering impact 379 + 611*4/5 = 868 (matches live capture).
        let g = PoisonGrenadeEffect {
            radius: 2.0,
            impact_damage: 350.0,
            total_damage: 750.0,
            duration_s: 1.0,
            scaling_stat: Some(ScalingStat::Attack),
            stat_mod_scaling_min: 50.0,
            stat_mod_damage: 100.0,
            stat_mod_impact_damage: 12.0,
            stat_mod_radius: 0.025,
        };
        assert_eq!(g.total_for(74), 990);
        assert_eq!(g.impact_for(74), 379);
        assert_eq!(g.delivered_total_for(74), 868);

        // A 6.0s poison (Doku-shaped, all DoT) fires 29 of 30 ticks.
        let doku = PoisonGrenadeEffect {
            radius: 6.0,
            impact_damage: 0.0,
            total_damage: 1000.0,
            duration_s: 6.0,
            scaling_stat: None,
            stat_mod_scaling_min: 0.0,
            stat_mod_damage: 0.0,
            stat_mod_impact_damage: 0.0,
            stat_mod_radius: 0.0,
        };
        // 1000 * (0.2 * 29 / 6) = 966.67 -> 967.
        assert_eq!(doku.delivered_total_for(0), 967);

        // Zero-duration guard: deliver the full nominal total.
        let instant = PoisonGrenadeEffect {
            duration_s: 0.0,
            ..doku
        };
        assert_eq!(instant.delivered_total_for(0), 1000);
    }

    #[test]
    fn test_weapon_procs_isolated_between_objects() {
        // A proc on the first object must not leak into the second.
        let mut list = ObjectList::new();
        list.insert_asset(0x1, weapon_shell(0x1, "Poison Sword"));
        list.insert_asset(0x2, weapon_shell(0x2, "Plain Sword"));

        let xml = r#"<Objects>
  <Object type="0x1" id="Poison Sword">
    <Class>Equipment</Class>
    <OnPlayerShootActivate proc="1" cooldown="2" totalDamage="500" impactDamage="0" duration="4">PoisonGrenade</OnPlayerShootActivate>
  </Object>
  <Object type="0x2" id="Plain Sword">
    <Class>Equipment</Class>
  </Object>
</Objects>"#;
        let dir = std::env::temp_dir().join(format!("rh_wproc_iso_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("equip.xml");
        std::fs::write(&path, xml).unwrap();
        list.merge_equipment_xml(&path).unwrap();

        assert_eq!(list.weapon_procs(0x1).len(), 1);
        assert!(list.weapon_procs(0x2).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_weapon_proc_conditional_is_flagged() {
        // A `mustWear`-gated proc is parsed but marked conditional so DPS math
        // can exclude it.
        let mut list = ObjectList::new();
        list.insert_asset(0x3, weapon_shell(0x3, "Set Dagger"));

        let xml = r#"<Objects>
  <Object type="0x3" id="Set Dagger">
    <Class>Equipment</Class>
    <OnPlayerShootActivate proc="0.05" mustWear="0x2257" cooldown="0.4" totalDamage="750" impactDamage="350" duration="1">PoisonGrenade</OnPlayerShootActivate>
  </Object>
</Objects>"#;
        let dir = std::env::temp_dir().join(format!("rh_wproc_cond_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("equip.xml");
        std::fs::write(&path, xml).unwrap();
        list.merge_equipment_xml(&path).unwrap();

        let procs = list.weapon_procs(0x3);
        assert_eq!(procs.len(), 1);
        assert_eq!(procs[0].required_conditions.as_deref(), Some("0x2257"));
        assert!((procs[0].proc_rate - 0.05).abs() < 1e-4);

        let _ = std::fs::remove_dir_all(&dir);
    }
}

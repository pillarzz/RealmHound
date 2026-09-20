//! Parser for the Crucible definition carried by the `CrucibleResponse` packet
//! (ID 183). The packet's `crucible_jsons` strings each wrap a `{"array":[..]}`
//! envelope; the entry that owns a `crucible` object describes the season's
//! Crucible restrictions (flat stat buffs, on-hit effects) and bonuses (loot,
//! XP, BXP). Other entries in the same feed (e.g. `modifierEvent` event-dungeon
//! modifiers) are ignored here.
//!
//! Schema verified against the S30 payload:
//! ```json
//! {"array":[{"id":"..","title":"S30 Crucible","crucible":{
//!   "restrictions":[
//!     {"type":2,"stat":"ATT","percent":0,"value":5},
//!     {"type":2,"stat":"DEF","percent":0,"value":10},
//!     {"type":5,"effects":"Exposed","effectChance":0.3,"threshold":15,
//!      "effectsDuration":3,"effectsCooldown":4}],
//!   "bonuses":[
//!     {"type":1,"amount":1.15,"percent":1},
//!     {"type":2,"amount":1.1,"percent":1,"fame":1},
//!     {"type":3,"percent":1,"amount":1.1}]}}]}
//! ```

use crate::assets::StatKind;
use serde::{Deserialize, Serialize};

/// Restriction `type` == flat/percentage stat modifier.
const RESTRICTION_STAT_MOD: i32 = 2;
/// Restriction `type` == on-hit status effect.
const RESTRICTION_ON_HIT: i32 = 5;
/// Bonus `type` values.
const BONUS_BXP: i32 = 1;
const BONUS_XP: i32 = 2;
const BONUS_LOOT: i32 = 3;
const BONUS_DAMAGE: i32 = 5;

/// A flat (or percentage) stat modifier granted by the Crucible.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CrucibleStatMod {
    pub stat: StatKind,
    /// Magnitude. Flat when `percent` is false; otherwise a percentage of the
    /// stat's base value.
    pub value: f32,
    pub percent: bool,
}

/// An on-hit status effect the Crucible applies (e.g. Exposed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrucibleEffect {
    pub name: String,
    /// Chance to apply per qualifying hit (0..1).
    pub chance: f32,
    /// Minimum shot damage required to trigger.
    pub threshold: f32,
    /// Effect duration in seconds.
    pub duration: f32,
    /// Cooldown between applications in seconds.
    pub cooldown: f32,
}

/// A parsed Crucible definition for one season.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CrucibleDef {
    pub id: String,
    pub title: String,
    pub stat_mods: Vec<CrucibleStatMod>,
    pub effects: Vec<CrucibleEffect>,
    /// Battle-pass XP bonus, in percent (e.g. 15.0 for a 1.15x multiplier).
    pub bxp_pct: f32,
    /// Experience bonus, in percent.
    pub xp_pct: f32,
    /// Whether the XP bonus also grants fame.
    pub xp_fame: bool,
    /// Loot-drop bonus, in percent.
    pub loot_pct: f32,
    /// Damage multiplier (1.0 == no change).
    pub damage_mult: f32,
}

impl CrucibleDef {
    /// Flat stat modifier for the given stat index (0..7), summed across mods.
    /// Percentage mods are excluded (handled separately if ever present).
    pub fn flat_stat_mod(&self, index: usize) -> f32 {
        self.stat_mods
            .iter()
            .filter(|m| !m.percent && stat_index(m.stat) == index)
            .map(|m| m.value)
            .sum()
    }

    /// True if this definition carries any applicable bonus or restriction.
    pub fn has_content(&self) -> bool {
        !self.stat_mods.is_empty()
            || !self.effects.is_empty()
            || self.bxp_pct != 0.0
            || self.xp_pct != 0.0
            || self.loot_pct != 0.0
            || self.damage_mult != 1.0
    }
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

// --- Raw serde model -------------------------------------------------------

#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    array: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    crucible: Option<Body>,
}

#[derive(Deserialize)]
struct Body {
    #[serde(default)]
    restrictions: Vec<RawRestriction>,
    #[serde(default)]
    bonuses: Vec<RawBonus>,
}

#[derive(Deserialize)]
struct RawRestriction {
    #[serde(rename = "type")]
    kind: i32,
    #[serde(default)]
    stat: Option<String>,
    #[serde(default)]
    percent: Option<i32>,
    #[serde(default)]
    value: Option<f64>,
    #[serde(default)]
    effects: Option<String>,
    #[serde(rename = "effectChance", default)]
    effect_chance: Option<f64>,
    #[serde(default)]
    threshold: Option<f64>,
    #[serde(rename = "effectsDuration", default)]
    effects_duration: Option<f64>,
    #[serde(rename = "effectsCooldown", default)]
    effects_cooldown: Option<f64>,
}

#[derive(Deserialize)]
struct RawBonus {
    #[serde(rename = "type")]
    kind: i32,
    #[serde(default)]
    amount: Option<f64>,
    #[serde(default)]
    fame: Option<i32>,
}

// --- Public entry point ----------------------------------------------------

/// Parse every `crucible` definition found across the packet's JSON payloads.
/// Malformed payloads and non-crucible entries are skipped.
pub fn parse_crucible_defs(jsons: &[String]) -> Vec<CrucibleDef> {
    let mut out = Vec::new();
    for json in jsons {
        let env: Envelope = match serde_json::from_str(json) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in env.array {
            let Some(body) = entry.crucible else { continue };
            out.push(build_def(
                entry.id.unwrap_or_default(),
                entry.title.unwrap_or_default(),
                body,
            ));
        }
    }
    out
}

fn build_def(id: String, title: String, body: Body) -> CrucibleDef {
    let mut def = CrucibleDef {
        id,
        title,
        damage_mult: 1.0,
        ..Default::default()
    };

    for r in &body.restrictions {
        match r.kind {
            RESTRICTION_STAT_MOD => {
                if let (Some(stat_str), Some(value)) = (&r.stat, r.value) {
                    if let Some(stat) = StatKind::parse(stat_str) {
                        def.stat_mods.push(CrucibleStatMod {
                            stat,
                            value: value as f32,
                            percent: r.percent.unwrap_or(0) != 0,
                        });
                    }
                }
            }
            RESTRICTION_ON_HIT => {
                if let Some(name) = &r.effects {
                    def.effects.push(CrucibleEffect {
                        name: name.clone(),
                        chance: r.effect_chance.unwrap_or(0.0) as f32,
                        threshold: r.threshold.unwrap_or(0.0) as f32,
                        duration: r.effects_duration.unwrap_or(0.0) as f32,
                        cooldown: r.effects_cooldown.unwrap_or(0.0) as f32,
                    });
                }
            }
            _ => {}
        }
    }

    for b in &body.bonuses {
        let pct = b.amount.map(|a| ((a - 1.0) * 100.0) as f32);
        match b.kind {
            BONUS_BXP => def.bxp_pct = pct.unwrap_or(0.0),
            BONUS_XP => {
                def.xp_pct = pct.unwrap_or(0.0);
                def.xp_fame = b.fame.unwrap_or(0) != 0;
            }
            BONUS_LOOT => def.loot_pct = pct.unwrap_or(0.0),
            BONUS_DAMAGE => def.damage_mult = b.amount.unwrap_or(1.0) as f32,
            _ => {}
        }
    }

    def
}

#[cfg(test)]
mod tests {
    use super::*;

    const S30: &str = r#"{"array": [{"id": "5614074501562368", "title": "S30 Crucible", "desc": "Season 30 Crucible", "crucible": {"seasonalonly": null, "leavelimitation": null, "start": 1785834000, "end": 1791277200, "trials": [], "restrictions": [{"type": 2, "stat": "ATT", "percent": 0, "value": 5}, {"type": 2, "stat": "DEF", "percent": 0, "value": 10}, {"type": 5, "damageMult": 1, "damageOnEffect": 0, "effects": "Exposed", "durationType": 1, "effectsDuration": 3, "effectsCooldown": 4, "healthPercent": null, "effectChance": 0.3, "threshold": 15, "projectileOnEffect": 0}], "bonuses": [{"type": 1, "amount": 1.15, "percent": 1}, {"type": 2, "amount": 1.1, "percent": 1, "fame": 1}, {"type": 3, "percent": 1, "amount": 1.1}]}}]}"#;

    const EVENT: &str = r#"{"array": [{"id": "5192146276089856", "title": "Wind Vortex", "desc": "x", "item": "Wind Vortex Dungeon", "modifierEvent": {"start": 1, "end": 2, "dungeons": "", "modifiers": []}}]}"#;

    #[test]
    fn parses_s30_crucible() {
        let defs = parse_crucible_defs(&[S30.to_string()]);
        assert_eq!(defs.len(), 1);
        let d = &defs[0];
        assert_eq!(d.id, "5614074501562368");
        assert_eq!(d.title, "S30 Crucible");

        // Stat mods: +5 ATT, +10 DEF (flat).
        assert_eq!(d.flat_stat_mod(2), 5.0);
        assert_eq!(d.flat_stat_mod(3), 10.0);
        assert!(d.stat_mods.iter().all(|m| !m.percent));

        // On-hit Exposed.
        assert_eq!(d.effects.len(), 1);
        let e = &d.effects[0];
        assert_eq!(e.name, "Exposed");
        assert!((e.chance - 0.3).abs() < 1e-6);
        assert_eq!(e.threshold, 15.0);
        assert_eq!(e.duration, 3.0);
        assert_eq!(e.cooldown, 4.0);

        // Bonuses.
        assert!((d.bxp_pct - 15.0).abs() < 1e-3);
        assert!((d.xp_pct - 10.0).abs() < 1e-3);
        assert!(d.xp_fame);
        assert!((d.loot_pct - 10.0).abs() < 1e-3);
        assert_eq!(d.damage_mult, 1.0);
    }

    #[test]
    fn skips_event_modifier_entries() {
        let defs = parse_crucible_defs(&[EVENT.to_string()]);
        assert!(defs.is_empty());
    }

    #[test]
    fn skips_malformed_json() {
        let defs = parse_crucible_defs(&["not json".to_string(), "".to_string()]);
        assert!(defs.is_empty());
    }
}

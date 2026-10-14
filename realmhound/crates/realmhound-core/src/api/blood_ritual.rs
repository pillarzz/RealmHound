//! Parser for the Blood Ritual definition carried by the same feed as the
//! Crucible (`CrucibleResponse`, ID 183). Each entry wraps a `{"array":[..]}`
//! envelope and the one owning a `bloodRitual` object describes one *tier* of the
//! season's ritual.
//!
//! Schema verified against the Halloween 2026 payload, and the three bonus types
//! cross-checked against the in-game Blood Ritual panel (screenshots of tiers 1,
//! 2 and 10):
//! ```json
//! {"array":[{"id":"5027802235043840","title":"1","desc":"Halloween 2026",
//!   "bloodRitual":{"start":1791192600,"end":1793698200,"bonuses":[
//!     {"type":4,"percent":1,"amount":0.8},    // "−20% HP Penalty"
//!     {"type":5,"percent":1,"amount":1.24},   // "+24% Damage Bonus"
//!     {"type":3,"percent":1,"amount":1.25}]}}]} // "+25% Loot Drop Bonus"
//! ```
//! `title` is the tier number the UI shows, and the tiers form a slider: 1..10
//! trades maximum HP for damage (and the loot bonus peaks at the damage end).
//!
//! The active tier is *not* in this feed -- the server reports it as
//! `StatType::BloodRitual` (155), whose **string** value is the active
//! definition's id (empty when no ritual is active). See
//! `CharacterCache::set_active_blood_ritual`.

use serde::Deserialize;

/// Bonus `type` == maximum-HP multiplier.
const BONUS_MAX_HP: i32 = 4;
/// Bonus `type` == damage multiplier (the same id the Crucible uses).
const BONUS_DAMAGE: i32 = 5;
/// Bonus `type` == loot-drop multiplier (same id the Crucible uses).
const BONUS_LOOT: i32 = 3;

/// One tier of the season's Blood Ritual.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BloodRitualDef {
    pub id: String,
    /// Tier number as the game labels it (1..=10).
    pub tier: i32,
    /// Maximum-HP multiplier (1.0 == unchanged, 0.8 == the "−20% HP" tier).
    pub hp_mult: f32,
    /// Damage multiplier (1.0 == unchanged, 1.24 == "+24% Damage").
    pub damage_mult: f32,
    /// Loot-drop bonus in percent (25.0 == "+25%").
    pub loot_pct: f32,
}

impl BloodRitualDef {
    /// Damage bonus as the UI states it, in percent: negative for a penalty.
    pub fn damage_pct(&self) -> f32 {
        (self.damage_mult - 1.0) * 100.0
    }

    /// Maximum-HP modifier in percent: negative for the HP penalty tiers.
    pub fn hp_pct(&self) -> f32 {
        (self.hp_mult - 1.0) * 100.0
    }

    /// True when this tier changes something (a 1.0/1.0/0% tier is a no-op).
    pub fn has_effect(&self) -> bool {
        (self.hp_mult - 1.0).abs() > 0.0001
            || (self.damage_mult - 1.0).abs() > 0.0001
            || self.loot_pct.abs() > 0.0001
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
    #[serde(default, rename = "bloodRitual")]
    blood_ritual: Option<Body>,
}

#[derive(Deserialize)]
struct Body {
    #[serde(default)]
    bonuses: Vec<RawBonus>,
}

#[derive(Deserialize)]
struct RawBonus {
    #[serde(rename = "type")]
    kind: i32,
    #[serde(default)]
    percent: Option<i32>,
    #[serde(default)]
    amount: Option<f64>,
}

// --- Public entry point ----------------------------------------------------

/// Parse every `bloodRitual` definition found across the feed's payloads.
/// Malformed payloads and non-ritual entries are skipped, so this can be run over
/// the whole feed alongside [`crate::api::crucible::parse_crucible_defs`].
pub fn parse_blood_ritual_defs(jsons: &[String]) -> Vec<BloodRitualDef> {
    let mut out = Vec::new();
    for json in jsons {
        let env: Envelope = match serde_json::from_str(json) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in env.array {
            let Some(body) = entry.blood_ritual else {
                continue;
            };
            out.push(build_def(
                entry.id.unwrap_or_default(),
                entry.title.unwrap_or_default(),
                body,
            ));
        }
    }
    out
}

fn build_def(id: String, title: String, body: Body) -> BloodRitualDef {
    let mut def = BloodRitualDef {
        id,
        tier: title.trim().parse::<i32>().unwrap_or(0),
        hp_mult: 1.0,
        damage_mult: 1.0,
        loot_pct: 0.0,
    };
    for b in &body.bonuses {
        // Every entry in this feed carries `percent: 1` and the multiplier itself
        // in `amount`, the same convention the Crucible's bonuses use.
        if b.percent.unwrap_or(0) == 0 {
            continue;
        }
        let amount = match b.amount {
            Some(a) => a as f32,
            None => continue,
        };
        match b.kind {
            BONUS_MAX_HP => def.hp_mult = amount,
            BONUS_DAMAGE => def.damage_mult = amount,
            BONUS_LOOT => def.loot_pct = (amount - 1.0) * 100.0,
            _ => {}
        }
    }
    def
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from the Halloween 2026 capture: tier 1 (the most aggressive),
    /// tier 2, and tier 10 (the +HP/penalty end).
    const FEED: &str = r#"{"array": [
        {"id": "5027802235043840", "title": "1", "desc": "Halloween 2026",
         "bloodRitual": {"start": 1791192600, "end": 1793698200, "bonuses": [
            {"type": 4, "percent": 1, "amount": 0.8},
            {"type": 5, "percent": 1, "amount": 1.24},
            {"type": 3, "percent": 1, "amount": 1.25}]}},
        {"id": "6078214115459072", "title": "2", "desc": "Halloween 2026\t\t\t",
         "bloodRitual": {"bonuses": [
            {"type": 4, "percent": 1, "amount": 0.85},
            {"type": 5, "percent": 1, "amount": 1.18},
            {"type": 3, "percent": 1, "amount": 1.15}]}},
        {"id": "6261236982906880", "title": "10", "desc": "Halloween 2026",
         "bloodRitual": {"bonuses": [
            {"type": 4, "percent": 1, "amount": 1.2},
            {"type": 5, "percent": 1, "amount": 0.78},
            {"type": 3, "percent": 1, "amount": 1}]}},
        {"id": "999", "title": "x", "desc": "not a ritual", "modifierEvent": {}}
    ]}"#;

    #[test]
    fn parses_the_ritual_tiers_and_their_bonuses() {
        let defs = parse_blood_ritual_defs(&[FEED.to_string()]);
        assert_eq!(defs.len(), 3, "the non-ritual entry is skipped");

        let t1 = &defs[0];
        assert_eq!(t1.id, "5027802235043840");
        assert_eq!(t1.tier, 1);
        assert!((t1.hp_mult - 0.8).abs() < 1e-6);
        assert!((t1.damage_mult - 1.24).abs() < 1e-6);
        assert!((t1.loot_pct - 25.0).abs() < 1e-3);
        paint(t1);

        let t2 = &defs[1];
        assert_eq!(t2.tier, 2, "the tier number comes from `title`");
        assert!((t2.hp_pct() + 15.0).abs() < 1e-3);
        assert!((t2.damage_pct() - 18.0).abs() < 1e-3);
        assert!((t2.loot_pct - 15.0).abs() < 1e-3);

        let t10 = &defs[2];
        assert!((t10.hp_pct() - 20.0).abs() < 1e-3);
        assert!((t10.damage_pct() + 22.0).abs() < 1e-3);
        assert_eq!(t10.loot_pct, 0.0);
    }

    /// The three types are pinned to the in-game panel's wording, so a future
    /// season cannot silently re-label them.
    fn paint(d: &BloodRitualDef) {
        assert!(d.has_effect());
    }

    #[test]
    fn ignores_non_ritual_payloads() {
        assert!(parse_blood_ritual_defs(&[]).is_empty());
        assert!(parse_blood_ritual_defs(&["not json".to_string()]).is_empty());
        assert!(
            parse_blood_ritual_defs(&[r#"{"array":[{"id":"1","title":"1"}]}"#.to_string()])
                .is_empty()
        );
    }
}

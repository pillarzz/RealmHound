//! Shared stat-bonus primitives parsed from `equip.xml` (`<ActivateOnEquip>`)
//! and `enchantments.xml` (`<Mutators>`). Both sources express flat stat
//! increments with the same `stat="ATT"` naming, so the mapping lives here and
//! is reused by the object and enchantment parsers.

use serde::{Deserialize, Serialize};

/// One of the eight character stats an item or enchant can modify.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StatKind {
    MaxHp,
    MaxMp,
    Attack,
    Defense,
    Speed,
    Dexterity,
    Vitality,
    Wisdom,
}

impl StatKind {
    /// Parse a `stat=".."` attribute value (e.g. `ATT`, `MAXHP`, `HP`).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "MAXHP" | "HP" | "MAXHITPOINTS" => Some(Self::MaxHp),
            "MAXMP" | "MP" | "MAXMAGICPOINTS" => Some(Self::MaxMp),
            "ATT" | "ATTACK" => Some(Self::Attack),
            "DEF" | "DEFENSE" => Some(Self::Defense),
            "SPD" | "SPEED" => Some(Self::Speed),
            "DEX" | "DEXTERITY" => Some(Self::Dexterity),
            "VIT" | "VITALITY" => Some(Self::Vitality),
            "WIS" | "WISDOM" => Some(Self::Wisdom),
            _ => None,
        }
    }
}

/// Flat additive bonuses to each of the eight stats. Values are `f32` because
/// enchantments carry fractional increments (e.g. Attack Bonus I = `+1.4`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StatBonuses {
    pub max_hp: f32,
    pub max_mp: f32,
    pub attack: f32,
    pub defense: f32,
    pub speed: f32,
    pub dexterity: f32,
    pub vitality: f32,
    pub wisdom: f32,
}

impl StatBonuses {
    /// Whether every bonus is zero.
    pub fn is_empty(&self) -> bool {
        *self == StatBonuses::default()
    }

    /// Add `amount` to the given stat.
    pub fn add(&mut self, stat: StatKind, amount: f32) {
        match stat {
            StatKind::MaxHp => self.max_hp += amount,
            StatKind::MaxMp => self.max_mp += amount,
            StatKind::Attack => self.attack += amount,
            StatKind::Defense => self.defense += amount,
            StatKind::Speed => self.speed += amount,
            StatKind::Dexterity => self.dexterity += amount,
            StatKind::Vitality => self.vitality += amount,
            StatKind::Wisdom => self.wisdom += amount,
        }
    }

    /// Read the current value for a stat.
    pub fn get(&self, stat: StatKind) -> f32 {
        match stat {
            StatKind::MaxHp => self.max_hp,
            StatKind::MaxMp => self.max_mp,
            StatKind::Attack => self.attack,
            StatKind::Defense => self.defense,
            StatKind::Speed => self.speed,
            StatKind::Dexterity => self.dexterity,
            StatKind::Vitality => self.vitality,
            StatKind::Wisdom => self.wisdom,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stat_names() {
        assert_eq!(StatKind::parse("ATT"), Some(StatKind::Attack));
        assert_eq!(StatKind::parse("maxhp"), Some(StatKind::MaxHp));
        assert_eq!(StatKind::parse("HP"), Some(StatKind::MaxHp));
        assert_eq!(StatKind::parse("MP"), Some(StatKind::MaxMp));
        assert_eq!(StatKind::parse("SUMMONPOWER"), None);
    }

    #[test]
    fn accumulates_bonuses() {
        let mut b = StatBonuses::default();
        assert!(b.is_empty());
        b.add(StatKind::Attack, 1.4);
        b.add(StatKind::Attack, 2.2);
        assert!((b.get(StatKind::Attack) - 3.6).abs() < 1e-6);
        assert!(!b.is_empty());
    }
}

//! Enchantment definitions parser.
//!
//! Parses enchantments.xml to provide enchant ID to name mapping.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::Reader;

use super::stat_bonus::StatKind;

/// The kind of a single enchantment `ActivateOnEquip` mutator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnchantEffectKind {
    /// Flat stat increment (`amount` added to `stat`).
    IncrementStat,
    /// Bonus-relative stat: adds `amount`% of the *bonus* portion of
    /// `stat_relative_to` to `stat`.
    BonusStatRelative,
    /// Loot-drop rate bonus, in percent (`amount`).
    LootBonus,
    /// XP gain bonus, in percent.
    XpBonus,
    /// Dust bonus, in percent.
    DustBonus,
    /// Flat regen boost: `amount` regen units added to the resource named by
    /// `stat` (`MaxHp` = HP regen, `MaxMp` = MP regen).
    FlatRegen,
    /// Percentage regen boost: `amount` (a fraction, e.g. `0.01`) of the
    /// resource's max value added to its regen, per the resource in `stat`.
    PercentageRegen,
    /// Multiplies the weapon's minimum damage by `amount` (e.g. `1.05`).
    MultiplyMinDamage,
    /// Multiplies the weapon's maximum damage by `amount` (e.g. `1.05`).
    MultiplyMaxDamage,
    /// A recognized but purely cosmetic/proc mutator (e.g. on-shoot self buffs)
    /// that grants no persistent stat effect. Modeled so its presence does not
    /// downgrade provenance to Partial.
    Cosmetic,
    /// Any other mutator kind we don't model.
    Other,
}

impl EnchantEffectKind {
    fn parse(s: &str) -> Self {
        match s.trim() {
            "IncrementStat" => Self::IncrementStat,
            "BonusStatRelative" => Self::BonusStatRelative,
            "LootBonus" => Self::LootBonus,
            "XPBonus" => Self::XpBonus,
            "DustBonus" => Self::DustBonus,
            "FlatRegen" => Self::FlatRegen,
            "PercentageRegen" => Self::PercentageRegen,
            // On-shoot / proc self-buffs (e.g. OnShoot Wisdom Boost): recognized
            // but not a persistent stat, so treated as cosmetic.
            "StatBoostSelf" => Self::Cosmetic,
            _ => Self::Other,
        }
    }
}

/// One parsed `<ActivateOnEquip>` mutator from an enchantment's `<Mutators>`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnchantEffect {
    pub kind: EnchantEffectKind,
    pub stat: Option<StatKind>,
    pub amount: f32,
    pub stat_relative_to: Option<StatKind>,
    /// For damage-multiplier mutators, the projectile index the multiplier
    /// applies to (the `projectileId` attribute). `-1` means all projectiles.
    /// Non-damage mutators leave this at `-1`.
    pub projectile_id: i32,
}

/// Enchantment tier/rarity classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnchantmentTier {
    /// Normal enchantments (mixed case names like "Attack Bonus I")
    Normal,
    /// Unique enchantments (ALL CAPS names or contains "Unique")
    Unique,
    /// Awakened enchantments (contains "Awakened" in name)
    Awakened,
    /// High-tier Loot Bonus enchantments (III or IV only)
    HighLootBonus,
}

impl EnchantmentTier {
    /// Determine the tier from an enchantment display name.
    ///
    /// Note: This only checks the display name. For more accurate detection,
    /// use `from_enchant()` which also checks the internal ID.
    pub fn from_name(name: &str) -> Self {
        // Check for high-tier Loot Bonus first (III or IV)
        // Must be "Loot Bonus III/IV" but NOT "XP Loot Bonus III/IV"
        if Self::is_high_loot_bonus_name(name) {
            return Self::HighLootBonus;
        }
        // Check for Awakened (highest tier)
        if name.contains("Awakened") || name.contains("AWAKENED") {
            return Self::Awakened;
        }
        // Check for Unique (all caps or contains "Unique")
        if name.contains("Unique") || name.contains("UNIQUE") || Self::is_all_caps(name) {
            return Self::Unique;
        }
        Self::Normal
    }

    /// Check if a name matches high-tier Loot Bonus (III or IV).
    /// Must match "Loot Bonus III/IV" but NOT "XP Loot Bonus III/IV".
    fn is_high_loot_bonus_name(name: &str) -> bool {
        (name.contains("Loot Bonus III") || name.contains("Loot Bonus IV"))
            && !name.contains("XP Loot Bonus")
    }

    /// Determine the tier from both internal ID and display name.
    ///
    /// This is more accurate as unique enchants have ALL_CAPS internal IDs
    /// (e.g., "STHENOS_SWIFTNESS") even if display name is mixed case
    /// (e.g., "Stheno's Swiftness").
    pub fn from_enchant(internal_id: &str, display_name: &str) -> Self {
        // Check for high-tier Loot Bonus first (III or IV)
        // Must be "Loot Bonus III/IV" but NOT "XP Loot Bonus III/IV"
        if Self::is_high_loot_bonus_name(display_name) {
            return Self::HighLootBonus;
        }
        // Check for Awakened (highest tier)
        if display_name.contains("Awakened") || display_name.contains("AWAKENED") {
            return Self::Awakened;
        }
        // Check for Unique:
        // - Internal ID is ALL_CAPS (like "STHENOS_SWIFTNESS")
        // - Or display name contains "Unique"
        // - Or display name is all caps
        if Self::is_all_caps_with_underscore(internal_id)
            || display_name.contains("Unique")
            || display_name.contains("UNIQUE")
            || Self::is_all_caps(display_name)
        {
            return Self::Unique;
        }
        Self::Normal
    }

    /// Check if a string is all uppercase letters (ignoring non-letters).
    fn is_all_caps(s: &str) -> bool {
        let has_letters = s.chars().any(|c| c.is_alphabetic());
        has_letters
            && s.chars()
                .filter(|c| c.is_alphabetic())
                .all(|c| c.is_uppercase())
    }

    /// Check if a string is all uppercase with underscores (internal ID format).
    /// e.g., "STHENOS_SWIFTNESS", "LOOT_BONUS_3"
    fn is_all_caps_with_underscore(s: &str) -> bool {
        let has_letters = s.chars().any(|c| c.is_alphabetic());
        has_letters
            && s.chars()
                .all(|c| c.is_uppercase() || c == '_' || c.is_numeric())
    }

    /// Check if this is a special tier (Unique or Awakened).
    pub fn is_special(&self) -> bool {
        matches!(self, Self::Unique | Self::Awakened)
    }

    /// Check if this is a high-tier Loot Bonus (III or IV).
    pub fn is_high_loot_bonus(&self) -> bool {
        matches!(self, Self::HighLootBonus)
    }

    /// Check if this is a "valuable" tier worth tracking (Unique, Awakened, or HighLootBonus).
    pub fn is_valuable(&self) -> bool {
        matches!(self, Self::Unique | Self::Awakened | Self::HighLootBonus)
    }
}

/// A single enchantment definition.
#[derive(Debug, Clone)]
pub struct EnchantmentDef {
    /// Unique enchantment type ID (hex in XML, stored as u16)
    pub type_id: u16,
    /// Internal ID string (e.g., "Attack_Bonus_1")
    pub id: String,
    /// Display name (e.g., "Attack Bonus I")
    pub display_name: String,
    /// Description text
    pub description: String,
    /// Texture spritesheet name (e.g., "enchantments16x16")
    pub texture_file: Option<String>,
    /// Texture index in the spritesheet
    pub texture_index: Option<i32>,
    /// Tier number (1-4) parsed from the `TIERn` label, if present.
    pub tier_num: Option<u8>,
    /// Whether the enchantment carries the `ROLLABLE` label, meaning it can
    /// appear naturally on dropped/enchanted equipment. Non-rollable uniques
    /// are engraving-only (seasonal event) enchants.
    pub rollable: bool,
    /// Parsed `<Mutators>` effects (stat/loot/xp/dust bonuses).
    pub effects: Vec<EnchantEffect>,
}

/// Uppercase family key for an enchantment internal id: the id with any
/// trailing tier suffix (`_1`..`_4`) removed, uppercased. Enchantments that
/// share a family (e.g. `Loot_Bonus_1`..`Loot_Bonus_4`) map to the same key
/// (`LOOT_BONUS`). This is the stable identity used to match tiered
/// enchantment notifications, independent of display-name renames.
pub fn family_id_from_internal(internal_id: &str, tier_num: Option<u8>) -> String {
    if tier_num.is_some() {
        if let Some((base, suffix)) = internal_id.rsplit_once('_') {
            if suffix.chars().all(|c| c.is_ascii_digit()) && !suffix.is_empty() {
                return base.to_uppercase();
            }
        }
    }
    internal_id.to_uppercase()
}

/// Extract the tier number (1-4) from a `TIERn` label inside an
/// `EnchantmentLabels` comma-separated string.
fn tier_from_labels(labels: &str) -> Option<u8> {
    labels.split(',').find_map(|label| {
        let label = label.trim();
        label
            .strip_prefix("TIER")
            .and_then(|n| n.parse::<u8>().ok())
    })
}

/// A lightweight, owned snapshot of one enchantment, used to enumerate the
/// enchantment list without holding the asset-manager lock.
#[derive(Debug, Clone)]
pub struct EnchantCatalogEntry {
    pub type_id: u16,
    pub display_name: String,
    pub internal_id: String,
    pub family_id: String,
    pub tier_num: Option<u8>,
    pub tier_class: EnchantmentTier,
    pub rollable: bool,
}

/// Collection of all enchantment definitions.
#[derive(Debug, Default)]
pub struct EnchantmentList {
    /// Map from type ID to enchantment definition
    by_id: HashMap<u16, EnchantmentDef>,
}

impl EnchantmentList {
    /// Create an empty enchantment list.
    pub fn new() -> Self {
        Self {
            by_id: HashMap::new(),
        }
    }

    /// Load enchantments from an XML file.
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let file = File::open(path.as_ref())
            .map_err(|e| format!("Failed to open enchantments.xml: {}", e))?;
        let reader = BufReader::new(file);
        Self::parse_xml(reader)
    }

    /// Parse enchantments from XML content.
    fn parse_xml<R: std::io::BufRead>(reader: R) -> Result<Self, String> {
        let mut xml_reader = Reader::from_reader(reader);
        xml_reader.config_mut().trim_text(true);

        let mut list = Self::new();
        let mut buf = Vec::new();

        // Current enchantment being parsed
        let mut current: Option<EnchantmentDef> = None;
        let mut in_display_id = false;
        let mut in_description = false;
        let mut in_texture = false;
        let mut in_texture_file = false;
        let mut in_texture_index = false;
        let mut in_labels = false;
        // Inside a <Mutators> block; ActivateOnEquip children are effect defs.
        let mut in_mutators = false;
        let mut in_mutator_effect = false;
        // Set to the damage-multiplier kind while inside a <MultiplyMinDamage> /
        // <MultiplyMaxDamage> tag, whose text content is the multiplier.
        let mut in_mult_damage: Option<EnchantEffectKind> = None;
        // `projectileId` attribute of the current damage-multiplier tag; -1 = all.
        let mut pending_mult_projectile: i32 = -1;
        let mut pending_effect_stat: Option<StatKind> = None;
        let mut pending_effect_amount: f32 = 0.0;
        let mut pending_effect_rel: Option<StatKind> = None;

        loop {
            match xml_reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    match e.name().as_ref() {
                        b"Enchantment" => {
                            // Parse attributes
                            let mut enchant = EnchantmentDef {
                                type_id: 0,
                                id: String::new(),
                                display_name: String::new(),
                                description: String::new(),
                                texture_file: None,
                                texture_index: None,
                                tier_num: None,
                                rollable: false,
                                effects: Vec::new(),
                            };

                            for attr in e.attributes().flatten() {
                                match attr.key.as_ref() {
                                    b"id" => {
                                        enchant.id =
                                            String::from_utf8_lossy(&attr.value).to_string();
                                    }
                                    b"type" => {
                                        let type_str = String::from_utf8_lossy(&attr.value);
                                        // Parse hex type (e.g., "0x107" -> 263)
                                        enchant.type_id = parse_hex_type(&type_str);
                                    }
                                    _ => {}
                                }
                            }

                            current = Some(enchant);
                        }
                        b"DisplayId" => {
                            in_display_id = true;
                        }
                        b"Description" => {
                            in_description = true;
                        }
                        b"Texture" => {
                            in_texture = true;
                        }
                        b"File" if in_texture => {
                            in_texture_file = true;
                        }
                        b"Index" if in_texture => {
                            in_texture_index = true;
                        }
                        b"EnchantmentLabels" => {
                            in_labels = true;
                        }
                        b"Mutators" => {
                            in_mutators = true;
                        }
                        b"ActivateOnEquip" if in_mutators => {
                            pending_effect_stat = None;
                            pending_effect_amount = 0.0;
                            pending_effect_rel = None;
                            for attr in e.attributes().flatten() {
                                let v = String::from_utf8_lossy(&attr.value);
                                match attr.key.as_ref() {
                                    b"stat" => pending_effect_stat = StatKind::parse(&v),
                                    b"amount" => pending_effect_amount = v.parse().unwrap_or(0.0),
                                    b"statRelativeTo" => pending_effect_rel = StatKind::parse(&v),
                                    _ => {}
                                }
                            }
                            in_mutator_effect = true;
                        }
                        // On-shoot / proc self-buff mutator: its text content is
                        // the effect kind (e.g. "StatBoostSelf"); parsed so its
                        // presence is recognized rather than treated as unknown.
                        b"AddOnPlayerShootActivate" if in_mutators => {
                            pending_effect_stat = None;
                            pending_effect_amount = 0.0;
                            pending_effect_rel = None;
                            for attr in e.attributes().flatten() {
                                let v = String::from_utf8_lossy(&attr.value);
                                match attr.key.as_ref() {
                                    b"stat" => pending_effect_stat = StatKind::parse(&v),
                                    b"amount" => pending_effect_amount = v.parse().unwrap_or(0.0),
                                    _ => {}
                                }
                            }
                            in_mutator_effect = true;
                        }
                        // Damage-multiplier mutators: the tag name is the kind and
                        // the text content is the multiplier (e.g. 1.05).
                        b"MultiplyMinDamage" if in_mutators => {
                            in_mult_damage = Some(EnchantEffectKind::MultiplyMinDamage);
                            pending_mult_projectile = -1;
                            for attr in e.attributes().flatten() {
                                if attr.key.as_ref() == b"projectileId" {
                                    pending_mult_projectile = String::from_utf8_lossy(&attr.value)
                                        .trim()
                                        .parse()
                                        .unwrap_or(-1);
                                }
                            }
                        }
                        b"MultiplyMaxDamage" if in_mutators => {
                            in_mult_damage = Some(EnchantEffectKind::MultiplyMaxDamage);
                            pending_mult_projectile = -1;
                            for attr in e.attributes().flatten() {
                                if attr.key.as_ref() == b"projectileId" {
                                    pending_mult_projectile = String::from_utf8_lossy(&attr.value)
                                        .trim()
                                        .parse()
                                        .unwrap_or(-1);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::End(ref e)) => match e.name().as_ref() {
                    b"Enchantment" => {
                        if let Some(enchant) = current.take() {
                            if enchant.type_id > 0 {
                                list.by_id.insert(enchant.type_id, enchant);
                            }
                        }
                    }
                    b"DisplayId" => {
                        in_display_id = false;
                    }
                    b"Description" => {
                        in_description = false;
                    }
                    b"Texture" => {
                        in_texture = false;
                    }
                    b"File" => {
                        in_texture_file = false;
                    }
                    b"Index" => {
                        in_texture_index = false;
                    }
                    b"EnchantmentLabels" => {
                        in_labels = false;
                    }
                    b"Mutators" => {
                        in_mutators = false;
                    }
                    b"ActivateOnEquip" => {
                        in_mutator_effect = false;
                    }
                    b"AddOnPlayerShootActivate" => {
                        in_mutator_effect = false;
                    }
                    b"MultiplyMinDamage" | b"MultiplyMaxDamage" => {
                        in_mult_damage = None;
                    }
                    _ => {}
                },
                Ok(Event::Text(ref e)) => {
                    if let Some(ref mut enchant) = current {
                        let text = e.unescape().unwrap_or_default().to_string();
                        if in_mutator_effect {
                            enchant.effects.push(EnchantEffect {
                                kind: EnchantEffectKind::parse(&text),
                                stat: pending_effect_stat,
                                amount: pending_effect_amount,
                                stat_relative_to: pending_effect_rel,
                                projectile_id: -1,
                            });
                        } else if let Some(kind) = in_mult_damage {
                            enchant.effects.push(EnchantEffect {
                                kind,
                                stat: None,
                                amount: text.trim().parse().unwrap_or(1.0),
                                stat_relative_to: None,
                                projectile_id: pending_mult_projectile,
                            });
                        } else if in_display_id {
                            enchant.display_name = text;
                        } else if in_description {
                            enchant.description = text;
                        } else if in_texture_file {
                            enchant.texture_file = Some(text);
                        } else if in_texture_index {
                            if let Ok(idx) = text.parse::<i32>() {
                                enchant.texture_index = Some(idx);
                            }
                        } else if in_labels {
                            enchant.tier_num = tier_from_labels(&text);
                            enchant.rollable = text.split(',').any(|l| l.trim() == "ROLLABLE");
                        }
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

        Ok(list)
    }

    /// Get the number of loaded enchantments.
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Check if the list is empty.
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Get an enchantment by type ID.
    pub fn get(&self, type_id: u16) -> Option<&EnchantmentDef> {
        self.by_id.get(&type_id)
    }

    /// Parsed `<Mutators>` effects for an enchantment, or empty if unknown.
    pub fn effects(&self, type_id: u16) -> &[EnchantEffect] {
        self.by_id
            .get(&type_id)
            .map(|e| e.effects.as_slice())
            .unwrap_or(&[])
    }

    /// Iterate over all enchantment definitions (unordered).
    pub fn iter(&self) -> impl Iterator<Item = &EnchantmentDef> {
        self.by_id.values()
    }

    /// Tier number (1-4) for an enchantment, from its `TIERn` label.
    pub fn tier_num(&self, type_id: u16) -> Option<u8> {
        self.by_id.get(&type_id).and_then(|e| e.tier_num)
    }

    /// Stable uppercase family key for an enchantment (e.g. `LOOT_BONUS`),
    /// with the tier suffix stripped when the enchantment carries a tier.
    pub fn family_id(&self, type_id: u16) -> Option<String> {
        self.by_id
            .get(&type_id)
            .map(|e| family_id_from_internal(&e.id, e.tier_num))
    }

    /// The internal id string (e.g. `FLURRY_OF_BLOWS`) for an enchantment.
    pub fn internal_id(&self, type_id: u16) -> Option<&str> {
        self.by_id.get(&type_id).map(|e| e.id.as_str())
    }

    /// Owned snapshot of every enchantment definition, for enumeration outside
    /// the asset-manager lock (e.g. the settings picker and sound matching).
    pub fn catalog(&self) -> Vec<EnchantCatalogEntry> {
        self.by_id
            .values()
            .map(|e| EnchantCatalogEntry {
                type_id: e.type_id,
                display_name: e.display_name.clone(),
                internal_id: e.id.clone(),
                family_id: family_id_from_internal(&e.id, e.tier_num),
                tier_num: e.tier_num,
                tier_class: EnchantmentTier::from_enchant(&e.id, &e.display_name),
                rollable: e.rollable,
            })
            .collect()
    }

    /// Get the display name for an enchantment type ID.
    pub fn name(&self, type_id: u16) -> Option<&str> {
        self.by_id.get(&type_id).map(|e| e.display_name.as_str())
    }

    /// Get the description for an enchantment type ID.
    pub fn description(&self, type_id: u16) -> Option<&str> {
        self.by_id.get(&type_id).map(|e| e.description.as_str())
    }

    /// Get the tier of an enchantment by type ID.
    ///
    /// Uses both internal ID (for ALL_CAPS unique detection) and display name.
    pub fn tier(&self, type_id: u16) -> Option<EnchantmentTier> {
        self.by_id
            .get(&type_id)
            .map(|e| EnchantmentTier::from_enchant(&e.id, &e.display_name))
    }

    /// Check if an enchantment is a special tier (Unique or Awakened).
    pub fn is_special(&self, type_id: u16) -> bool {
        self.tier(type_id).map(|t| t.is_special()).unwrap_or(false)
    }

    /// Check if an enchantment is Unique tier.
    pub fn is_unique(&self, type_id: u16) -> bool {
        self.tier(type_id) == Some(EnchantmentTier::Unique)
    }

    /// Check if an enchantment is Awakened tier.
    pub fn is_awakened(&self, type_id: u16) -> bool {
        self.tier(type_id) == Some(EnchantmentTier::Awakened)
    }

    /// Check if an enchantment is Loot Bonus III or IV (high-tier loot bonus).
    pub fn is_high_loot_bonus(&self, type_id: u16) -> bool {
        self.tier(type_id) == Some(EnchantmentTier::HighLootBonus)
    }

    /// Check if an enchantment is "valuable" (Unique, Awakened, or HighLootBonus).
    pub fn is_valuable(&self, type_id: u16) -> bool {
        self.tier(type_id).map(|t| t.is_valuable()).unwrap_or(false)
    }

    /// Check if any of the given enchant IDs are valuable (Unique, Awakened, or HighLootBonus).
    pub fn has_any_valuable(&self, enchant_ids: &[u16]) -> bool {
        enchant_ids.iter().any(|&id| self.is_valuable(id))
    }

    /// Get the texture info (spritesheet name, index) for an enchantment.
    pub fn texture(&self, type_id: u16) -> Option<(&str, i32)> {
        self.by_id
            .get(&type_id)
            .and_then(|e| match (&e.texture_file, e.texture_index) {
                (Some(file), Some(index)) => Some((file.as_str(), index)),
                _ => None,
            })
    }
}

/// Parse a hex type string like "0x107" to u16.
fn parse_hex_type(s: &str) -> u16 {
    let s = s.trim();
    if s.starts_with("0x") || s.starts_with("0X") {
        u16::from_str_radix(&s[2..], 16).unwrap_or(0)
    } else {
        s.parse().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_xml_captures_new_mutator_kinds() {
        let xml = r#"<Enchantments>
  <Enchantment id="Damage_Bonus_4" type="0x152">
    <DisplayId>Damage Bonus IV</DisplayId>
    <Mutators>
      <MultiplyMinDamage projectileId="-1">1.05</MultiplyMinDamage>
      <MultiplyMaxDamage projectileId="-1">1.05</MultiplyMaxDamage>
    </Mutators>
  </Enchantment>
  <Enchantment id="Flat_Life_Regen_4" type="0x17E">
    <DisplayId>Flat Life Regeneration IV</DisplayId>
    <Mutators>
      <ActivateOnEquip amount="10" stat="HP">FlatRegen</ActivateOnEquip>
    </Mutators>
  </Enchantment>
  <Enchantment id="Pct_Mana_Regen_4" type="0x607">
    <DisplayId>Percentage Mana Regeneration IV</DisplayId>
    <Mutators>
      <ActivateOnEquip amount="0.015" stat="MP">PercentageRegen</ActivateOnEquip>
    </Mutators>
  </Enchantment>
  <Enchantment id="OnShoot_Wis" type="0x4D">
    <DisplayId>OnShoot Wisdom Boost I</DisplayId>
    <Mutators>
      <AddOnPlayerShootActivate amount="7" stat="WIS">StatBoostSelf</AddOnPlayerShootActivate>
    </Mutators>
  </Enchantment>
</Enchantments>"#;
        let list = EnchantmentList::parse_xml(xml.as_bytes()).unwrap();

        let dmg = list.effects(0x152);
        assert_eq!(dmg.len(), 2);
        assert_eq!(dmg[0].kind, EnchantEffectKind::MultiplyMinDamage);
        assert!((dmg[0].amount - 1.05).abs() < 1e-4);
        assert_eq!(dmg[0].projectile_id, -1);
        assert_eq!(dmg[1].kind, EnchantEffectKind::MultiplyMaxDamage);
        assert_eq!(dmg[1].projectile_id, -1);

        let flat = list.effects(0x17E);
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].kind, EnchantEffectKind::FlatRegen);
        assert_eq!(flat[0].stat, Some(StatKind::MaxHp));
        assert!((flat[0].amount - 10.0).abs() < 1e-4);

        let pct = list.effects(0x607);
        assert_eq!(pct.len(), 1);
        assert_eq!(pct[0].kind, EnchantEffectKind::PercentageRegen);
        assert_eq!(pct[0].stat, Some(StatKind::MaxMp));
        assert!((pct[0].amount - 0.015).abs() < 1e-4);

        // On-shoot self buff is recognized as cosmetic (present, no downgrade).
        let cos = list.effects(0x4D);
        assert_eq!(cos.len(), 1);
        assert_eq!(cos[0].kind, EnchantEffectKind::Cosmetic);
    }

    #[test]
    fn test_parse_hex_type() {
        assert_eq!(parse_hex_type("0x107"), 263);
        assert_eq!(parse_hex_type("0x117"), 279);
        assert_eq!(parse_hex_type("0x8b"), 139);
        assert_eq!(parse_hex_type("0x8B"), 139);
        assert_eq!(parse_hex_type("123"), 123);
    }

    #[test]
    fn test_tier_from_labels() {
        assert_eq!(
            tier_from_labels("REWARD,REWARDBONUS,LOOT,ROLLABLE,TIER1"),
            Some(1)
        );
        assert_eq!(tier_from_labels("TIER3,LOOT"), Some(3));
        assert_eq!(tier_from_labels("REWARD,LOOT"), None);
    }

    #[test]
    fn test_family_id_from_internal() {
        // Tiered enchants strip the trailing tier suffix.
        assert_eq!(
            family_id_from_internal("Loot_Bonus_3", Some(3)),
            "LOOT_BONUS"
        );
        assert_eq!(
            family_id_from_internal("Loot_Bonus_4", Some(4)),
            "LOOT_BONUS"
        );
        assert_eq!(
            family_id_from_internal("XP_Loot_Bonus_3", Some(3)),
            "XP_LOOT_BONUS"
        );
        // Without a tier the whole id is used (uniques keep their identity).
        assert_eq!(
            family_id_from_internal("FLURRY_OF_BLOWS", None),
            "FLURRY_OF_BLOWS"
        );
        // A trailing digit is only stripped when the enchant actually has a tier.
        assert_eq!(
            family_id_from_internal("Some_Enchant_2", None),
            "SOME_ENCHANT_2"
        );
    }

    #[test]
    fn test_parse_xml_captures_tier_and_family() {
        let xml = r#"<Enchantments>
  <Enchantment id="Loot_Bonus_3" type="0x5B2">
    <DisplayId>Loot Bonus III</DisplayId>
    <EnchantmentLabels>REWARD,LOOT,TIER3</EnchantmentLabels>
  </Enchantment>
  <Enchantment id="FLURRY_OF_BLOWS" type="0x631">
    <DisplayId>Flurry of Blows</DisplayId>
    <EnchantmentLabels>ABILITY</EnchantmentLabels>
  </Enchantment>
</Enchantments>"#;
        let list = EnchantmentList::parse_xml(xml.as_bytes()).unwrap();
        assert_eq!(list.tier_num(0x5B2), Some(3));
        assert_eq!(list.family_id(0x5B2).as_deref(), Some("LOOT_BONUS"));
        assert_eq!(list.tier_num(0x631), None);
        assert_eq!(list.family_id(0x631).as_deref(), Some("FLURRY_OF_BLOWS"));
    }

    #[test]
    fn test_enchantment_tier_from_name() {
        // Normal enchants (mixed case, including low-tier loot bonus)
        assert_eq!(
            EnchantmentTier::from_name("Attack Bonus I"),
            EnchantmentTier::Normal
        );
        assert_eq!(
            EnchantmentTier::from_name("Mana Regen"),
            EnchantmentTier::Normal
        );
        assert_eq!(
            EnchantmentTier::from_name("Defense Bonus III"),
            EnchantmentTier::Normal
        );
        assert_eq!(
            EnchantmentTier::from_name("Loot Bonus I"),
            EnchantmentTier::Normal
        );
        assert_eq!(
            EnchantmentTier::from_name("Loot Bonus II"),
            EnchantmentTier::Normal
        );
        // XP Loot Bonus should NOT be counted as valuable
        assert_eq!(
            EnchantmentTier::from_name("XP Loot Bonus III"),
            EnchantmentTier::Normal
        );
        assert_eq!(
            EnchantmentTier::from_name("XP Loot Bonus IV"),
            EnchantmentTier::Normal
        );

        // Unique enchants (all caps or contains "Unique")
        assert_eq!(
            EnchantmentTier::from_name("LEGENDARY STRIKE"),
            EnchantmentTier::Unique
        );
        assert_eq!(
            EnchantmentTier::from_name("SUPER RARE"),
            EnchantmentTier::Unique
        );
        assert_eq!(
            EnchantmentTier::from_name("Unique Power"),
            EnchantmentTier::Unique
        );

        // Awakened enchants (contains "Awakened")
        assert_eq!(
            EnchantmentTier::from_name("Awakened Strike"),
            EnchantmentTier::Awakened
        );
        assert_eq!(
            EnchantmentTier::from_name("AWAKENED POWER"),
            EnchantmentTier::Awakened
        );

        // High-tier Loot Bonus enchants (III and IV only, NOT XP Loot Bonus)
        assert_eq!(
            EnchantmentTier::from_name("Loot Bonus III"),
            EnchantmentTier::HighLootBonus
        );
        assert_eq!(
            EnchantmentTier::from_name("Loot Bonus IV"),
            EnchantmentTier::HighLootBonus
        );
    }

    #[test]
    fn test_enchantment_tier_from_enchant_named_uniques() {
        // Named uniques have ALL_CAPS internal IDs but mixed case display names
        // These are the ones that from_name() alone would miss!
        assert_eq!(
            EnchantmentTier::from_enchant("STHENOS_SWIFTNESS", "Stheno's Swiftness"),
            EnchantmentTier::Unique
        );
        assert_eq!(
            EnchantmentTier::from_enchant("SHAITANS_MIGHT", "Shaitan's Might"),
            EnchantmentTier::Unique
        );
        assert_eq!(
            EnchantmentTier::from_enchant("AVALONS_INTELLECT", "Avalon's Intellect"),
            EnchantmentTier::Unique
        );
        assert_eq!(
            EnchantmentTier::from_enchant("JESTERS_TRICK", "Jester's Trick"),
            EnchantmentTier::Unique
        );

        // Normal enchants have mixed-case internal IDs
        assert_eq!(
            EnchantmentTier::from_enchant("Attack_Bonus_1", "Attack Bonus I"),
            EnchantmentTier::Normal
        );
        assert_eq!(
            EnchantmentTier::from_enchant("Mana_Regen", "Mana Regen"),
            EnchantmentTier::Normal
        );

        // Loot Bonus III/IV are still high-tier
        assert_eq!(
            EnchantmentTier::from_enchant("LOOT_BONUS_3", "Loot Bonus III"),
            EnchantmentTier::HighLootBonus
        );
        assert_eq!(
            EnchantmentTier::from_enchant("LOOT_BONUS_4", "Loot Bonus IV"),
            EnchantmentTier::HighLootBonus
        );
    }

    #[test]
    fn test_enchantment_tier_is_special() {
        assert!(!EnchantmentTier::Normal.is_special());
        assert!(EnchantmentTier::Unique.is_special());
        assert!(EnchantmentTier::Awakened.is_special());
        assert!(!EnchantmentTier::HighLootBonus.is_special());
    }

    #[test]
    fn test_high_loot_bonus_tier() {
        assert!(EnchantmentTier::HighLootBonus.is_high_loot_bonus());
        assert!(!EnchantmentTier::Normal.is_high_loot_bonus());
        assert!(!EnchantmentTier::Unique.is_high_loot_bonus());
        assert!(!EnchantmentTier::Awakened.is_high_loot_bonus());
    }

    #[test]
    fn test_valuable_tier() {
        // Valuable tiers: Unique, Awakened, HighLootBonus
        assert!(EnchantmentTier::Unique.is_valuable());
        assert!(EnchantmentTier::Awakened.is_valuable());
        assert!(EnchantmentTier::HighLootBonus.is_valuable());

        // Normal is not valuable
        assert!(!EnchantmentTier::Normal.is_valuable());
    }
}

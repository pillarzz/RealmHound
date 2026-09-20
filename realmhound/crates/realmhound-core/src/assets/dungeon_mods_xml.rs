//! Dungeon modifier definitions parsed from the game's `mods.xml`.
//!
//! RotMG ships the authoritative dungeon modifier list as `mods.xml` inside
//! `resources.assets`. The Unity extractor writes it to `assets/xml/mods.xml`,
//! and it is re-extracted automatically whenever the game updates. This parser
//! turns that file into an id-keyed table of display names and reward bonuses so
//! the app no longer relies on a hardcoded RealmEye-derived copy.
//!
//! Reward bonuses come from the `<Mutators>` block:
//! - `<EntityStatMultiplier stat="XP" amount="1.05" />`  -> xp%   = (amount-1)*100
//! - `<EntityDropMultiplier amount="1.01" />`            -> loot% = (amount-1)*100
//! - `<EntityDustMultiplier amount="1.02" />`            -> dust% = (amount-1)*100
//!
//! Other `EntityStatMultiplier` stats (Damage/Defense/MaxHitPoints) are enemy
//! tuning and are ignored for reward purposes.
//!
//! Lookups are keyed by the canonical form of the modifier id (uppercased ASCII
//! alphanumerics only), so the wire token `WEAKBOSS_3` matches the table id
//! `WEAKBOSS_3`, and `MERCA` matches `MERCA`.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::dungeon_modifiers::canonical;

/// A single dungeon modifier definition from `mods.xml`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModifierDef {
    /// Raw modifier id / wire token (e.g. "WEAKBOSS_3", "MERCA").
    pub id: String,
    /// Display name (e.g. "Weak Boss III"); may include a "(Legacy)" suffix for
    /// deprecated variants.
    pub display_name: String,
    /// Free-text description.
    pub description: String,
    /// XP gain bonus (%).
    pub xp: i32,
    /// Loot drop bonus (%).
    pub loot: i32,
    /// Dust gain bonus (%).
    pub dust: i32,
    /// Roll weight; 0 typically marks a deprecated/legacy variant.
    pub weight: i32,
    /// Comma-separated label list from the XML.
    pub labels: String,
}

impl ModifierDef {
    /// Whether this modifier is an active (rollable) variant rather than a
    /// deprecated legacy one. Legacy variants carry weight 0 and a "(Legacy)"
    /// display suffix.
    pub fn is_active(&self) -> bool {
        self.weight > 0 && !self.display_name.contains("(Legacy)")
    }
}

/// Collection of all dungeon modifier definitions, keyed by canonical id.
#[derive(Debug, Default, Clone)]
pub struct ModifierTable {
    by_id: HashMap<String, ModifierDef>,
}

impl ModifierTable {
    /// Create an empty table.
    pub fn new() -> Self {
        Self {
            by_id: HashMap::new(),
        }
    }

    /// Load modifiers from a `mods.xml` file.
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let file =
            File::open(path.as_ref()).map_err(|e| format!("Failed to open mods.xml: {}", e))?;
        let reader = BufReader::new(file);
        Self::parse_xml(reader)
    }

    /// Parse modifiers from XML content.
    pub fn parse_xml<R: std::io::BufRead>(reader: R) -> Result<Self, String> {
        let mut xml_reader = Reader::from_reader(reader);
        xml_reader.config_mut().trim_text(true);

        let mut table = Self::new();
        let mut buf = Vec::new();

        let mut current: Option<ModifierDef> = None;
        let mut in_display_id = false;
        let mut in_description = false;
        let mut in_weight = false;
        let mut in_labels = false;
        let mut in_mutators = false;

        loop {
            match xml_reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => match e.name().as_ref() {
                    b"DungeonModifier" => {
                        let mut def = ModifierDef::default();
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"id" {
                                def.id = String::from_utf8_lossy(&attr.value).to_string();
                            }
                        }
                        current = Some(def);
                    }
                    b"DisplayId" => in_display_id = true,
                    b"Description" => in_description = true,
                    b"Weight" => in_weight = true,
                    b"Labels" => in_labels = true,
                    b"Mutators" => in_mutators = true,
                    // A mutator with children is rare but possible; still parse
                    // its attributes.
                    other if in_mutators => {
                        apply_mutator(current.as_mut(), other, e);
                    }
                    _ => {}
                },
                // Self-closing mutator tags arrive as Empty events.
                Ok(Event::Empty(ref e)) => {
                    if in_mutators {
                        apply_mutator(current.as_mut(), e.name().as_ref(), e);
                    }
                }
                Ok(Event::End(ref e)) => match e.name().as_ref() {
                    b"DungeonModifier" => {
                        if let Some(def) = current.take() {
                            if !def.id.is_empty() {
                                table.by_id.insert(canonical(&def.id), def);
                            }
                        }
                    }
                    b"DisplayId" => in_display_id = false,
                    b"Description" => in_description = false,
                    b"Weight" => in_weight = false,
                    b"Labels" => in_labels = false,
                    b"Mutators" => in_mutators = false,
                    _ => {}
                },
                Ok(Event::Text(ref e)) => {
                    if let Some(ref mut def) = current {
                        let text = e.unescape().unwrap_or_default().to_string();
                        if in_display_id {
                            def.display_name = text;
                        } else if in_description {
                            def.description = text;
                        } else if in_labels {
                            def.labels = text;
                        } else if in_weight {
                            def.weight = text.trim().parse().unwrap_or(0);
                        }
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(format!("XML parse error: {}", e)),
                _ => {}
            }
            buf.clear();
        }

        if table.is_empty() {
            return Err("mods.xml contained no DungeonModifier entries".to_string());
        }

        Ok(table)
    }

    /// Number of loaded modifiers.
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Whether the table is empty.
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Look up a modifier by wire token / id (matched canonically).
    pub fn get(&self, token: &str) -> Option<&ModifierDef> {
        let key = canonical(token);
        if key.is_empty() {
            return None;
        }
        self.by_id.get(&key)
    }
}

/// Read the reward bonus from a single mutator element into the current
/// modifier. Recognizes `EntityStatMultiplier` (stat="XP"), `EntityDropMultiplier`
/// and `EntityDustMultiplier`; everything else is ignored.
///
/// Only the reward multiplier is counted. Mutators qualified by `itemId` or
/// `excludeLabels` describe per-item effects (e.g. doubling a specific potion
/// drop) and must NOT be added to the headline XP/loot/dust bonus, otherwise
/// mods like `GUARANTEEDSTATPOTION` (many `amount="100.00"` per-item entries)
/// would report absurd values. Mutators with only `reqLabels` (e.g. Squared's
/// boss/miniboss drop bonus) ARE counted since they represent a general loot
/// bonus for an enemy class.
fn apply_mutator(def: Option<&mut ModifierDef>, tag: &[u8], e: &quick_xml::events::BytesStart) {
    let def = match def {
        Some(d) => d,
        None => return,
    };

    // Ignore per-item qualified mutators (itemId / excludeLabels); enemy-class
    // qualifiers (reqLabels alone) still count as a headline bonus.
    if is_qualified_mutator(e) {
        return;
    }

    match tag {
        b"EntityStatMultiplier" => {
            let mut stat = String::new();
            let mut amount = None;
            for attr in e.attributes().flatten() {
                match attr.key.as_ref() {
                    b"stat" => stat = String::from_utf8_lossy(&attr.value).to_string(),
                    b"amount" => amount = parse_amount(&attr.value),
                    _ => {}
                }
            }
            if stat.eq_ignore_ascii_case("XP") {
                if let Some(pct) = amount {
                    def.xp += pct;
                }
            }
        }
        b"EntityDropMultiplier" => {
            if let Some(pct) = mutator_amount(e) {
                def.loot += pct;
            }
        }
        b"EntityDustMultiplier" => {
            if let Some(pct) = mutator_amount(e) {
                def.dust += pct;
            }
        }
        _ => {}
    }
}

/// Whether a mutator is per-item qualified, i.e. it applies only to a specific
/// drop rather than as a general reward bonus. `itemId` and `excludeLabels`
/// mark per-item entries (e.g. GUARANTEEDSTATPOTION's 100x potion lines);
/// `reqLabels` alone targets an enemy class (BOSS/MINIBOSS) and represents a
/// legitimate loot bonus (e.g. Squared's +45% boss drops).
fn is_qualified_mutator(e: &quick_xml::events::BytesStart) -> bool {
    e.attributes()
        .flatten()
        .any(|a| matches!(a.key.as_ref(), b"itemId" | b"excludeLabels"))
}

/// Extract and convert the `amount` attribute of a mutator into a percent.
fn mutator_amount(e: &quick_xml::events::BytesStart) -> Option<i32> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == b"amount")
        .and_then(|a| parse_amount(&a.value))
}

/// Convert a multiplier attribute value (e.g. `1.18`) into a percent bonus
/// (e.g. `18`), rounding to the nearest integer.
fn parse_amount(raw: &[u8]) -> Option<i32> {
    let s = String::from_utf8_lossy(raw);
    let mult: f64 = s.trim().parse().ok()?;
    Some(((mult - 1.0) * 100.0).round() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<DungeonModifiers>
  <DungeonModifier id="WEAKBOSS_3" type="0x55">
    <DisplayId>Weak Boss III</DisplayId>
    <Description>Bosses are weaker</Description>
    <Weight>30000</Weight>
    <Labels>ROLLABLE,ANY,BOSS</Labels>
    <Mutators>
      <EntityStatMultiplier amount="0.625" reqLabels="BOSSFIGHT" stat="Defense" />
      <EntityStatMultiplier stat="XP" amount="1.15" />
      <EntityDustMultiplier amount="1.03" />
    </Mutators>
  </DungeonModifier>
  <DungeonModifier id="WEAK_1" type="0x22a">
    <DisplayId>Weak I</DisplayId>
    <Description>Players have -3 ATT</Description>
    <Weight>10000</Weight>
    <Labels>ROLLABLE,ANY,STAT</Labels>
    <Mutators>
      <PlayerStatIncrement amount="-3" stat="ATT" />
      <EntityDropMultiplier amount="1.06" />
      <EntityDustMultiplier amount="1.02" />
    </Mutators>
  </DungeonModifier>
  <DungeonModifier id="MERCA" type="0x134">
    <DisplayId>Syndicate Takeover I</DisplayId>
    <Description>Pirate Queen Ramm</Description>
    <Weight>10000</Weight>
    <Labels>ROLLABLE,UNIQUE,MERCA</Labels>
    <Mutators>
      <SpecialSpawnOnDeath category="Mercenary_A" prob="1" reqLabels="BOSS" />
      <EntityStatMultiplier stat="XP" amount="1.1" />
      <EntityDustMultiplier amount="1.1" />
    </Mutators>
  </DungeonModifier>
  <DungeonModifier id="SOUVENIR_1" type="0xb">
    <DisplayId>Souvenir I (Legacy)</DisplayId>
    <Description>Legacy souvenir</Description>
    <Weight>0</Weight>
    <Labels>ANY,REWARD</Labels>
    <Mutators>
      <EntityStatMultiplier stat="XP" amount="1.08" />
    </Mutators>
  </DungeonModifier>
</DungeonModifiers>"#;

    fn parse() -> ModifierTable {
        ModifierTable::parse_xml(SAMPLE.as_bytes()).expect("parse sample")
    }

    #[test]
    fn parses_all_entries() {
        let t = parse();
        assert_eq!(t.len(), 4);
    }

    #[test]
    fn rejects_xml_without_modifier_entries() {
        let result = ModifierTable::parse_xml(b"<DungeonModifiers />".as_slice());
        assert_eq!(
            result.unwrap_err(),
            "mods.xml contained no DungeonModifier entries"
        );
    }

    #[test]
    fn xp_drop_dust_mutators_map_to_bonuses() {
        let t = parse();
        let m = t.get("WEAKBOSS_3").expect("weak boss");
        assert_eq!(m.display_name, "Weak Boss III");
        // Only the XP EntityStatMultiplier counts; Defense is ignored.
        assert_eq!(m.xp, 15);
        assert_eq!(m.loot, 0);
        assert_eq!(m.dust, 3);
    }

    #[test]
    fn weak_1_is_distinct_player_debuff_not_weak_minions() {
        let t = parse();
        let m = t.get("WEAK_1").expect("weak 1");
        assert_eq!(m.display_name, "Weak I");
        assert_eq!(m.xp, 0);
        assert_eq!(m.loot, 6);
        assert_eq!(m.dust, 2);
    }

    #[test]
    fn token_lookup_is_canonical() {
        let t = parse();
        // Wire tokens may arrive without underscores or in other forms; all
        // canonicalize to the same key.
        assert!(t.get("weakboss3").is_some());
        assert!(t.get("WEAKBOSS_3").is_some());
        assert_eq!(t.get("MERCA").unwrap().display_name, "Syndicate Takeover I");
    }

    #[test]
    fn legacy_variants_are_flagged_inactive() {
        let t = parse();
        assert!(!t.get("SOUVENIR_1").unwrap().is_active());
        assert!(t.get("WEAKBOSS_3").unwrap().is_active());
    }

    #[test]
    fn unknown_token_returns_none() {
        let t = parse();
        assert!(t.get("NOT_A_MOD").is_none());
        assert!(t.get("").is_none());
    }

    #[test]
    fn item_and_label_qualified_mutators_are_excluded_from_bonus() {
        // Mirrors real SURVIVOR_1 / GUARANTEEDSTATPOTION shapes: only the
        // unqualified global multiplier should count, not per-item ones.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<DungeonModifiers>
  <DungeonModifier id="SURVIVOR_1" type="0x1">
    <DisplayId>Survivor I</DisplayId>
    <Weight>10000</Weight>
    <Mutators>
      <EntityDropMultiplier amount="1.25" itemId="Health Potion" reqLabels="MINION" />
      <EntityDropMultiplier amount="1.25" itemId="Magic Potion" reqLabels="MINION" />
      <EntityDropMultiplier amount="1.01" />
      <EntityDustMultiplier amount="1.04" />
    </Mutators>
  </DungeonModifier>
  <DungeonModifier id="GUARANTEEDSTATPOTION" type="0x2">
    <DisplayId>Guaranteed Stat Potion</DisplayId>
    <Weight>10000</Weight>
    <Mutators>
      <EntityDropMultiplier amount="100.00" itemId="Potion of Attack" reqLabels="BOSS" />
      <EntityDropMultiplier amount="100.00" excludeLabels="SHORTMINIBOSS" itemId="Potion of Life" reqLabels="MINIBOSS" />
      <EntityStatMultiplier stat="XP" amount="1.18" />
      <EntityDropMultiplier amount="1.18" />
    </Mutators>
  </DungeonModifier>
</DungeonModifiers>"#;
        let t = ModifierTable::parse_xml(xml.as_bytes()).unwrap();

        let s = t.get("SURVIVOR_1").unwrap();
        assert_eq!((s.xp, s.loot, s.dust), (0, 1, 4));

        let g = t.get("GUARANTEEDSTATPOTION").unwrap();
        assert_eq!((g.xp, g.loot, g.dust), (18, 18, 0));
    }

    #[test]
    fn req_labels_only_drop_multiplier_counts_as_loot_bonus() {
        // Mirrors Squared: boss/miniboss EntityDropMultiplier with reqLabels but
        // no itemId should be counted as a headline loot bonus.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<DungeonModifiers>
  <DungeonModifier id="SQUARED" type="0x16a">
    <DisplayId>Squared</DisplayId>
    <Weight>15000</Weight>
    <Labels>ROLLABLE,3D</Labels>
    <Mutators>
      <EntityDropMultiplier amount="1.45" reqLabels="BOSS" />
      <EntityDropMultiplier amount="1.45" reqLabels="MINIBOSS" />
      <EntityStatMultiplier stat="XP" amount="1.15" />
      <EntityDustMultiplier amount="1.1" />
    </Mutators>
  </DungeonModifier>
</DungeonModifiers>"#;
        let t = ModifierTable::parse_xml(xml.as_bytes()).unwrap();
        let s = t.get("SQUARED").unwrap();
        assert_eq!((s.xp, s.loot, s.dust), (15, 90, 10));
    }

    /// Integration check against the real extracted `mods.xml`. Ignored by
    /// default because it requires assets to have been extracted from an
    /// installed game. Run with:
    /// `cargo test -p realmhound-core real_mods_xml -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_mods_xml_resolves_mismatched_wire_tokens() {
        let dir = crate::assets::find_assets_dir().expect("assets extracted");
        let path = dir.join("xml").join("mods.xml");
        let table = ModifierTable::load_from_file(&path).expect("load mods.xml");
        assert!(
            table.len() > 100,
            "expected many modifiers, got {}",
            table.len()
        );

        // Tokens whose real wire id differs from the RealmEye-derived fallback
        // key; these would silently fail on the hardcoded table.
        let merca = table.get("MERCA").expect("MERCA present");
        assert_eq!(merca.display_name, "Syndicate Takeover I");

        let wanderer = table.get("WANDERERBOSS").expect("WANDERERBOSS present");
        assert_eq!(wanderer.display_name, "Wanderer");
        assert_eq!(wanderer.loot, 16);

        let troom = table.get("FOOUNDTREASURE").expect("FOOUNDTREASURE present");
        assert!(troom.description.to_lowercase().contains("treasure room"));

        // WEAK_1 must be the player-debuff "Weak I", not "Weak Minions I".
        let weak1 = table.get("WEAK_1").expect("WEAK_1 present");
        assert!(weak1.display_name.starts_with("Weak I"));

        // Spot-check a stable reward mod's bonuses match the known table.
        let chef = table.get("CHEF").expect("CHEF present");
        assert_eq!((chef.xp, chef.loot, chef.dust), (10, 0, 14));

        // Mods with item/label-qualified mutators must report only the global
        // reward bonus, not the per-item multipliers.
        let survivor = table.get("SURVIVOR_1").expect("SURVIVOR_1 present");
        assert_eq!((survivor.xp, survivor.loot, survivor.dust), (0, 1, 4));

        let gsp = table
            .get("GUARANTEEDSTATPOTION")
            .expect("GUARANTEEDSTATPOTION present");
        assert_eq!((gsp.xp, gsp.loot, gsp.dust), (18, 18, 0));
    }
}

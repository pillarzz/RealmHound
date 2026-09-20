//! Seasonal "battle pass" mission DEFINITIONS.
//!
//! Parses the `missions/getClientSeasons` HTTP response (JSON), which supplies
//! the human-readable mission names, descriptions, objectives (`conds`) and
//! rewards that the game socket never sends. Progress/claim state comes from a
//! separate source (`missions/getPlayerMissions` + packet 165 deltas) and is
//! NOT modeled here.
//!
//! The structs are deliberately lenient: every field is `#[serde(default)]` and
//! unknown fields are ignored, so a single unfamiliar cond/reward or a schema
//! tweak never discards the whole payload.

use serde::{Deserialize, Deserializer};

/// Top-level `getClientSeasons` response.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ClientSeasons {
    #[serde(default)]
    pub seasons: Vec<Season>,
}

/// A single season (battle pass). `id` matches the `seasonId` in packet-165
/// `prog/` strings and the outgoing ClaimMission (163) packet.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Season {
    #[serde(default)]
    pub id: i32,
    #[serde(default)]
    pub name: String,
    /// `1` when this is the active season.
    #[serde(default)]
    pub current: i32,
    #[serde(default)]
    pub available: i32,
    /// Season end, unix seconds.
    #[serde(default, rename = "endDate")]
    pub end_date: i64,
    #[serde(default)]
    pub missions: Vec<MissionDef>,
}

/// One mission definition. `id` matches the `missionIdx` in packet-165 `prog/`
/// strings and the outgoing ClaimMission (163) packet.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MissionDef {
    #[serde(default)]
    pub id: i32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub desc: String,
    /// Sub-objectives, index-aligned with the packet-165 delta colon-array.
    #[serde(default)]
    pub conds: Vec<Cond>,
    /// When present the mission is "ONE OF BELOW": each character is a group id
    /// for the cond at the same index. Conds sharing a digit are alternatives
    /// (OR); distinct digits are independent groups (AND across groups). Absent
    /// means every cond is required. Raw value is preserved because the exact
    /// semantics of mixed spreads (e.g. `"1112"`) still need live validation.
    #[serde(default, rename = "condSpread")]
    pub cond_spread: Option<String>,
    #[serde(default)]
    pub rewards: Vec<Reward>,
    /// Optional reward grouping, same digit convention as `cond_spread`.
    #[serde(default, rename = "rewardsSpread")]
    pub rewards_spread: Option<String>,
    /// Prerequisite mission ids, source-formatted (e.g. `"2"` or `"9,18"`).
    #[serde(default, rename = "parentsStr")]
    pub parents_str: String,
    /// Sprite reference for the mission's unique icon, `"sheet/index"` (e.g.
    /// `"beacons32x32/206"`). Resolved to atlas pixels at render time.
    #[serde(default)]
    pub icon: String,
    /// Repeat interval in days (e.g. `0.75` = 18h); `0` for one-time missions.
    #[serde(default)]
    pub interval: f64,
    /// Eligibility bitmask: `255`=all, `2`=seasonal only, `4`=crucible only.
    #[serde(default)]
    pub participants: i32,
    /// Equipment restrictions: the character must have an item of each listed
    /// slot type equipped to make progress (e.g. an Orb, restricting the
    /// mission to Mystics). Empty when unrestricted. Deca serialises the absent
    /// value as an empty string and the present value as an array, so it is
    /// parsed leniently.
    #[serde(
        default,
        rename = "wornRestriction",
        deserialize_with = "de_worn_restrictions"
    )]
    pub worn_restriction: Vec<WornRestriction>,
}

/// A single worn-equipment restriction. `kind` is the game `SlotType` name
/// (e.g. `"ORB"`, `"SKULL"`); `display` is the human label (e.g. `"Orb"`).
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct WornRestriction {
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub display: String,
}

/// A single sub-objective.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Cond {
    /// See [`CondKind`]. Stored raw so unknown types survive.
    #[serde(default, rename = "type")]
    pub cond_type: i32,
    /// Meaning depends on `cond_type`: dungeon name (type 3/14), difficulty
    /// range like `"1-2"` (type 12), named entity (type 1), else empty.
    #[serde(default, deserialize_with = "de_lenient_string")]
    pub target: String,
    /// Required count (runs / kills / etc.).
    #[serde(default)]
    pub amount: i32,
    /// Time limit in seconds, present for timed (type 14) conds.
    #[serde(default)]
    pub time: Option<i32>,
}

/// A single reward.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Reward {
    /// `1` = battle XP (`amount`), `2` = item (`target` = object id).
    #[serde(default, rename = "type")]
    pub reward_type: i32,
    /// Item object id as a decimal string for item rewards; empty for BXP.
    #[serde(default, deserialize_with = "de_lenient_string")]
    pub target: String,
    /// Item count, or BXP amount (can be in the millions).
    #[serde(default)]
    pub amount: i64,
}

/// Decoded condition type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CondKind {
    /// type 1: kill a specific named entity (`target`, `amount`).
    KillNamed,
    /// type 2: defeat N enemies.
    KillCount,
    /// type 3: complete `amount` runs of dungeon `target`.
    Dungeon,
    /// type 4: complete a dungeon of an exact difficulty.
    DifficultyExact,
    /// type 7: close N realms.
    CloseRealms,
    /// type 12: complete `amount` runs within a difficulty range (`target`).
    DifficultyRange,
    /// type 14: complete dungeon `target` under `time` seconds, `amount` times.
    Timed,
    /// type 15: gain `amount` EXP (e.g. reach a character level).
    GainXp,
    /// Any type not yet decoded.
    Unknown(i32),
}

impl Cond {
    /// Decode [`Self::cond_type`] into a [`CondKind`].
    pub fn kind(&self) -> CondKind {
        match self.cond_type {
            1 => CondKind::KillNamed,
            2 => CondKind::KillCount,
            3 => CondKind::Dungeon,
            4 => CondKind::DifficultyExact,
            7 => CondKind::CloseRealms,
            12 => CondKind::DifficultyRange,
            14 => CondKind::Timed,
            15 => CondKind::GainXp,
            other => CondKind::Unknown(other),
        }
    }

    /// Short label for this sub-objective, used when rendering per-cond progress
    /// (e.g. "Ocean Trench", "Difficulty 1-2", "Defeat enemies").
    pub fn objective_label(&self) -> String {
        match self.kind() {
            CondKind::KillNamed => {
                if is_vortex_target(&self.target) {
                    if self.amount == 1 {
                        "Wind Vortex".to_string()
                    } else {
                        "Wind Vortexes".to_string()
                    }
                } else {
                    humanize_target_counted(strip_boss_prefix(&self.target), self.amount)
                }
            }
            CondKind::Dungeon => humanize_target(&self.target),
            CondKind::Timed => {
                if let Some(secs) = self.time {
                    format!("{} (under {}s)", humanize_target(&self.target), secs)
                } else {
                    humanize_target(&self.target)
                }
            }
            CondKind::DifficultyRange => format!("Difficulty {}", self.target),
            CondKind::DifficultyExact => "Difficulty (exact)".to_string(),
            CondKind::KillCount => "Defeat enemies".to_string(),
            CondKind::CloseRealms => "Close realms".to_string(),
            CondKind::GainXp => "Gain EXP".to_string(),
            CondKind::Unknown(_) => {
                if self.target.is_empty() {
                    "Objective".to_string()
                } else {
                    humanize_target(&self.target)
                }
            }
        }
    }

    /// A full human-readable objective phrase including the required count and a
    /// verb (e.g. "Defeat 5 Adept Encounter", "Complete 3 Ocean Trench"). This
    /// mirrors the game's reliable "Completion" line, sourced from the structured
    /// objective rather than the hand-typed mission description.
    pub fn objective_phrase(&self) -> String {
        let count = if self.amount > 0 {
            format!("{} ", self.amount)
        } else {
            String::new()
        };
        match self.kind() {
            CondKind::KillNamed => {
                if is_vortex_target(&self.target) {
                    let noun = if self.amount == 1 {
                        "Wind Vortex"
                    } else {
                        "Wind Vortexes"
                    };
                    format!("Defeat {count}{noun} in dungeons")
                } else {
                    format!(
                        "Defeat {count}{}",
                        humanize_target_counted(strip_boss_prefix(&self.target), self.amount)
                    )
                }
            }
            CondKind::KillCount => format!("Defeat {count}enemies"),
            CondKind::Dungeon => format!("Complete {count}{}", humanize_target(&self.target)),
            CondKind::Timed => {
                let dungeon = humanize_target(&self.target);
                match self.time {
                    Some(secs) => format!("Complete {count}{dungeon} under {secs}s"),
                    None => format!("Complete {count}{dungeon}"),
                }
            }
            CondKind::DifficultyRange => {
                let dungeons = if self.amount == 1 {
                    "Dungeon"
                } else {
                    "Dungeons"
                };
                format!("Complete {count}{dungeons} Difficulty {}", self.target)
            }
            CondKind::DifficultyExact => {
                let dungeons = if self.amount == 1 {
                    "Dungeon"
                } else {
                    "Dungeons"
                };
                format!("Complete {count}{dungeons} at an exact difficulty")
            }
            CondKind::CloseRealms => format!("Close {count}realms"),
            CondKind::GainXp => {
                if self.amount > 0 {
                    format!("Gain {} EXP", self.amount)
                } else {
                    "Gain EXP".to_string()
                }
            }
            CondKind::Unknown(_) => {
                if self.target.is_empty() {
                    format!("Complete {count}objective")
                } else {
                    format!("Complete {count}{}", humanize_target(&self.target))
                }
            }
        }
    }
}

/// Convert a SCREAMING_SNAKE_CASE mission tag (e.g. `ADEPT_ENCOUNTER`) into a
/// human-readable, title-cased string (`Adept Encounter`). Already-readable
/// names (anything containing lowercase, spaces, or punctuation) pass through
/// unchanged so real dungeon/boss names are untouched.
fn humanize_target(target: &str) -> String {
    let t = target.trim();
    if !is_screaming_tag(t) {
        return t.to_string();
    }
    t.split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first, chars.as_str().to_lowercase()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// True when `target` is a SCREAMING_SNAKE_CASE category tag (e.g.
/// `ADEPT_ENCOUNTER`, `BEACON_GUARDIAN`, `GOD`) rather than a real dungeon or
/// boss name. Only tags get pluralized, so proper names are left untouched.
fn is_screaming_tag(target: &str) -> bool {
    let t = target.trim();
    !t.is_empty()
        && t.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && t.chars().any(|c| c.is_ascii_uppercase())
}

/// Pluralize a single English word with simple suffix rules
/// (Encounter->Encounters, Vortex->Vortexes, Enemy->Enemies, God->Gods).
fn pluralize_word(w: &str) -> String {
    let l = w.to_ascii_lowercase();
    if l.ends_with('s')
        || l.ends_with('x')
        || l.ends_with('z')
        || l.ends_with("ch")
        || l.ends_with("sh")
    {
        format!("{w}es")
    } else if l.ends_with('y')
        && w.len() > 1
        && !matches!(l.chars().rev().nth(1), Some('a' | 'e' | 'i' | 'o' | 'u'))
    {
        format!("{}ies", &w[..w.len() - 1])
    } else {
        format!("{w}s")
    }
}

/// Pluralize the final word of a phrase, leaving any leading qualifier
/// ("Adept Encounter" -> "Adept Encounters", "Beacon Guardian" -> "Beacon Guardians").
fn pluralize_phrase(phrase: &str) -> String {
    match phrase.rsplit_once(' ') {
        Some((head, last)) => format!("{head} {}", pluralize_word(last)),
        None => pluralize_word(phrase),
    }
}

/// Humanize a target tag, pluralizing the trailing noun when the required
/// `amount` is not exactly 1. Only category tags are pluralized; real
/// dungeon/boss names pass through unchanged.
fn humanize_target_counted(target: &str, amount: i32) -> String {
    let phrase = humanize_target(target);
    if amount != 1 && is_screaming_tag(target) {
        pluralize_phrase(&phrase)
    } else {
        phrase
    }
}

/// True when a KillNamed target refers to a Wind Vortex (defeated inside
/// dungeons), which gets bespoke "... in dungeons" wording.
pub fn is_vortex_target(target: &str) -> bool {
    target.to_ascii_lowercase().contains("vortex")
}

/// Strip a leading lowercase dungeon abbreviation from a boss target
/// ("shtrs Twilight Archmage" -> "Twilight Archmage"). Mission KillNamed targets
/// prefix the boss name with a lowercase dungeon tag; boss names are proper
/// nouns starting with an uppercase letter, so only an all-lowercase leading
/// token is dropped (leaving already-clean names untouched).
fn strip_boss_prefix(target: &str) -> &str {
    let t = target.trim();
    if let Some((head, rest)) = t.split_once(' ') {
        let rest = rest.trim();
        if !head.is_empty()
            && !rest.is_empty()
            && head
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        {
            return rest;
        }
    }
    t
}

impl MissionDef {
    /// Prerequisite mission ids parsed from `parents_str` (source-formatted as
    /// e.g. `"2"` or `"9,18"`). A mission unlocks once all its parents are
    /// completed/claimed; a mission with no parents is a graph root.
    pub fn parents(&self) -> Vec<i32> {
        self.parents_str
            .split(|c: char| !c.is_ascii_digit())
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect()
    }

    /// True when the mission uses "ONE OF BELOW" grouping (`condSpread` present).
    pub fn is_one_of(&self) -> bool {
        self.cond_spread.as_deref().is_some_and(|s| !s.is_empty())
    }

    /// A reliable, human-readable objective description built from the structured
    /// sub-objectives.
    ///
    /// - No conds: empty, so callers fall back to the hand-typed `desc`.
    /// - One cond: the full phrase (e.g. "Complete 3 Dungeons Difficulty 7-10").
    /// - Multiple conds: a concise header ("Complete one of the below" /
    ///   "Complete all of the below") mirroring the game's "ONE OF BELOW:"
    ///   layout, since repeating the verb+count per option is redundant with the
    ///   per-objective bars (which already show each option's icon and count).
    pub fn objective_description(&self) -> String {
        match self.conds.len() {
            0 => String::new(),
            1 => self.conds[0].objective_phrase(),
            _ => {
                if self.is_one_of() {
                    "Complete one of the below".to_string()
                } else {
                    "Complete all of the below".to_string()
                }
            }
        }
    }

    /// Parse the `icon` field into `(sheet_name, index)` for atlas lookup.
    pub fn icon_ref(&self) -> Option<(&str, i32)> {
        let (sheet, idx) = self.icon.rsplit_once('/')?;
        let idx: i32 = idx.trim().parse().ok()?;
        (!sheet.is_empty()).then_some((sheet, idx))
    }

    /// Cooldown between claims in seconds for a repeatable mission (from
    /// `interval`, in days), or `None` for one-time missions.
    pub fn cooldown_secs(&self) -> Option<i64> {
        (self.interval > 0.0).then(|| (self.interval * 86_400.0).round() as i64)
    }

    /// Group cond indices by their `cond_spread` digit. Conds in the same group
    /// are alternatives (satisfy any one); groups are ANDed together. With no
    /// spread, all conds form a single all-required group. Order of appearance
    /// is preserved. This is advisory only: authoritative completion/claim state
    /// must come from the server, not this computation.
    pub fn cond_groups(&self) -> Vec<Vec<usize>> {
        match self.cond_spread.as_deref() {
            Some(spread) if !spread.is_empty() => {
                let mut order: Vec<char> = Vec::new();
                let mut groups: Vec<Vec<usize>> = Vec::new();
                for (idx, _) in self.conds.iter().enumerate() {
                    // Fall back to a unique key when the spread is shorter than
                    // the cond list so extra conds become their own groups.
                    let key = spread.chars().nth(idx).unwrap_or('\u{0}');
                    if let Some(pos) = order.iter().position(|c| *c == key) {
                        groups[pos].push(idx);
                    } else {
                        order.push(key);
                        groups.push(vec![idx]);
                    }
                }
                groups
            }
            _ => (0..self.conds.len()).map(|i| vec![i]).collect(),
        }
    }

    /// Group reward indices by their `rewards_spread` digit, same convention as
    /// [`Self::cond_groups`]: rewards sharing a digit are a "pick one of" group
    /// (OR); distinct digits are independent groups (AND across groups). Absent
    /// spread means every reward is granted (each its own group).
    pub fn reward_groups(&self) -> Vec<Vec<usize>> {
        match self.rewards_spread.as_deref() {
            Some(spread) if !spread.is_empty() => {
                let mut order: Vec<char> = Vec::new();
                let mut groups: Vec<Vec<usize>> = Vec::new();
                for (idx, _) in self.rewards.iter().enumerate() {
                    let key = spread.chars().nth(idx).unwrap_or('\u{0}');
                    if let Some(pos) = order.iter().position(|c| *c == key) {
                        groups[pos].push(idx);
                    } else {
                        order.push(key);
                        groups.push(vec![idx]);
                    }
                }
                groups
            }
            _ => (0..self.rewards.len()).map(|i| vec![i]).collect(),
        }
    }

    /// Best-effort completion check from an absolute per-cond progress array
    /// (index-aligned with `conds`). Within a [`Self::cond_groups`] group any
    /// one cond meeting its `amount` satisfies the group (OR); every group must
    /// be satisfied (AND). Advisory only - authoritative claim state comes from
    /// the server `state`.
    pub fn is_complete(&self, progress: &[i32]) -> bool {
        if self.conds.is_empty() {
            return false;
        }
        self.cond_groups().iter().all(|group| {
            group.iter().any(|&i| {
                let have = progress.get(i).copied().unwrap_or(0);
                have >= self.conds[i].amount
            })
        })
    }
}

impl Season {
    /// Find a mission by its positional id (the `missionIdx` in packet 165).
    pub fn mission(&self, mission_id: i32) -> Option<&MissionDef> {
        self.missions.iter().find(|m| m.id == mission_id)
    }
}

impl ClientSeasons {
    /// Find a season by id (the `seasonId` in packet 165).
    pub fn season(&self, season_id: i32) -> Option<&Season> {
        self.seasons.iter().find(|s| s.id == season_id)
    }

    /// Distinct `cond_type` values we don't yet have a phrasing template for
    /// (i.e. decode to [`CondKind::Unknown`]), sorted ascending. Used to detect
    /// when Deca introduces a brand-new objective type so it can be surfaced in
    /// the logs instead of silently falling back to generic phrasing.
    pub fn unknown_cond_types(&self) -> Vec<i32> {
        let mut types: Vec<i32> = self
            .seasons
            .iter()
            .flat_map(|s| s.missions.iter())
            .flat_map(|m| m.conds.iter())
            .filter_map(|c| match c.kind() {
                CondKind::Unknown(t) => Some(t),
                _ => None,
            })
            .collect();
        types.sort_unstable();
        types.dedup();
        types
    }
}

/// Log a one-time warning for each objective `cond_type` we don't recognize, so
/// a new mission-objective type Deca adds announces itself in the logs (with the
/// first mission that uses it) rather than silently degrading to the generic
/// fallback. Each type is reported at most once per process.
fn report_unknown_cond_types(seasons: &ClientSeasons) {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static WARNED: OnceLock<Mutex<HashSet<i32>>> = OnceLock::new();
    let warned = WARNED.get_or_init(|| Mutex::new(HashSet::new()));
    let mut warned = match warned.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    for season in &seasons.seasons {
        for mission in &season.missions {
            for cond in &mission.conds {
                if let CondKind::Unknown(t) = cond.kind() {
                    if warned.insert(t) {
                        tracing::warn!(
                            "[MISSIONS] Unrecognized objective cond_type={} \
                             (mission {} {:?}, target={:?}, amount={}) - using generic \
                             phrasing; add a template in Cond::objective_phrase/objective_label",
                            t,
                            mission.id,
                            mission.name,
                            cond.target,
                            cond.amount,
                        );
                    }
                }
            }
        }
    }
}

/// Parse a raw `getClientSeasons` JSON body. Unknown fields are ignored and
/// missing fields default, so partial/older payloads still parse.
pub fn parse_client_seasons(json: &str) -> Result<ClientSeasons, serde_json::Error> {
    let seasons: ClientSeasons = serde_json::from_str(json)?;
    report_unknown_cond_types(&seasons);
    Ok(seasons)
}

/// Deserialize a field that is normally a JSON string but may occasionally be a
/// number (e.g. an item id emitted unquoted). Null/other types become empty.
fn de_lenient_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    use serde_json::Value;
    let value = Value::deserialize(deserializer)?;
    Ok(match value {
        Value::String(s) => s,
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    })
}

/// Lenient deserializer for `wornRestriction`: Deca sends an empty string when
/// there is no restriction and an array of `{type, display}` objects when there
/// is one. Anything that is not a well-formed array yields an empty list.
fn de_worn_restrictions<'de, D>(deserializer: D) -> Result<Vec<WornRestriction>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde_json::Value;
    let value = Value::deserialize(deserializer)?;
    Ok(match value {
        Value::Array(items) => items
            .into_iter()
            .filter_map(|v| serde_json::from_value::<WornRestriction>(v).ok())
            .filter(|w| !w.kind.is_empty())
            .collect(),
        _ => Vec::new(),
    })
}

// ----------------------------------------------------------------------------
// Packet-165 progress stream (`prog/`) parsing.
// ----------------------------------------------------------------------------

/// One progress update decoded from a packet-165 string. `values` is the
/// per-sub-objective array (index-aligned with `MissionDef::conds`); a
/// single-cond mission carries one element.
///
/// Live in-run updates arrive as small increments, while the login sync sends a
/// snapshot; callers reconcile these against the authoritative
/// `getPlayerMissions` baseline rather than trusting them as absolutes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgEntry {
    pub season_id: i32,
    pub mission_id: i32,
    pub values: Vec<i32>,
}

/// Parse a packet-165 (`Unknown165`) string into progress entries.
///
/// Format: an optional `pool/#` prefix, then `prog/` followed by one or more
/// `|`-joined entries of `<seasonId>,<missionId>,<v0>[:<v1>:...]`. The bare
/// `pool/#` heartbeat and any malformed entry yield nothing.
pub fn parse_prog_string(s: &str) -> Vec<ProgEntry> {
    let s = s.trim();
    let s = s.strip_prefix("pool/#").unwrap_or(s);
    // Only progress payloads are relevant; heartbeats / other keys are ignored.
    let Some(body) = s.strip_prefix("prog/") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in body.split('|') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let mut parts = entry.split(',');
        let (Some(season), Some(mission), Some(values)) =
            (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        // Reject trailing junk beyond the value field to stay strict.
        if parts.next().is_some() {
            continue;
        }
        let (Ok(season_id), Ok(mission_id)) = (season.trim().parse(), mission.trim().parse())
        else {
            continue;
        };
        let mut vals = Vec::new();
        let mut ok = true;
        for v in values.split(':') {
            match v.trim().parse::<i32>() {
                Ok(n) => vals.push(n),
                Err(_) => {
                    ok = false;
                    break;
                }
            }
        }
        if ok && !vals.is_empty() {
            out.push(ProgEntry {
                season_id,
                mission_id,
                values: vals,
            });
        }
    }
    out
}

// ----------------------------------------------------------------------------
// getPlayerMissions (authoritative player progress + claim state).
// ----------------------------------------------------------------------------

/// Server-side status of a mission for the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissionState {
    /// Objective not yet met (raw state 0).
    InProgress,
    /// Objective met, reward not yet claimed (raw state 1).
    Claimable,
    /// Reward claimed (raw state 2 one-time, or 3 repeatable/on-cooldown).
    Claimed,
    /// Mission is locked (prerequisites in its tree not yet met). Not a raw
    /// `<Season>` state - derived from the `<LockedMissions>` list by the
    /// tracker.
    Locked,
    /// Any state code not yet decoded.
    Unknown(i32),
}

impl MissionState {
    fn from_raw(raw: i32) -> Self {
        match raw {
            0 => Self::InProgress,
            1 => Self::Claimable,
            2 | 3 => Self::Claimed,
            other => Self::Unknown(other),
        }
    }
}

/// One mission's authoritative progress from `getPlayerMissions`.
#[derive(Debug, Clone)]
pub struct PlayerMission {
    /// Tree (season) id this mission belongs to, from the enclosing
    /// `<Season id="X">` block.
    pub tree_id: i32,
    pub mission_id: i32,
    /// Absolute per-sub-objective progress, index-aligned with `conds`.
    pub progress: Vec<i32>,
    pub state: MissionState,
    /// Raw state code, retained for diagnostics / future decode.
    pub raw_state: i32,
    /// Unix seconds the mission was last claimed (6th CSV field). For a
    /// repeatable mission on cooldown this anchors the reset: the mission
    /// restarts at `claimed_at + interval` (interval from the definition).
    pub claimed_at: Option<i64>,
}

/// Parsed `getPlayerMissions` response. Models the per-tree `<Season>` progress
/// blocks plus the tree-level lock state (`<TreesUnlocked>`, `<LockedMissions>`,
/// `<UnlockedMissions>`) so a battlepass with multiple mission trees - some
/// fully unlocked, some with individually locked missions - can be shown.
#[derive(Debug, Clone, Default)]
pub struct PlayerMissions {
    /// Primary season id: the first `<Season id=...>` attribute, else the first
    /// `<TreesUnlocked>` id. Used as the header/title anchor.
    pub season_id: Option<i32>,
    /// Timestamp (unix seconds) shared by the `<ActivePoolMissions>` entries.
    /// Empirically equals the season start, so it anchors the battlepass-end
    /// estimate. `None` when the section is absent/malformed.
    pub pool_ts: Option<i64>,
    pub missions: Vec<PlayerMission>,
    /// Fully-unlocked tree ids (`<TreesUnlocked>`): every mission in these trees
    /// is available.
    pub trees_unlocked: Vec<i32>,
    /// Individually locked missions as `(tree_id, mission_id)` from
    /// `<LockedMissions>` (`;treeId,missionId` entries).
    pub locked_missions: Vec<(i32, i32)>,
    /// Individually unlocked missions as `(tree_id, mission_id)` from
    /// `<UnlockedMissions>`.
    pub unlocked_missions: Vec<(i32, i32)>,
}

impl PlayerMissions {
    pub fn mission(&self, mission_id: i32) -> Option<&PlayerMission> {
        self.missions.iter().find(|m| m.mission_id == mission_id)
    }
}

/// Parse a raw `getPlayerMissions` body. Extracts the `<Season>` block entries
/// (`missionId,progressArray,state,...`); unknown/extra trailing fields are
/// ignored and malformed entries skipped, so a partial payload still yields the
/// entries it can.
pub fn parse_player_missions(body: &str) -> PlayerMissions {
    let mut result = PlayerMissions::default();

    // Fully-unlocked trees (`<TreesUnlocked>52,31</TreesUnlocked>`). Split on any
    // non-digit so either comma- or whitespace-separated payloads parse.
    if let Some(inner) = extract_tag(body, "TreesUnlocked") {
        result.trees_unlocked = inner
            .split(|c: char| !c.is_ascii_digit())
            .filter_map(|t| t.parse().ok())
            .collect();
    }

    // Per-mission lock lists for trees that aren't fully unlocked. Entries are
    // `;treeId,missionId` (leading `;` and empty fields skipped).
    result.locked_missions = parse_tree_mission_pairs(extract_tag(body, "LockedMissions"));
    result.unlocked_missions = parse_tree_mission_pairs(extract_tag(body, "UnlockedMissions"));

    // Primary season id: first <Season id="X"> attribute, else first unlocked
    // tree. Anchors the header title.
    result.season_id =
        extract_attr_i32(body, "<Season id=\"").or_else(|| result.trees_unlocked.first().copied());

    // Pool-mission timestamp (`poolId:missionId:progress:ts`), shared by all
    // entries and equal to the season start. Take the first entry's 4th field.
    if let Some(pool) = extract_tag(body, "ActivePoolMissions") {
        result.pool_ts = pool.split('|').find_map(|e| {
            e.trim()
                .split(':')
                .nth(3)
                .and_then(|t| t.trim().parse().ok())
        });
    }

    // Every `<Season id="X">` block (a battlepass can expose more than one tree
    // as separate blocks); each mission is tagged with its tree id.
    for (tree_id, season_block) in extract_all_tags(body, "Season") {
        let tree_id = tree_id.unwrap_or(0);
        for entry in season_block.split('|') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            let mut parts = entry.split(',');
            let (Some(id), Some(prog), Some(state)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let (Ok(mission_id), Ok(raw_state)) = (id.trim().parse(), state.trim().parse::<i32>())
            else {
                continue;
            };
            let progress: Vec<i32> = prog
                .split(':')
                .filter_map(|v| v.trim().parse::<i32>().ok())
                .collect();
            // Trailing fields: `T_updated, count, T_claimed`. The last field is
            // the last-claimed timestamp used to anchor a repeatable mission's
            // reset.
            let _t_updated = parts.next();
            let _count = parts.next();
            let claimed_at = parts
                .next()
                .and_then(|v| v.trim().parse::<i64>().ok())
                .filter(|&t| t > 0);
            result.missions.push(PlayerMission {
                tree_id,
                mission_id,
                progress,
                state: MissionState::from_raw(raw_state),
                raw_state,
                claimed_at,
            });
        }
    }
    result
}

/// Parse a `;treeId,missionId;treeId,missionId` lock list into `(tree, mission)`
/// pairs. Empty/malformed entries (including the leading empty field) are
/// skipped.
fn parse_tree_mission_pairs(section: Option<&str>) -> Vec<(i32, i32)> {
    let Some(section) = section else {
        return Vec::new();
    };
    section
        .split(';')
        .filter_map(|e| {
            let (tree, mission) = e.trim().split_once(',')?;
            Some((tree.trim().parse().ok()?, mission.trim().parse().ok()?))
        })
        .collect()
}

/// Return the text between `<tag>` and `</tag>` (first occurrence).
fn extract_tag<'a>(body: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}");
    let start_tag = body.find(&open)?;
    // Skip to the end of the opening tag (handles attributes like id="52").
    let content_start = start_tag + body[start_tag..].find('>')? + 1;
    let close = format!("</{tag}>");
    let end = body[content_start..].find(&close)? + content_start;
    Some(&body[content_start..end])
}

/// Return every `<tag ...>...</tag>` block with its parsed `id` attribute (when
/// present). Used to read multiple `<Season id="X">` trees from one payload.
fn extract_all_tags<'a>(body: &'a str, tag: &str) -> Vec<(Option<i32>, &'a str)> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut cursor = 0;
    while let Some(rel) = body[cursor..].find(&open) {
        let start_tag = cursor + rel;
        // Require a tag-name boundary so `<Season` doesn't match `<Seasonal`.
        let after = body[start_tag + open.len()..].chars().next();
        if !matches!(after, Some(c) if c.is_whitespace() || c == '>' || c == '/') {
            cursor = start_tag + open.len();
            continue;
        }
        let Some(gt) = body[start_tag..].find('>') else {
            break;
        };
        let open_end = start_tag + gt + 1;
        let Some(rel_close) = body[open_end..].find(&close) else {
            break;
        };
        let end = open_end + rel_close;
        // Parse the `id="X"` attribute from the opening tag, if any.
        let open_tag = &body[start_tag..open_end];
        let id = find_attr(open_tag, "id").and_then(|v| v.trim().parse().ok());
        out.push((id, &body[open_end..end]));
        cursor = end + close.len();
    }
    out
}

/// Find the value of attribute `name` in an opening tag, enforcing an
/// attribute-name boundary so `id` never matches `grid`.
fn find_attr<'a>(open_tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");
    let mut search = 0;
    while let Some(rel) = open_tag[search..].find(&needle) {
        let at = search + rel;
        let before = open_tag[..at].chars().next_back();
        if matches!(before, Some(c) if c.is_whitespace()) || at == 0 {
            let rest = &open_tag[at + needle.len()..];
            return rest.find('"').map(|q| &rest[..q]);
        }
        search = at + needle.len();
    }
    None
}

/// Parse an integer attribute value that immediately follows `prefix`
/// (e.g. prefix `<Season id="` -> the number before the closing quote).
fn extract_attr_i32(body: &str, prefix: &str) -> Option<i32> {
    let start = body.find(prefix)? + prefix.len();
    let rest = &body[start..];
    let end = rest.find('"')?;
    rest[..end].trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Representative slice of a real getClientSeasons response covering: a
    // type-2 kill-count mission, a type-3 dungeon mission, a type-12 diff-range,
    // a type-14 timed mission (with `time`), and a `condSpread` "one of" mission
    // with two type-3 conds. Item-id target given as a bare number to exercise
    // the lenient string deserializer.
    const SAMPLE: &str = r#"
    {
      "seasons": [{
        "name": "Dance of Fire and Ice",
        "current": 1,
        "available": 1,
        "id": 37,
        "endDate": 1768726799,
        "extraUnknownField": 123,
        "missions": [
          { "id": 1, "name": "Frozen Ground", "desc": "Defeat 50 enemies",
            "conds": [{ "type": 2, "target": "", "amount": 50 }],
            "rewards": [{ "type": 2, "target": 3138, "amount": 1 },
                        { "type": 1, "target": "", "amount": 1400000 }],
            "participants": 255 },
          { "id": 2, "name": "Trial of Ages",
            "desc": "Complete 2 runs of dungeons with difficulty range 1-2",
            "conds": [{ "type": 12, "target": "1-2", "amount": 2 }],
            "rewards": [{ "type": 1, "target": "", "amount": 4200000 }],
            "parentsStr": "1", "participants": 255 },
          { "id": 5, "name": "The Sunken Temple", "desc": "Timed",
            "conds": [{ "type": 14, "target": "Parasite Chambers", "amount": 1, "time": 410 },
                      { "type": 14, "target": "Cnidarian Reef", "amount": 1, "time": 240 }],
            "rewards": [], "parentsStr": "4", "participants": 4 },
          { "id": 9, "name": "Circle of Magics",
            "desc": "Complete 2 runs of Sprite World or Magic Woods",
            "conds": [{ "type": 3, "target": "Sprite World", "amount": 2 },
                      { "type": 3, "target": "Magic Woods", "amount": 2 }],
            "rewards": [{ "type": 1, "target": "", "amount": 4500000 }],
            "parentsStr": "2", "participants": 2, "condSpread": "11" }
        ]
      }]
    }"#;

    fn parsed() -> ClientSeasons {
        parse_client_seasons(SAMPLE).expect("sample should parse")
    }

    #[test]
    fn parses_season_and_missions() {
        let cs = parsed();
        let season = cs.season(37).expect("season 37");
        assert_eq!(season.name, "Dance of Fire and Ice");
        assert_eq!(season.current, 1);
        assert_eq!(season.missions.len(), 4);
    }

    #[test]
    fn lenient_string_accepts_numeric_target() {
        let cs = parsed();
        let m = cs.season(37).unwrap().mission(1).unwrap();
        // target came in as the bare number 3138.
        assert_eq!(m.rewards[0].target, "3138");
        assert_eq!(m.rewards[1].amount, 1_400_000);
    }

    #[test]
    fn icon_and_interval_parse() {
        let json = r#"{"id":6,"name":"Storming the Watch (Repeatable)","interval":0.75,
            "icon":"beacons32x32/206","conds":[],"rewards":[]}"#;
        let m: MissionDef = serde_json::from_str(json).unwrap();
        assert_eq!(m.icon_ref(), Some(("beacons32x32", 206)));
        assert_eq!(m.cooldown_secs(), Some(64_800)); // 0.75 day = 18h
                                                     // One-time mission: no icon / no cooldown.
        let one: MissionDef = serde_json::from_str(r#"{"id":1}"#).unwrap();
        assert_eq!(one.icon_ref(), None);
        assert_eq!(one.cooldown_secs(), None);
    }

    #[test]
    fn cond_kinds_decode() {
        let cs = parsed();
        let s = cs.season(37).unwrap();
        assert_eq!(s.mission(1).unwrap().conds[0].kind(), CondKind::KillCount);
        assert_eq!(
            s.mission(2).unwrap().conds[0].kind(),
            CondKind::DifficultyRange
        );
        assert_eq!(s.mission(5).unwrap().conds[0].kind(), CondKind::Timed);
        assert_eq!(s.mission(9).unwrap().conds[0].kind(), CondKind::Dungeon);
    }

    #[test]
    fn objective_labels() {
        let cs = parsed();
        let s = cs.season(37).unwrap();
        assert_eq!(
            s.mission(9).unwrap().conds[0].objective_label(),
            "Sprite World"
        );
        assert_eq!(
            s.mission(2).unwrap().conds[0].objective_label(),
            "Difficulty 1-2"
        );
        assert_eq!(
            s.mission(5).unwrap().conds[0].objective_label(),
            "Parasite Chambers (under 410s)"
        );
    }

    #[test]
    fn humanizes_screaming_snake_tags_only() {
        assert_eq!(humanize_target("ADEPT_ENCOUNTER"), "Adept Encounter");
        assert_eq!(humanize_target("VETERAN_ENCOUNTER"), "Veteran Encounter");
        assert_eq!(humanize_target("TIER_2_BOSS"), "Tier 2 Boss");
        // Already-readable names pass through untouched.
        assert_eq!(humanize_target("Ocean Trench"), "Ocean Trench");
        assert_eq!(
            humanize_target("shtrs Twilight Archmage"),
            "shtrs Twilight Archmage"
        );
    }

    #[test]
    fn objective_phrase_and_description_are_reliable_and_readable() {
        let kill = Cond {
            cond_type: 1,
            target: "ADEPT_ENCOUNTER".to_string(),
            amount: 5,
            time: None,
        };
        assert_eq!(kill.objective_label(), "Adept Encounters");
        assert_eq!(kill.objective_phrase(), "Defeat 5 Adept Encounters");

        // A single-count encounter stays singular.
        let one_kill = Cond {
            cond_type: 1,
            target: "ADEPT_ENCOUNTER".to_string(),
            amount: 1,
            time: None,
        };
        assert_eq!(one_kill.objective_phrase(), "Defeat 1 Adept Encounter");

        // Category tags pluralize their trailing noun by simple English rules.
        let beacons = Cond {
            cond_type: 1,
            target: "BEACON_GUARDIAN".to_string(),
            amount: 3,
            time: None,
        };
        assert_eq!(beacons.objective_phrase(), "Defeat 3 Beacon Guardians");
        let gods = Cond {
            cond_type: 1,
            target: "GOD".to_string(),
            amount: 4,
            time: None,
        };
        assert_eq!(gods.objective_phrase(), "Defeat 4 Gods");

        // A real boss name (not a tag) is never pluralized; the lowercase
        // dungeon-tag prefix is stripped for display.
        let boss = Cond {
            cond_type: 1,
            target: "shtrs Twilight Archmage".to_string(),
            amount: 2,
            time: None,
        };
        assert_eq!(boss.objective_phrase(), "Defeat 2 Twilight Archmage");

        // Wind Vortex kills get bespoke "in dungeons" wording, pluralized by count.
        let vortex = Cond {
            cond_type: 1,
            target: "Wind Vortex".to_string(),
            amount: 6,
            time: None,
        };
        assert_eq!(
            vortex.objective_phrase(),
            "Defeat 6 Wind Vortexes in dungeons"
        );

        let dungeon = Cond {
            cond_type: 3,
            target: "Ocean Trench".to_string(),
            amount: 3,
            time: None,
        };
        assert_eq!(dungeon.objective_phrase(), "Complete 3 Ocean Trench");

        // Difficulty objectives read as dungeon completions (matches the game's
        // "Complete N dungeons Difficulty ..." wording), pluralized by count.
        let diff = Cond {
            cond_type: 12,
            target: "7-10".to_string(),
            amount: 3,
            time: None,
        };
        assert_eq!(
            diff.objective_phrase(),
            "Complete 3 Dungeons Difficulty 7-10"
        );
        let one_diff = Cond {
            cond_type: 12,
            target: "5.5-10".to_string(),
            amount: 1,
            time: None,
        };
        assert_eq!(
            one_diff.objective_phrase(),
            "Complete 1 Dungeon Difficulty 5.5-10"
        );

        // EXP-gain objectives (type 15) read as "Gain N EXP" rather than the
        // generic "Complete N objective" fallback.
        let xp = Cond {
            cond_type: 15,
            target: String::new(),
            amount: 18050,
            time: None,
        };
        assert_eq!(xp.objective_phrase(), "Gain 18050 EXP");
        assert_eq!(xp.objective_label(), "Gain EXP");

        // Multiple objectives collapse to a concise header (the per-objective
        // bars already show each option), matching the game's "ONE OF BELOW:".
        let one_of = MissionDef {
            cond_spread: Some("11".to_string()),
            conds: vec![kill.clone(), dungeon.clone()],
            ..Default::default()
        };
        assert_eq!(one_of.objective_description(), "Complete one of the below");
        let all = MissionDef {
            conds: vec![kill.clone(), dungeon],
            ..Default::default()
        };
        assert_eq!(all.objective_description(), "Complete all of the below");
        // A single objective keeps its full, informative phrase.
        let single = MissionDef {
            conds: vec![kill],
            ..Default::default()
        };
        assert_eq!(single.objective_description(), "Defeat 5 Adept Encounters");
        // No structured objectives -> empty, so callers fall back to `desc`.
        assert_eq!(MissionDef::default().objective_description(), "");
    }

    #[test]
    fn detects_unknown_cond_types() {
        // Known types decode cleanly; only genuinely new type ids are flagged.
        let known = ClientSeasons {
            seasons: vec![Season {
                missions: vec![MissionDef {
                    conds: vec![
                        Cond {
                            cond_type: 1,
                            target: "X".into(),
                            amount: 1,
                            time: None,
                        },
                        Cond {
                            cond_type: 12,
                            target: "7-10".into(),
                            amount: 3,
                            time: None,
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        assert!(known.unknown_cond_types().is_empty());

        let with_new = ClientSeasons {
            seasons: vec![Season {
                missions: vec![MissionDef {
                    conds: vec![
                        Cond {
                            cond_type: 3,
                            target: "Ocean Trench".into(),
                            amount: 1,
                            time: None,
                        },
                        Cond {
                            cond_type: 99,
                            target: "".into(),
                            amount: 2,
                            time: None,
                        },
                        Cond {
                            cond_type: 99,
                            target: "".into(),
                            amount: 1,
                            time: None,
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        assert_eq!(with_new.unknown_cond_types(), vec![99]);
    }

    #[test]
    fn one_of_grouping() {
        let cs = parsed();
        let s = cs.season(37).unwrap();
        // condSpread "11": both conds are alternatives in a single OR group.
        let m9 = s.mission(9).unwrap();
        assert!(m9.is_one_of());
        assert_eq!(m9.cond_groups(), vec![vec![0, 1]]);
        // No spread: every cond required -> one single-member group each (AND).
        let m5 = s.mission(5).unwrap();
        assert!(!m5.is_one_of());
        assert_eq!(m5.cond_groups(), vec![vec![0], vec![1]]);
    }

    #[test]
    fn completion_one_of_vs_all() {
        let cs = parsed();
        let s = cs.season(37).unwrap();
        // Mission 9 one-of (need 2 of Sprite World OR Magic Woods).
        let m9 = s.mission(9).unwrap();
        assert!(m9.is_complete(&[2, 0])); // first alt met
        assert!(m9.is_complete(&[0, 2])); // second alt met
        assert!(!m9.is_complete(&[1, 1])); // neither alt fully met
                                           // Mission 5 all-required (both timed conds).
        let m5 = s.mission(5).unwrap();
        assert!(m5.is_complete(&[1, 1]));
        assert!(!m5.is_complete(&[1, 0]));
        // Short/missing progress array treated as zeros.
        assert!(!m5.is_complete(&[1]));
    }

    #[test]
    fn prog_string_parsing() {
        // Bare heartbeat -> nothing.
        assert!(parse_prog_string("pool/#").is_empty());
        // Live single delta with pool prefix.
        let live = parse_prog_string("pool/#prog/52,15,0:1:0:0");
        assert_eq!(
            live,
            vec![ProgEntry {
                season_id: 52,
                mission_id: 15,
                values: vec![0, 1, 0, 0]
            }]
        );
        // Login sync: multiple '|'-joined single-value entries, no pool prefix.
        let sync = parse_prog_string("prog/52,29,0|52,17,0");
        assert_eq!(sync.len(), 2);
        assert_eq!(
            sync[0],
            ProgEntry {
                season_id: 52,
                mission_id: 29,
                values: vec![0]
            }
        );
        assert_eq!(
            sync[1],
            ProgEntry {
                season_id: 52,
                mission_id: 17,
                values: vec![0]
            }
        );
        // Non-prog key and malformed entries yield nothing.
        assert!(parse_prog_string("pool/#other/1,2,3").is_empty());
        assert!(parse_prog_string("prog/52,abc,0").is_empty());
        assert!(parse_prog_string("prog/52,15").is_empty());
    }

    #[test]
    fn player_missions_parsing() {
        // Trimmed real capture: season attr, one single-value + one colon-array
        // mission, plus the unrelated pool/tree sections that must be ignored.
        let body = concat!(
            "<Missions><ActivePoolMissions>50:55:103:1|</ActivePoolMissions>",
            "<LockedMissions>;31,5560</LockedMissions>",
            "<UnlockedMissions>;31,5561</UnlockedMissions>",
            "<TreesUnlocked>52</TreesUnlocked>",
            "<Season id=\"52\">1,75,2,1786974807,1,1785972084|",
            "6,5,1,1786974807,10,1785972084|",
            "15,0:4:0:0,3,1786976879,10,1786976953|",
            "33,1,0,1786977397,9,1786907281</Season></Missions>"
        );
        let pm = parse_player_missions(body);
        assert_eq!(pm.season_id, Some(52));
        assert_eq!(pm.pool_ts, Some(1));
        assert_eq!(pm.missions.len(), 4);

        // F3=2 => claimed one-time (previously misread as claimable).
        let m1 = pm.mission(1).unwrap();
        assert_eq!(m1.progress, vec![75]);
        assert_eq!(m1.state, MissionState::Claimed);

        // F3=1 => genuinely claimable now.
        let m6 = pm.mission(6).unwrap();
        assert_eq!(m6.state, MissionState::Claimable);

        // F3=3 => claimed repeatable (on cooldown).
        let m15 = pm.mission(15).unwrap();
        assert_eq!(m15.progress, vec![0, 4, 0, 0]);
        assert_eq!(m15.state, MissionState::Claimed);
        // Last CSV field is the claimed-at timestamp used to anchor the reset.
        assert_eq!(m15.claimed_at, Some(1786976953));
        assert_eq!(pm.mission(1).unwrap().claimed_at, Some(1785972084));

        let m33 = pm.mission(33).unwrap();
        assert_eq!(m33.progress, vec![1]);
        assert_eq!(m33.state, MissionState::InProgress);

        // Tree-level lock state is now modeled.
        assert_eq!(pm.trees_unlocked, vec![52]);
        assert_eq!(pm.locked_missions, vec![(31, 5560)]);
        assert_eq!(pm.unlocked_missions, vec![(31, 5561)]);
        // Season-block missions are tagged with their tree id.
        assert_eq!(pm.mission(1).unwrap().tree_id, 52);
    }

    #[test]
    fn player_missions_parses_multiple_tree_blocks() {
        // A battlepass exposing two trees as separate <Season> blocks: each
        // mission is tagged with its own tree id, and the lock lists cover a
        // third, not-fully-unlocked tree.
        let body = concat!(
            "<Missions><TreesUnlocked>52,53</TreesUnlocked>",
            "<LockedMissions>;31,5555;31,5556</LockedMissions>",
            "<UnlockedMissions>;31,5561</UnlockedMissions>",
            "<Season id=\"52\">1,75,2</Season>",
            "<Season id=\"53\">200,3,0</Season></Missions>",
        );
        let pm = parse_player_missions(body);
        assert_eq!(pm.trees_unlocked, vec![52, 53]);
        assert_eq!(pm.locked_missions, vec![(31, 5555), (31, 5556)]);
        assert_eq!(pm.unlocked_missions, vec![(31, 5561)]);
        assert_eq!(pm.missions.len(), 2);
        assert_eq!(pm.mission(1).unwrap().tree_id, 52);
        assert_eq!(pm.mission(200).unwrap().tree_id, 53);
        // Primary season anchors to the first block.
        assert_eq!(pm.season_id, Some(52));
    }

    #[test]
    fn player_missions_falls_back_to_trees_unlocked() {
        let body = "<Missions><TreesUnlocked>52</TreesUnlocked></Missions>";
        let pm = parse_player_missions(body);
        assert_eq!(pm.season_id, Some(52));
        assert!(pm.missions.is_empty());
    }

    #[test]
    fn empty_body_is_error_but_empty_seasons_ok() {
        assert!(parse_client_seasons("").is_err());
        let cs = parse_client_seasons("{}").expect("missing seasons defaults to empty");
        assert!(cs.seasons.is_empty());
    }
}

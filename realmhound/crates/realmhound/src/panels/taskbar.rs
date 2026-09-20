//! Live Feed Taskbar projection.
//!
//! Turns the two heterogeneous progress sources -- seasonal **missions**
//! (`MissionView`) and Daily Quest Chest **quests** (`QuestData`) -- into one
//! uniform [`TaskbarTask`] list the Live Feed page can render as progress pills.
//!
//! The builders here are pure so they can be unit-tested without a UI: they take
//! already-resolved data (mission views, quest requirements, owned-mark counts)
//! and return display-ready tasks. Icon artwork is carried as an
//! [`ObjIcon`](crate::panels::missions::ObjIcon) so the renderer can reuse the
//! mission cards' `draw_obj_tile`. Ordering and lifecycle live in later stages.

use realmhound_core::api::MissionState;
use realmhound_core::protocol::data::QuestData;

use realmhound_core::assets::{
    dungeon_difficulty, dungeon_for_mark_name, get_asset_manager, get_dungeon_portal_map,
};

use crate::panels::missions::{
    completion_fraction, mission_eligible, mission_fallback_icon, obj_icon, on_cooldown,
    CurrentChar, ObjIcon,
};
use crate::processing::{MissionEntryView, ObjectiveKind, ObjectiveView};

/// One objective surfaced as a pill chip: its artwork, optional qualifier and
/// how many targets remain. A combined (AND) mission surfaces one chip per
/// still-incomplete objective; an unmade choice (OR) mission surfaces one chip
/// per variant (rendered `|`-separated).
#[derive(Clone)]
pub struct MissionObj {
    /// Resolved artwork (portal/boss/grave/oryx/fallback).
    pub icon: ObjIcon,
    /// A short qualifier shown *before* the icon: the grave-difficulty range
    /// (e.g. `7-10`), so it reads as a property of the grave tier instead of
    /// merging with the `x N` count into a single "7-10 x 27" formula. Empty
    /// for non-difficulty objectives.
    pub prefix: String,
    /// A short qualifier shown between the icon and the `x N` count:
    /// `veteran`/`adept` for encounter-kill objectives. Empty otherwise.
    pub qualifier: String,
    /// Targets still to go on this objective (`need - have`, min 0).
    pub remaining: i32,
}

/// A mission projected onto the Taskbar: every objective still worth showing
/// (all incomplete AND steps, or all not-yet-chosen OR variants) plus an
/// overall completion fraction for ordering.
#[derive(Clone)]
pub struct MissionTask {
    /// One chip per objective the pill should surface.
    pub objs: Vec<MissionObj>,
    /// Objectives (indices into `entry.objectives`) the hover tooltip should
    /// reproduce: after a choice is made only the progressed variant(s),
    /// otherwise every objective.
    pub tooltip_objs: Vec<usize>,
    /// True for an unmade choice (OR) mission: the pill renders its chips
    /// `|`-separated as mutually-exclusive variants.
    pub choice: bool,
    /// Overall mission completion (averaged across objectives) for ordering.
    pub fraction: f32,
    /// Whether the live character can make progress on this mission.
    pub eligible: bool,
    /// Full source entry, kept so the hover tooltip can reproduce the card.
    pub entry: MissionEntryView,
    /// Live character, kept so the tooltip can draw the sprite + qualifier.
    pub current_char: Option<CurrentChar>,
}

/// One distinct mark type a quest still needs, with how many remain on the
/// tracked storage side. A quest may require several mark types at once, so the
/// pill renders one `[icon] x N` group per entry.
#[derive(Clone)]
pub struct QuestMark {
    pub mark_id: i32,
    pub remaining: i32,
}

/// A Daily Quest Chest projected onto the Taskbar: how many Marks of Oryx remain
/// on the relevant storage side (seasonal or regular per the user setting).
#[derive(Clone)]
pub struct QuestTask {
    /// Mark sprite object id (first requirement) for the pill icon.
    pub mark_id: i32,
    /// Which storage side the progress reflects (`true` = seasonal marks).
    pub seasonal: bool,
    /// Marks collected on the shown side (capped at `need`).
    pub have: i32,
    /// Total marks the quest requires.
    pub need: i32,
    /// Marks still to go on the shown side (`need - have`, min 0).
    pub remaining: i32,
    /// Per-mark-type remaining counts (only types with progress left), in
    /// requirement order. The pill shows one `[icon] x N` group per entry.
    pub marks: Vec<QuestMark>,
    /// Full source quest, kept so the hover tooltip can reproduce the card.
    pub quest: QuestData,
    /// Owned `(seasonal, regular)` counts per required id, for the tooltip's
    /// collected columns.
    pub item_counts: std::collections::HashMap<i32, (u32, u32)>,
}

/// One Taskbar entry: either a mission or a quest.
#[derive(Clone)]
pub enum TaskbarTask {
    Mission(MissionTask),
    Quest(QuestTask),
}

impl TaskbarTask {
    /// Progress fraction in `0.0..=1.0`, used for ordering.
    pub fn fraction(&self) -> f32 {
        match self {
            TaskbarTask::Mission(m) => m.fraction,
            TaskbarTask::Quest(q) => {
                if q.need > 0 {
                    (q.have as f32 / q.need as f32).clamp(0.0, 1.0)
                } else {
                    0.0
                }
            }
        }
    }

    /// Whether the live character can make progress on this task. Quests apply
    /// to any character, so they are always eligible.
    pub fn eligible(&self) -> bool {
        match self {
            TaskbarTask::Mission(m) => m.eligible,
            TaskbarTask::Quest(_) => true,
        }
    }
}

/// The short qualifier a mission pill shows between its icon and the `x N`
/// count: encounter-kill objectives surface their tier (`veteran`/`adept`).
/// Difficulty ranges are handled by [`pill_prefix`] instead (rendered before
/// the grave icon). Everything else has no qualifier.
fn pill_qualifier(obj: &ObjectiveView) -> String {
    match obj.target.as_str() {
        "VETERAN_ENCOUNTER" => "veteran".to_string(),
        "ADEPT_ENCOUNTER" => "adept".to_string(),
        _ => String::new(),
    }
}

/// The qualifier a difficulty-range mission pill shows *before* its grave icon
/// (e.g. `7-10`), so the range reads as a property of the grave tier rather
/// than merging with the trailing `x N` count into a single "7-10 x 27"
/// formula. Empty for non-difficulty objectives.
fn pill_prefix(obj: &ObjectiveView) -> String {
    if matches!(obj.kind, ObjectiveKind::Difficulty) {
        obj.target.trim().to_string()
    } else {
        String::new()
    }
}

/// Project one in-progress mission onto a [`MissionTask`]. Returns `None` when
/// the mission has no incomplete objective left to surface (every step done, or
/// placeholder rows before defs load).
pub fn build_mission_task(
    e: &MissionEntryView,
    current_char: Option<CurrentChar>,
    show_all_choice: bool,
) -> Option<MissionTask> {
    let fallback = mission_fallback_icon(e);
    let incomplete = |o: &ObjectiveView| o.need > 0 && o.have < o.need;

    // A choice (OR) mission counts as "decided" once any variant has progress:
    // from then on only that variant is relevant. Combined (AND) missions and
    // not-yet-decided choices surface every objective. When the user opts to
    // always show all options, a choice mission is never treated as decided.
    let choice_made =
        e.one_of && !show_all_choice && e.objectives.iter().any(|o| o.need > 0 && o.have > 0);
    let tooltip_objs: Vec<usize> = if choice_made {
        leading_objs(e)
    } else {
        (0..e.objectives.len()).collect()
    };

    // Pill chips: the still-incomplete relevant objectives.
    let objs: Vec<MissionObj> = tooltip_objs
        .iter()
        .filter(|&&i| incomplete(&e.objectives[i]))
        .map(|&i| {
            let o = &e.objectives[i];
            MissionObj {
                icon: obj_icon(o, &fallback),
                prefix: pill_prefix(o),
                qualifier: pill_qualifier(o),
                remaining: (o.need - o.have).max(0),
            }
        })
        .collect();
    if objs.is_empty() {
        return None;
    }

    Some(MissionTask {
        objs,
        // An OR mission renders its variants with `OR` + a `|` separator
        // whenever more than one alternative is on display -- including the
        // collapsed case where several variants are tied on progress. Only a
        // single surviving variant drops the separator.
        choice: e.one_of && tooltip_objs.len() > 1,
        tooltip_objs,
        fraction: completion_fraction(e),
        eligible: mission_eligible(current_char, e.participants, &e.worn_restriction),
        entry: e.clone(),
        current_char,
    })
}

/// Project one Daily Quest Chest onto a [`QuestTask`]. `owned` returns the
/// `(seasonal, regular)` owned count for a given required item id. When
/// `prioritize_seasonal` is set the pill always reflects seasonal marks (even
/// if the regular marks are already collected); otherwise it reflects regular.
pub fn build_quest_task(
    quest: &QuestData,
    owned: impl Fn(i32) -> (u32, u32),
    prioritize_seasonal: bool,
) -> Option<QuestTask> {
    use std::collections::HashMap;
    let mark_id = *quest.requirements.first()?;
    let seasonal = prioritize_seasonal;
    let side = |id: i32| {
        let (s, r) = owned(id);
        (if seasonal { s } else { r }) as i32
    };

    // Distinct mark types in requirement order, with how many copies each needs.
    let mut order: Vec<i32> = Vec::new();
    let mut required: HashMap<i32, i32> = HashMap::new();
    for &id in &quest.requirements {
        if !required.contains_key(&id) {
            order.push(id);
        }
        *required.entry(id).or_insert(0) += 1;
    }

    let mut marks = Vec::new();
    let mut have = 0;
    let mut need = 0;
    for &id in &order {
        let req = required[&id];
        let collected = side(id).min(req);
        let rem = (req - collected).max(0);
        have += collected;
        need += req;
        if rem > 0 {
            marks.push(QuestMark {
                mark_id: id,
                remaining: rem,
            });
        }
    }

    let mut item_counts = HashMap::new();
    for &id in &quest.requirements {
        item_counts.entry(id).or_insert_with(|| owned(id));
    }
    Some(QuestTask {
        mark_id,
        seasonal,
        have,
        need,
        remaining: (need - have).max(0),
        marks,
        quest: quest.clone(),
        item_counts,
    })
}

// ---------------------------------------------------------------------------
// Smart combining: merge overlapping missions and mark quests, and
// link missions that progress together.
// ---------------------------------------------------------------------------

/// One dungeon variety inside a combined mission+quest pill. The Mission side
/// (`dungeon_runs` dungeon completions) and the Quest side (`mark_count` marks
/// to pick up) render as two separate columns. A locked pill has a single
/// variant; a tie shows several.
#[derive(Clone)]
pub struct CombinedVariant {
    pub dungeon_name: String,
    /// Portal artwork for the dungeon (the Mission column icon).
    pub portal_icon: ObjIcon,
    /// Mark sprite id (the Quest column icon).
    pub mark_id: i32,
    /// Dungeon completions the mission still needs (Mission column count).
    pub dungeon_runs: i32,
    /// Marks the quest still needs to pick up (Quest column count).
    pub mark_count: i32,
}

/// A mission fused with the mark quest(s) it overlaps. An *exact* overlap fuses
/// a dungeon mission with a same-dungeon mark quest; a *partial* overlap
/// (`partial = true`) reframes a broad grave-difficulty mission as the specific
/// dungeon of a matching mark quest.
#[derive(Clone)]
pub struct CombinedTask {
    pub eligible: bool,
    pub fraction: f32,
    /// Dungeon varieties shown on the pill (one when locked, several on a tie).
    pub variants: Vec<CombinedVariant>,
    /// True when the varieties are mutually-exclusive choices (rendered `|`).
    pub choice: bool,
    /// True for a grave-difficulty reframe (feature 2) vs an exact merge.
    pub partial: bool,
    pub entry: MissionEntryView,
    pub current_char: Option<CurrentChar>,
    /// Source quests, kept so the hover tooltip can reproduce their cards.
    pub quests: Vec<QuestData>,
    /// Which storage side (seasonal vs regular) the merge counted marks on, so
    /// the tooltip can hide dungeons whose mark is already fully collected.
    pub seasonal: bool,
    pub item_counts: std::collections::HashMap<i32, (u32, u32)>,
}

/// One pill in a chain: the task plus whether it should be dimmed (a cooldown
/// partner, or an active pill waiting on one). A chain is usually all missions,
/// but a mark quest waiting on a cooldown mission pairs a bright quest with a
/// dimmed mission.
#[derive(Clone)]
pub struct ChainMember {
    pub task: TaskbarTask,
    pub dimmed: bool,
}

/// Two or more missions that progress together, rendered adjacent and joined by
/// a chain-link glyph so the player runs them in parallel.
#[derive(Clone)]
pub struct ChainGroup {
    pub members: Vec<ChainMember>,
    pub eligible: bool,
    pub fraction: f32,
}

impl ChainGroup {
    /// True while any member is inactive (on cooldown or char-ineligible): the
    /// chain is pushed right so the player doesn't progress one half without the
    /// other. Individual pills dim themselves; active members still render
    /// bright.
    pub fn dimmed(&self) -> bool {
        self.members.iter().any(|m| m.dimmed)
    }
}

/// A processed Taskbar entry: a plain task, a fused mission+quest, or a chain of
/// linked missions.
#[derive(Clone)]
pub enum TaskbarItem {
    Single(TaskbarTask),
    Combined(CombinedTask),
    Chain(ChainGroup),
}

impl TaskbarItem {
    /// Whether the live character can currently progress this item (bright vs
    /// dimmed). A chain waiting on a cooldown partner is not yet actionable.
    pub fn eligible(&self) -> bool {
        match self {
            TaskbarItem::Single(t) => t.eligible(),
            TaskbarItem::Combined(c) => c.eligible,
            TaskbarItem::Chain(g) => g.eligible && !g.dimmed(),
        }
    }

    /// Progress fraction for ordering (more-progressed sorts first).
    pub fn fraction(&self) -> f32 {
        match self {
            TaskbarItem::Single(t) => t.fraction(),
            TaskbarItem::Combined(c) => c.fraction,
            TaskbarItem::Chain(g) => g.fraction,
        }
    }

    /// Items pushed to the far right and dimmed: ineligible tasks, and chains
    /// still blocked by a cooldown partner.
    pub fn dim_right(&self) -> bool {
        match self {
            TaskbarItem::Chain(g) => g.dimmed() || !g.eligible,
            other => !other.eligible(),
        }
    }

    /// A standalone mark quest (not fused with a mission). Used as an ordering
    /// tie-breaker: on equal progress, missions sit ahead of (left of) quests.
    pub fn is_mark_quest(&self) -> bool {
        matches!(self, TaskbarItem::Single(TaskbarTask::Quest(_)))
    }
}

/// Portal artwork for a dungeon by name (`None` when the dungeon is unmapped).
fn dungeon_portal_icon(dungeon: &str) -> ObjIcon {
    match get_dungeon_portal_map().get_portal_id(dungeon) {
        Some(id) if id > 0 => ObjIcon::Object(id),
        _ => ObjIcon::None,
    }
}

/// The single dungeon a mark quest's marks all come from, or `None` when the
/// quest mixes marks from several dungeons (or none resolve to a dungeon).
fn quest_dungeon(q: &QuestTask) -> Option<String> {
    let am = get_asset_manager();
    let mut ids: Vec<i32> = Vec::new();
    for &id in &q.quest.requirements {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    let mut dungeon: Option<String> = None;
    for id in ids {
        let name = am.object_name(id)?;
        let dn = dungeon_for_mark_name(&name)?;
        match &dungeon {
            Some(prev) if prev != dn => return None,
            _ => dungeon = Some(dn.to_string()),
        }
    }
    dungeon
}

/// Reduce a dungeon name to the base form the mark table uses, so a mission's
/// dungeon objective matches a mark quest's resolved dungeon. Mark quests resolve
/// to the *base* dungeon (e.g. the Advanced Control Core mark maps to "Kogbold
/// Steamworks"), while missions target the specific variant ("Advanced Kogbold
/// Steamworks", "Plagued Nest"). Case-insensitive; returns a lowercase key.
pub(crate) fn canonical_match_dungeon(name: &str) -> String {
    let base = name.trim().strip_prefix("Advanced ").unwrap_or(name.trim());
    let base = if base.eq_ignore_ascii_case("Plagued Nest") {
        "The Nest"
    } else {
        base
    };
    base.to_ascii_lowercase()
}

/// True when a mission's dungeon objective and a mark quest's resolved dungeon
/// refer to the same dungeon (handling Advanced/Plagued variants).
fn dungeon_names_match(mission_dungeon: &str, quest_dungeon: &str) -> bool {
    canonical_match_dungeon(mission_dungeon) == canonical_match_dungeon(quest_dungeon)
}

/// Parse a grave-difficulty objective target (`"7-10"` or `"5"`) into `(lo, hi)`.
fn parse_difficulty_range(target: &str) -> Option<(f32, f32)> {
    let t = target.trim();
    match t.split_once('-') {
        Some((a, b)) => Some((a.trim().parse().ok()?, b.trim().parse().ok()?)),
        None => {
            let v: f32 = t.parse().ok()?;
            Some((v, v))
        }
    }
}

/// The most-progressed variant(s) of a choice (OR) mission: the objectives tied
/// at the highest completion fraction among those with any progress. Collapsing
/// a decided OR mission to these keeps a single leader when one is clearly ahead
/// (e.g. 3/4 Cultist vs 1/4 Nest -> just Cultist) but preserves every tied
/// option so genuine ties still render as a choice.
fn leading_objs(e: &MissionEntryView) -> Vec<usize> {
    let frac = |o: &ObjectiveView| {
        if o.need > 0 {
            o.have as f32 / o.need as f32
        } else {
            0.0
        }
    };
    let max = e
        .objectives
        .iter()
        .filter(|o| o.need > 0 && o.have > 0)
        .map(&frac)
        .fold(0.0_f32, f32::max);
    e.objectives
        .iter()
        .enumerate()
        .filter(|(_, o)| o.need > 0 && o.have > 0 && (frac(o) - max).abs() < 1e-4)
        .map(|(i, _)| i)
        .collect()
}

/// The relevant objective indices of a mission pill: after a choice is decided,
/// only the progressed variant(s); otherwise every objective. Mirrors
/// [`build_mission_task`]. `show_all_choice` keeps every variant even after a
/// choice mission has in-game progress.
fn relevant_objs(e: &MissionEntryView, show_all_choice: bool) -> Vec<usize> {
    let choice_made =
        e.one_of && !show_all_choice && e.objectives.iter().any(|o| o.need > 0 && o.have > 0);
    if choice_made {
        leading_objs(e)
    } else {
        (0..e.objectives.len()).collect()
    }
}

/// Incomplete relevant objective indices (the chips the pill actually shows).
fn incomplete_objs(e: &MissionEntryView, show_all_choice: bool) -> Vec<usize> {
    relevant_objs(e, show_all_choice)
        .into_iter()
        .filter(|&i| e.objectives[i].need > 0 && e.objectives[i].have < e.objectives[i].need)
        .collect()
}

/// True when objective `o` would advance a grave-difficulty range `[lo, hi]`:
/// a dungeon in range, or an overlapping difficulty objective.
fn objective_progresses_range(o: &ObjectiveView, lo: f32, hi: f32) -> bool {
    match o.kind {
        ObjectiveKind::Dungeon => o
            .dungeon_name
            .as_deref()
            .and_then(dungeon_difficulty)
            .is_some_and(|d| d >= lo && d <= hi),
        ObjectiveKind::Difficulty => {
            parse_difficulty_range(&o.target).is_some_and(|(l, h)| l <= hi && h >= lo)
        }
        _ => false,
    }
}

/// Whether two objectives target the *exact same* thing, so every run counts for
/// both: the same dungeon, an identical grave-difficulty range, or the same named
/// boss. Deliberately strict -- mere range overlap or a dungeon that merely falls
/// inside a broad band does NOT qualify, otherwise a couple of wide "bucket"
/// missions (e.g. `5.5-10 x50`, `7-10 x17`) would act as hubs and drag every
/// unrelated mission into one giant chain.
fn same_chain_criteria(oa: &ObjectiveView, ob: &ObjectiveView) -> bool {
    match (oa.kind, ob.kind) {
        (ObjectiveKind::Dungeon, ObjectiveKind::Dungeon) => {
            match (&oa.dungeon_name, &ob.dungeon_name) {
                (Some(x), Some(y)) => x.eq_ignore_ascii_case(y),
                _ => false,
            }
        }
        (ObjectiveKind::Difficulty, ObjectiveKind::Difficulty) => {
            match (
                parse_difficulty_range(&oa.target),
                parse_difficulty_range(&ob.target),
            ) {
                (Some((la, ha)), Some((lb, hb))) => {
                    (la - lb).abs() < f32::EPSILON && (ha - hb).abs() < f32::EPSILON
                }
                _ => false,
            }
        }
        (ObjectiveKind::KillNamed, ObjectiveKind::KillNamed) => {
            // Wind Vortex kills are a shared pseudo-target with no enemy sprite,
            // so boss_id never resolves; match them by their vortex identity so
            // "Defeat N Wind Vortexes in dungeons" missions cross-progress.
            if realmhound_core::api::is_vortex_target(&oa.target)
                && realmhound_core::api::is_vortex_target(&ob.target)
            {
                return true;
            }
            matches!((oa.boss_id, ob.boss_id), (Some(x), Some(y)) if x == y && x > 0)
        }
        _ => false,
    }
}

/// Whether two missions should be chained: they share an objective that is
/// *still outstanding in both*, so completing runs for one always advances the
/// other. Objectives already satisfied for a mission (`have >= need`) are
/// ignored -- a shared but already-completed target (e.g. a dungeon one mission
/// has finished) must not link the two, since running it no longer helps that
/// mission. This keeps chains to genuine "do these together" pairs.
fn missions_chain(a: &MissionEntryView, b: &MissionEntryView) -> bool {
    let outstanding = |o: &&ObjectiveView| o.need > 0 && o.have < o.need;
    for oa in a.objectives.iter().filter(outstanding) {
        for ob in b.objectives.iter().filter(outstanding) {
            if same_chain_criteria(oa, ob) {
                return true;
            }
        }
    }
    false
}

/// Build a mission task for a cooldown mission whose objectives have reset:
/// every objective is shown at its full requirement so the dimmed chain partner
/// still reads correctly. Returns `None` when nothing resolves to a chip.
fn build_cooldown_task(
    e: &MissionEntryView,
    current_char: Option<CurrentChar>,
) -> Option<MissionTask> {
    let fallback = mission_fallback_icon(e);
    let objs: Vec<MissionObj> = e
        .objectives
        .iter()
        .filter(|o| o.need > 0)
        .map(|o| MissionObj {
            icon: obj_icon(o, &fallback),
            prefix: if matches!(o.kind, ObjectiveKind::Difficulty) {
                o.target.trim().to_string()
            } else {
                String::new()
            },
            qualifier: pill_qualifier(o),
            remaining: o.need,
        })
        .collect();
    if objs.is_empty() {
        return None;
    }
    Some(MissionTask {
        objs,
        tooltip_objs: (0..e.objectives.len()).collect(),
        choice: e.one_of,
        fraction: 0.0,
        eligible: mission_eligible(current_char, e.participants, &e.worn_restriction),
        entry: e.clone(),
        current_char,
    })
}

/// Build a cooldown mission task locked onto a single objective (the dungeon a
/// waiting mark quest matches). Only that objective is surfaced, at its full
/// requirement, so a multi-choice mission reads as "run this dungeon" instead
/// of listing every alternative. Returns `None` when the objective is empty.
fn build_locked_cooldown_task(
    e: &MissionEntryView,
    oi: usize,
    current_char: Option<CurrentChar>,
) -> Option<MissionTask> {
    let fallback = mission_fallback_icon(e);
    let o = e.objectives.get(oi)?;
    if o.need <= 0 {
        return None;
    }
    let obj = MissionObj {
        icon: obj_icon(o, &fallback),
        prefix: if matches!(o.kind, ObjectiveKind::Difficulty) {
            o.target.trim().to_string()
        } else {
            String::new()
        },
        qualifier: pill_qualifier(o),
        remaining: o.need,
    };
    Some(MissionTask {
        objs: vec![obj],
        tooltip_objs: vec![oi],
        choice: false,
        fraction: 0.0,
        eligible: mission_eligible(current_char, e.participants, &e.worn_restriction),
        entry: e.clone(),
        current_char,
    })
}

/// A quest matched to a mission dungeon variety during combining.
struct QuestMatch {
    quest_idx: usize,
    dungeon: String,
    mission_remaining: i32,
    mark_id: i32,
    mark_remaining: i32,
    /// Marks already collected on the tracked side (the headstart used to pick
    /// a winner among tied choice variants).
    headstart: i32,
}

/// Build one combined variety from a mission/quest match.
fn variant_from_match(m: &QuestMatch) -> CombinedVariant {
    CombinedVariant {
        dungeon_name: m.dungeon.clone(),
        portal_icon: dungeon_portal_icon(&m.dungeon),
        mark_id: m.mark_id,
        dungeon_runs: m.mission_remaining.max(0),
        mark_count: m.mark_remaining.max(0),
    }
}

/// Fold matched quests' cards and owned-count maps into a combined task's
/// tooltip inputs.
fn combine_quest_inputs(
    quests: &[QuestTask],
    idxs: impl Iterator<Item = usize>,
) -> (Vec<QuestData>, std::collections::HashMap<i32, (u32, u32)>) {
    let mut datas = Vec::new();
    let mut counts = std::collections::HashMap::new();
    for i in idxs {
        let q = &quests[i];
        datas.push(q.quest.clone());
        for (&k, &v) in &q.item_counts {
            counts.entry(k).or_insert(v);
        }
    }
    (datas, counts)
}

/// Fuse tracked missions and mark quests into smart Taskbar items:
///
/// 1. **Exact overlap** -- a dungeon mission and a same-dungeon mark quest merge
///    into one pill split into portal-only and mark-pickup runs. Choice (OR)
///    missions lock onto the mark quest with the biggest headstart, or stay a
///    choice pill when tied.
/// 2. **Partial overlap** -- a broad grave-difficulty mission is reframed as a
///    matching mark quest's dungeon, but only when the mission needs no more
///    runs than the quest and no other tracked mission already advances it.
/// 3. **Chains** -- missions that progress together are linked; a partner still
///    on cooldown is shown dimmed alongside so the player waits for both.
/// 4. **Quest waiting on a cooldown mission** -- a mark quest whose dungeon a
///    cooldown mission will run is linked to that dimmed, choice-locked mission
///    so the player waits to run the dungeon once instead of collecting the
///    marks early.
pub fn build_taskbar_items(
    mission_view: &crate::processing::MissionView,
    tracked: impl Fn(i64) -> bool,
    quest_tasks: Vec<QuestTask>,
    current_char: Option<CurrentChar>,
    show_all_choice_options: bool,
) -> Vec<TaskbarItem> {
    // In-progress tracked missions become mission tasks; tracked missions on
    // cooldown are candidate chain partners shown dimmed.
    let mission_tasks: Vec<MissionTask> = mission_view
        .entries
        .iter()
        .filter(|e| e.defs_loaded && e.state == MissionState::InProgress)
        .filter(|e| tracked(e.uid()))
        .filter_map(|e| build_mission_task(e, current_char, show_all_choice_options))
        .collect();
    let cooldown_entries: Vec<MissionEntryView> = mission_view
        .entries
        .iter()
        .filter(|e| e.defs_loaded && on_cooldown(e))
        .filter(|e| tracked(e.uid()))
        .cloned()
        .collect();
    let cooldown_entries = cooldown_entries.as_slice();

    let mut consumed_mission = vec![false; mission_tasks.len()];
    let mut consumed_quest = vec![false; quest_tasks.len()];
    let mut in_chain = vec![false; mission_tasks.len()];
    let mut items: Vec<TaskbarItem> = Vec::new();

    let quest_dungeons: Vec<Option<String>> = quest_tasks.iter().map(quest_dungeon).collect();
    let quest_seasonal = quest_tasks.first().map(|q| q.seasonal).unwrap_or(false);

    // --- Feature 1: exact dungeon<->mark overlaps -------------------------
    for mi in 0..mission_tasks.len() {
        let e = &mission_tasks[mi].entry;
        let inc = incomplete_objs(e, show_all_choice_options);
        // Only fuse when every remaining chip is a dungeon objective, so the
        // combined pill fully represents the mission.
        if inc.is_empty()
            || inc
                .iter()
                .any(|&i| !matches!(e.objectives[i].kind, ObjectiveKind::Dungeon))
        {
            continue;
        }

        // Match each dungeon variety to an available same-dungeon quest.
        let mut matches: Vec<QuestMatch> = Vec::new();
        for &oi in &inc {
            let o = &e.objectives[oi];
            let Some(dn) = o.dungeon_name.as_deref() else {
                continue;
            };
            let remaining = (o.need - o.have).max(0);
            for (qi, qd) in quest_dungeons.iter().enumerate() {
                if consumed_quest[qi] {
                    continue;
                }
                if qd.as_deref().is_some_and(|d| dungeon_names_match(dn, d)) {
                    matches.push(QuestMatch {
                        quest_idx: qi,
                        dungeon: dn.to_string(),
                        mission_remaining: remaining,
                        mark_id: quest_tasks[qi].mark_id,
                        mark_remaining: quest_tasks[qi].remaining,
                        headstart: quest_tasks[qi].have,
                    });
                }
            }
        }
        if matches.is_empty() {
            continue;
        }

        let is_choice = mission_tasks[mi].choice;
        let (variants, choice, used_quests): (Vec<CombinedVariant>, bool, Vec<usize>) = if is_choice
        {
            // Undecided OR mission: lock onto the biggest headstart, or keep a
            // choice pill when tied. With "show all choice options" enabled we
            // never collapse -- every matching variety stays on the pill.
            let max_head = matches.iter().map(|m| m.headstart).max().unwrap_or(0);
            let top: Vec<&QuestMatch> =
                matches.iter().filter(|m| m.headstart == max_head).collect();
            if !show_all_choice_options && top.len() == 1 {
                let m = top[0];
                (vec![variant_from_match(m)], false, vec![m.quest_idx])
            } else {
                let vs = matches.iter().map(variant_from_match).collect();
                let used = matches.iter().map(|m| m.quest_idx).collect();
                (vs, true, used)
            }
        } else {
            // AND / decided mission: one variety per dungeon chip, mark-fused
            // when a quest matches, portal-only otherwise.
            let mut vs = Vec::new();
            let mut used = Vec::new();
            for &oi in &inc {
                let o = &e.objectives[oi];
                let Some(dn) = o.dungeon_name.as_deref() else {
                    continue;
                };
                let remaining = (o.need - o.have).max(0);
                match matches.iter().find(|m| m.dungeon.eq_ignore_ascii_case(dn)) {
                    Some(m) => {
                        vs.push(variant_from_match(m));
                        used.push(m.quest_idx);
                    }
                    None => vs.push(CombinedVariant {
                        dungeon_name: dn.to_string(),
                        portal_icon: dungeon_portal_icon(dn),
                        mark_id: 0,
                        dungeon_runs: remaining,
                        mark_count: 0,
                    }),
                }
            }
            (vs, false, used)
        };

        consumed_mission[mi] = true;
        for &qi in &used_quests {
            consumed_quest[qi] = true;
        }
        let (quests, item_counts) = combine_quest_inputs(&quest_tasks, used_quests.into_iter());
        items.push(TaskbarItem::Combined(CombinedTask {
            eligible: mission_tasks[mi].eligible,
            fraction: mission_tasks[mi].fraction,
            variants,
            choice,
            partial: false,
            entry: mission_tasks[mi].entry.clone(),
            current_char: mission_tasks[mi].current_char,
            quests,
            seasonal: quest_seasonal,
            item_counts,
        }));
    }

    // --- Feature 2: partial overlap (grave difficulty reframe) ------------
    for mi in 0..mission_tasks.len() {
        if consumed_mission[mi] {
            continue;
        }
        let e = &mission_tasks[mi].entry;
        let inc = incomplete_objs(e, false);
        // Only a single, lone grave-difficulty objective qualifies.
        if inc.len() != 1 || !matches!(e.objectives[inc[0]].kind, ObjectiveKind::Difficulty) {
            continue;
        }
        let o = &e.objectives[inc[0]];
        let Some((lo, hi)) = parse_difficulty_range(&o.target) else {
            continue;
        };
        let mission_remaining = (o.need - o.have).max(0);

        // Candidate quests: dungeon lands in the difficulty band, mission needs
        // no more runs than the quest, and quest not already used.
        let mut candidates: Vec<QuestMatch> = Vec::new();
        for (qi, qd) in quest_dungeons.iter().enumerate() {
            if consumed_quest[qi] {
                continue;
            }
            let Some(dn) = qd.as_deref() else { continue };
            let Some(diff) = dungeon_difficulty(dn) else {
                continue;
            };
            if diff < lo || diff > hi {
                continue;
            }
            if mission_remaining > quest_tasks[qi].remaining {
                continue;
            }
            candidates.push(QuestMatch {
                quest_idx: qi,
                dungeon: dn.to_string(),
                mission_remaining,
                mark_id: quest_tasks[qi].mark_id,
                mark_remaining: quest_tasks[qi].remaining,
                headstart: quest_tasks[qi].have,
            });
        }
        if candidates.is_empty() {
            continue;
        }

        // Guard: skip if any other tracked mission already advances this band.
        let other_closes = mission_tasks.iter().enumerate().any(|(mj, other)| {
            mj != mi
                && incomplete_objs(&other.entry, false)
                    .iter()
                    .any(|&i| objective_progresses_range(&other.entry.objectives[i], lo, hi))
        });
        if other_closes {
            continue;
        }

        // Pick the biggest headstart.
        candidates.sort_by(|a, b| b.headstart.cmp(&a.headstart));
        let m = &candidates[0];
        let variant = variant_from_match(m);
        consumed_mission[mi] = true;
        consumed_quest[m.quest_idx] = true;
        let (quests, item_counts) =
            combine_quest_inputs(&quest_tasks, std::iter::once(m.quest_idx));
        items.push(TaskbarItem::Combined(CombinedTask {
            eligible: mission_tasks[mi].eligible,
            fraction: mission_tasks[mi].fraction,
            variants: vec![variant],
            choice: false,
            partial: true,
            entry: mission_tasks[mi].entry.clone(),
            current_char: mission_tasks[mi].current_char,
            quests,
            seasonal: quest_seasonal,
            item_counts,
        }));
    }

    // --- Feature 3: chains ------------------------------------------------
    // Union-find over the still-standalone missions.
    let n = mission_tasks.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while parent[r] != r {
            r = parent[r];
        }
        let mut c = x;
        while parent[c] != c {
            let next = parent[c];
            parent[c] = r;
            c = next;
        }
        r
    }
    for i in 0..n {
        if consumed_mission[i] {
            continue;
        }
        for j in (i + 1)..n {
            if consumed_mission[j] {
                continue;
            }
            if missions_chain(&mission_tasks[i].entry, &mission_tasks[j].entry) {
                let ri = find(&mut parent, i);
                let rj = find(&mut parent, j);
                if ri != rj {
                    parent[ri] = rj;
                }
            }
        }
    }
    // Group active missions by component; only >=2-member components chain.
    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> =
        std::collections::BTreeMap::new();
    for i in 0..n {
        if consumed_mission[i] {
            continue;
        }
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    let mut used_cooldown = vec![false; cooldown_entries.len()];
    for (_, members) in groups {
        if members.len() < 2 {
            continue;
        }
        for &mi in &members {
            in_chain[mi] = true;
        }
        let eligible = members.iter().all(|&mi| mission_tasks[mi].eligible);
        let fraction = members
            .iter()
            .map(|&mi| mission_tasks[mi].fraction)
            .fold(0.0_f32, f32::max);
        let mut chain_members: Vec<ChainMember> = members
            .iter()
            .map(|&mi| ChainMember {
                task: TaskbarTask::Mission(mission_tasks[mi].clone()),
                dimmed: !mission_tasks[mi].eligible,
            })
            .collect();
        // Active (bright) pills first, inactive (dimmed) second.
        chain_members.sort_by_key(|m| m.dimmed);
        items.push(TaskbarItem::Chain(ChainGroup {
            members: chain_members,
            eligible,
            fraction,
        }));
    }

    // A lone active mission may still chain with a cooldown partner: show both
    // so the player waits to run them together, but only dim the pill that is
    // actually inactive (the cooldown partner) -- the active pill keeps
    // progressing and stays bright. Active pill first, cooldown second.
    for mi in 0..n {
        if consumed_mission[mi] || in_chain[mi] {
            continue;
        }
        let partner = cooldown_entries
            .iter()
            .enumerate()
            .find(|(ci, ce)| !used_cooldown[*ci] && missions_chain(&mission_tasks[mi].entry, ce));
        let Some((ci, ce)) = partner else { continue };
        let Some(cooldown_task) = build_cooldown_task(ce, current_char) else {
            continue;
        };
        used_cooldown[ci] = true;
        in_chain[mi] = true;
        let eligible = mission_tasks[mi].eligible;
        let fraction = mission_tasks[mi].fraction;
        items.push(TaskbarItem::Chain(ChainGroup {
            members: vec![
                ChainMember {
                    task: TaskbarTask::Mission(mission_tasks[mi].clone()),
                    dimmed: !mission_tasks[mi].eligible,
                },
                ChainMember {
                    task: TaskbarTask::Mission(cooldown_task),
                    dimmed: true,
                },
            ],
            eligible,
            fraction,
        }));
    }

    // --- Feature 4: a mark quest waiting on a cooldown mission -------------
    // A tracked quest whose dungeon a cooldown mission will also run: link them
    // so the player waits to run the dungeon once (clearing marks and the
    // mission together) instead of collecting the marks early. The quest pill
    // stays bright; the mission pill is dimmed and locked onto the matching
    // objective. The active-mission cooldown pass above already claimed any
    // cooldown mission it needed (via `used_cooldown`).
    for qi in 0..quest_tasks.len() {
        if consumed_quest[qi] || quest_tasks[qi].remaining == 0 {
            continue;
        }
        let Some(qd) = quest_dungeons[qi].as_deref() else {
            continue;
        };
        let partner = cooldown_entries.iter().enumerate().find_map(|(ci, ce)| {
            if used_cooldown[ci] {
                return None;
            }
            let oi = ce.objectives.iter().position(|o| {
                o.need > 0
                    && o.dungeon_name
                        .as_deref()
                        .is_some_and(|dn| dungeon_names_match(dn, qd))
            })?;
            // Don't falsely imply a multi-objective AND mission is done by one
            // dungeon: only lock choice missions or single-objective missions.
            let active_objs = ce.objectives.iter().filter(|o| o.need > 0).count();
            if !ce.one_of && active_objs != 1 {
                return None;
            }
            Some((ci, oi))
        });
        let Some((ci, oi)) = partner else { continue };
        let Some(locked) = build_locked_cooldown_task(&cooldown_entries[ci], oi, current_char)
        else {
            continue;
        };
        used_cooldown[ci] = true;
        consumed_quest[qi] = true;
        let q = &quest_tasks[qi];
        let fraction = if q.need > 0 {
            (q.have as f32 / q.need as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        items.push(TaskbarItem::Chain(ChainGroup {
            members: vec![
                ChainMember {
                    task: TaskbarTask::Quest(q.clone()),
                    dimmed: false,
                },
                ChainMember {
                    task: TaskbarTask::Mission(locked),
                    dimmed: true,
                },
            ],
            eligible: false,
            fraction,
        }));
    }

    // --- Leftovers: plain single pills ------------------------------------
    for (mi, task) in mission_tasks.into_iter().enumerate() {
        if consumed_mission[mi] || in_chain[mi] {
            continue;
        }
        items.push(TaskbarItem::Single(TaskbarTask::Mission(task)));
    }
    for (qi, task) in quest_tasks.into_iter().enumerate() {
        if consumed_quest[qi] {
            continue;
        }
        items.push(TaskbarItem::Single(TaskbarTask::Quest(task)));
    }

    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processing::{MissionCategory, ObjectiveKind};

    fn obj(label: &str, have: i32, need: i32) -> ObjectiveView {
        ObjectiveView {
            label: label.to_string(),
            have,
            need,
            kind: ObjectiveKind::Other,
            dungeon_name: None,
            boss_id: None,
            target: String::new(),
        }
    }

    fn mission(id: i32, objs: Vec<ObjectiveView>) -> MissionEntryView {
        MissionEntryView {
            mission_id: id,
            tree_id: 0,
            tree_name: String::new(),
            name: format!("Mission {id}"),
            desc: String::new(),
            objectives: objs,
            rewards: vec!["1,000 BXP".to_string()],
            rewards_detail: Vec::new(),
            reward_groups: Vec::new(),
            category: MissionCategory::Seasonal,
            participants: 2,
            worn_restriction: Vec::new(),
            repeatable: false,
            icon_object_id: None,
            icon_ref: None,
            state: MissionState::InProgress,
            complete: false,
            reset_at: None,
            one_of: false,
            defs_loaded: true,
        }
    }

    #[test]
    fn and_mission_surfaces_every_incomplete_objective() {
        // Combined (AND) mission: the completed objective is dropped, both
        // incomplete ones are surfaced in source order.
        let e = mission(1, vec![obj("A", 1, 10), obj("B", 10, 10), obj("C", 0, 10)]);
        let t = build_mission_task(&e, None, false).unwrap();
        assert!(!t.choice);
        assert_eq!(t.objs.len(), 2);
        assert_eq!(t.objs[0].remaining, 9);
        assert_eq!(t.objs[1].remaining, 10);
        // The tooltip still reproduces every objective (incl. the finished one).
        assert_eq!(t.tooltip_objs, vec![0, 1, 2]);
    }

    #[test]
    fn choice_mission_before_pick_shows_all_variants() {
        let mut e = mission(2, vec![obj("A", 0, 4), obj("B", 0, 4), obj("C", 0, 4)]);
        e.one_of = true;
        let t = build_mission_task(&e, None, false).unwrap();
        assert!(t.choice);
        assert_eq!(t.objs.len(), 3);
        assert_eq!(t.tooltip_objs, vec![0, 1, 2]);
    }

    #[test]
    fn choice_mission_after_pick_shows_only_progressed_variant() {
        let mut e = mission(3, vec![obj("A", 0, 4), obj("B", 2, 4), obj("C", 0, 4)]);
        e.one_of = true;
        let t = build_mission_task(&e, None, false).unwrap();
        assert!(!t.choice);
        assert_eq!(t.objs.len(), 1);
        assert_eq!(t.objs[0].remaining, 2);
        assert_eq!(t.tooltip_objs, vec![1]);
    }

    #[test]
    fn choice_mission_after_pick_keeps_all_variants_when_show_all() {
        // With "show all options" on, an OR mission that already has in-game
        // progress must still surface every variant instead of collapsing.
        let mut e = mission(3, vec![obj("A", 0, 4), obj("B", 2, 4), obj("C", 0, 4)]);
        e.one_of = true;
        let t = build_mission_task(&e, None, true).unwrap();
        assert!(t.choice);
        assert_eq!(t.objs.len(), 3);
        assert_eq!(t.tooltip_objs, vec![0, 1, 2]);
    }

    #[test]
    fn choice_mission_tied_variants_stay_a_choice() {
        // Collapsed OR mission where two variants are tied on progress: it must
        // still read as a choice (OR + separator) rather than an AND of both.
        let mut e = mission(4, vec![obj("A", 2, 4), obj("B", 2, 4), obj("C", 0, 4)]);
        e.one_of = true;
        let t = build_mission_task(&e, None, false).unwrap();
        assert!(t.choice);
        assert_eq!(t.objs.len(), 2);
        assert_eq!(t.tooltip_objs, vec![0, 1]);
    }

    #[test]
    fn choice_mission_collapses_to_clear_leader() {
        // "Show all options" off and one variant clearly ahead (3/4 vs 1/4):
        // collapse to the single most-progressed option, not every progressed
        // variant. The trailing 1/4 Nest must drop out.
        let mut e = mission(
            5,
            vec![obj("Cult", 3, 4), obj("Nest", 1, 4), obj("Void", 0, 3)],
        );
        e.one_of = true;
        let t = build_mission_task(&e, None, false).unwrap();
        assert!(!t.choice);
        assert_eq!(t.objs.len(), 1);
        assert_eq!(t.objs[0].remaining, 1);
        assert_eq!(t.tooltip_objs, vec![0]);
    }

    #[test]
    fn choice_mission_leader_by_fraction_not_raw_count() {
        // Different needs: 2/3 (0.67) beats 2/4 (0.50) even though the raw
        // counts are equal, so the higher-fraction variant is the sole leader.
        let mut e = mission(6, vec![obj("A", 2, 4), obj("B", 2, 3)]);
        e.one_of = true;
        let t = build_mission_task(&e, None, false).unwrap();
        assert!(!t.choice);
        assert_eq!(t.tooltip_objs, vec![1]);
    }

    #[test]
    fn build_items_filters_untracked_and_non_inprogress() {
        let mut claimable = mission(3, vec![obj("A", 1, 2)]);
        claimable.state = MissionState::Claimable;
        let view = crate::processing::MissionView {
            season_id: None,
            season_name: String::new(),
            defs_loaded: true,
            entries: vec![mission(1, vec![obj("A", 1, 2)]), claimable],
        };
        let items = build_taskbar_items(&view, |id| id == 1, Vec::new(), None, true);
        assert_eq!(items.len(), 1);
        match &items[0] {
            TaskbarItem::Single(TaskbarTask::Mission(m)) => assert_eq!(m.entry.uid(), 1),
            _ => panic!("expected a single mission item"),
        }
    }

    #[test]
    fn build_items_drops_completed_objective() {
        // In-progress mission whose only objective is already done (remaining 0)
        // must not show on the Taskbar - nothing left to do.
        let view = crate::processing::MissionView {
            season_id: None,
            season_name: String::new(),
            defs_loaded: true,
            entries: vec![mission(1, vec![obj("A", 2, 2)])],
        };
        let items = build_taskbar_items(&view, |_| true, Vec::new(), None, true);
        assert!(items.is_empty());
    }

    fn quest(id: &str, requirements: Vec<i32>) -> QuestData {
        QuestData {
            id: id.to_string(),
            name: "Daily Chest".to_string(),
            description: String::new(),
            expiration: String::new(),
            category: 0,
            unknown_int: 0,
            requirements,
            rewards: Vec::new(),
            completed: false,
            item_of_choice: false,
            repeatable: false,
        }
    }

    #[test]
    fn quest_marks_use_chosen_side() {
        // Two required marks (same id). Seasonal owns 1, regular owns 2.
        let q = quest("q1", vec![100, 100]);
        let seasonal = build_quest_task(&q, |_| (1, 2), true).unwrap();
        assert_eq!(
            (seasonal.have, seasonal.need, seasonal.seasonal),
            (1, 2, true)
        );
        assert_eq!(seasonal.remaining, 1);

        let regular = build_quest_task(&q, |_| (1, 2), false).unwrap();
        assert_eq!(
            (regular.have, regular.need, regular.seasonal),
            (2, 2, false)
        );
        assert_eq!(regular.remaining, 0);
        assert!(regular.marks.is_empty());
    }

    #[test]
    fn quest_groups_distinct_marks() {
        // Four of mark 10, two of mark 20 (Sprite World + 3D marks). Regular
        // owns 1 of mark 10 and none of mark 20.
        let q = quest("q2", vec![10, 10, 10, 10, 20, 20]);
        let t = build_quest_task(&q, |id| if id == 10 { (0, 1) } else { (0, 0) }, false).unwrap();
        assert_eq!(t.marks.len(), 2);
        assert_eq!((t.marks[0].mark_id, t.marks[0].remaining), (10, 3));
        assert_eq!((t.marks[1].mark_id, t.marks[1].remaining), (20, 2));
        assert_eq!((t.have, t.need, t.remaining), (1, 6, 5));
    }

    // --- Smart combining ---------------------------------------------------

    #[test]
    fn dungeon_names_match_handles_advanced_and_plagued_variants() {
        // Missions target the specific variant; mark quests resolve to the base
        // dungeon the mark table uses. They must still match.
        assert!(dungeon_names_match(
            "Advanced Kogbold Steamworks",
            "Kogbold Steamworks"
        ));
        assert!(dungeon_names_match("Plagued Nest", "The Nest"));
        // Same-name and case-insensitive still match.
        assert!(dungeon_names_match("Ocean Trench", "ocean trench"));
        // Genuinely different dungeons do not.
        assert!(!dungeon_names_match("The Void", "The Shatters"));
        assert!(!dungeon_names_match(
            "Advanced Kogbold Steamworks",
            "The Nest"
        ));
    }

    fn dungeon_obj(dungeon: &str, have: i32, need: i32) -> ObjectiveView {
        ObjectiveView {
            label: dungeon.to_string(),
            have,
            need,
            kind: ObjectiveKind::Dungeon,
            dungeon_name: Some(dungeon.to_string()),
            boss_id: None,
            target: dungeon.to_string(),
        }
    }

    fn diff_obj(range: &str, have: i32, need: i32) -> ObjectiveView {
        ObjectiveView {
            label: format!("Difficulty {range}"),
            have,
            need,
            kind: ObjectiveKind::Difficulty,
            dungeon_name: None,
            boss_id: None,
            target: range.to_string(),
        }
    }

    fn vortex_obj(have: i32, need: i32) -> ObjectiveView {
        ObjectiveView {
            label: "Wind Vortexes".to_string(),
            have,
            need,
            kind: ObjectiveKind::KillNamed,
            dungeon_name: None,
            boss_id: None,
            target: "Wind Vortex".to_string(),
        }
    }

    fn view_of(entries: Vec<MissionEntryView>) -> crate::processing::MissionView {
        crate::processing::MissionView {
            season_id: None,
            season_name: String::new(),
            defs_loaded: true,
            entries,
        }
    }

    #[test]
    fn parse_difficulty_range_handles_range_and_exact() {
        assert_eq!(parse_difficulty_range("7-10"), Some((7.0, 10.0)));
        assert_eq!(parse_difficulty_range(" 5 "), Some((5.0, 5.0)));
        assert_eq!(parse_difficulty_range("bad"), None);
    }

    #[test]
    fn objective_progresses_range_matches_dungeon_and_overlap() {
        // The Nest sits in the 7-10 grave band (RealmEye static table).
        let nest = dungeon_obj("The Nest", 0, 3);
        assert!(objective_progresses_range(&nest, 7.0, 10.0));
        assert!(!objective_progresses_range(&nest, 1.0, 2.0));
        // Overlapping difficulty ranges.
        let d = diff_obj("6-8", 0, 3);
        assert!(objective_progresses_range(&d, 7.0, 10.0));
        assert!(!objective_progresses_range(&d, 1.0, 4.0));
    }

    #[test]
    fn missions_chain_detects_shared_objectives() {
        let a = mission(1, vec![dungeon_obj("Ocean Trench", 0, 4)]);
        let b = mission(2, vec![dungeon_obj("ocean trench", 0, 2)]);
        assert!(missions_chain(&a, &b));

        // Identical grave-difficulty ranges chain (same dungeons count for both).
        let g1 = mission(7, vec![diff_obj("7-10", 0, 17)]);
        let g2 = mission(8, vec![diff_obj("7-10", 0, 3)]);
        assert!(missions_chain(&g1, &g2));

        // A specific dungeon that merely falls inside a broad band does NOT chain,
        // so wide "bucket" missions don't hub every dungeon into one chain.
        let nest = mission(3, vec![dungeon_obj("The Nest", 0, 3)]);
        let grave = mission(4, vec![diff_obj("7-10", 0, 3)]);
        assert!(!missions_chain(&nest, &grave));

        // Overlapping-but-different difficulty ranges do NOT chain.
        let wide = mission(9, vec![diff_obj("5.5-10", 0, 50)]);
        let narrow = mission(10, vec![diff_obj("7-10", 0, 17)]);
        assert!(!missions_chain(&wide, &narrow));

        // Unrelated dungeons do not chain.
        let x = mission(5, vec![dungeon_obj("The Void", 0, 3)]);
        let y = mission(6, vec![dungeon_obj("The Shatters", 0, 3)]);
        assert!(!missions_chain(&x, &y));

        // Wind Vortex kills share one counter but resolve to no boss sprite
        // (boss_id None), so they chain by their vortex identity, differing
        // counts and crucible-only variants notwithstanding.
        let v1 = mission(11, vec![vortex_obj(0, 20)]);
        let v2 = mission(12, vec![vortex_obj(0, 25)]);
        assert!(missions_chain(&v1, &v2));

        // A shared objective already completed in one mission does NOT link the
        // two: running that dungeon no longer advances the finished mission.
        let done = mission(13, vec![dungeon_obj("Ocean Trench", 4, 4)]);
        let todo = mission(14, vec![dungeon_obj("Ocean Trench", 0, 2)]);
        assert!(!missions_chain(&done, &todo));

        // They still chain if a second, still-outstanding objective overlaps.
        let done2 = mission(
            15,
            vec![
                dungeon_obj("Ocean Trench", 4, 4),
                dungeon_obj("The Nest", 0, 1),
            ],
        );
        let todo2 = mission(
            16,
            vec![
                dungeon_obj("Ocean Trench", 0, 2),
                dungeon_obj("The Nest", 0, 1),
            ],
        );
        assert!(missions_chain(&done2, &todo2));
    }

    #[test]
    fn build_items_chains_two_active_missions() {
        // Two tracked in-progress missions sharing a dungeon are linked. Use
        // open (participants 255) missions so they're eligible for any
        // character and the active chain renders bright.
        let mut a = mission(1, vec![dungeon_obj("Ocean Trench", 0, 4)]);
        let mut b = mission(2, vec![dungeon_obj("Ocean Trench", 0, 2)]);
        a.participants = 255;
        b.participants = 255;
        let view = view_of(vec![a, b]);
        let items = build_taskbar_items(&view, |_| true, Vec::new(), None, true);
        assert_eq!(items.len(), 1);
        match &items[0] {
            TaskbarItem::Chain(g) => {
                assert_eq!(g.members.len(), 2);
                assert!(!g.dimmed());
                assert!(g.members.iter().all(|m| !m.dimmed));
            }
            _ => panic!("expected a chain of the two shared-dungeon missions"),
        }
    }

    #[test]
    fn build_items_chain_dims_only_ineligible_member() {
        // A seasonal-only mission (participants 2) chained with an open one,
        // evaluated for a non-seasonal character: only the ineligible pill dims,
        // and the active (bright) pill sorts first.
        let mut open = mission(1, vec![dungeon_obj("Ocean Trench", 0, 2)]);
        open.participants = 255;
        let seasonal = mission(2, vec![dungeon_obj("Ocean Trench", 0, 4)]); // participants 2
        let view = view_of(vec![seasonal, open]);
        let items = build_taskbar_items(&view, |_| true, Vec::new(), None, true);
        assert_eq!(items.len(), 1);
        match &items[0] {
            TaskbarItem::Chain(g) => {
                assert_eq!(g.members.len(), 2);
                assert!(!g.members[0].dimmed, "active pill first and bright");
                assert!(g.members[1].dimmed, "ineligible pill dimmed and second");
                assert!(g.dimmed(), "chain has an inactive member");
            }
            _ => panic!("expected a chain"),
        }
    }

    #[test]
    fn build_items_links_cooldown_partner_dimmed() {
        // One active mission plus a tracked cooldown mission that shares its
        // dungeon: they are linked and the whole chain is dimmed. Only a
        // *repeatable* claimed mission is genuinely on cooldown (a non-repeatable
        // claimed mission is completed forever and must not be a chain partner).
        let mut cooldown = mission(2, vec![dungeon_obj("The Nest", 0, 3)]);
        cooldown.state = MissionState::Claimed;
        cooldown.repeatable = true;
        let view = view_of(vec![
            mission(1, vec![dungeon_obj("The Nest", 0, 3)]),
            cooldown,
        ]);
        let items = build_taskbar_items(&view, |_| true, Vec::new(), None, true);
        assert_eq!(items.len(), 1);
        match &items[0] {
            TaskbarItem::Chain(g) => {
                assert_eq!(g.members.len(), 2);
                assert!(g.dimmed());
                assert!(items[0].dim_right());
            }
            _ => panic!("expected a dimmed chain with the cooldown partner"),
        }
    }

    #[test]
    fn build_items_ignores_completed_forever_partner() {
        // A claimed but non-repeatable mission is completed forever (not on
        // cooldown), so it must never be pulled in as a chain partner.
        let mut done = mission(2, vec![dungeon_obj("The Nest", 0, 3)]);
        done.state = MissionState::Claimed;
        done.repeatable = false;
        let view = view_of(vec![mission(1, vec![dungeon_obj("The Nest", 0, 3)]), done]);
        let items = build_taskbar_items(&view, |_| true, Vec::new(), None, true);
        assert_eq!(items.len(), 1);
        assert!(
            matches!(items[0], TaskbarItem::Single(_)),
            "a completed-forever mission must not form a dimmed chain"
        );
    }

    #[test]
    fn build_items_keeps_unrelated_missions_separate() {
        let view = view_of(vec![
            mission(1, vec![dungeon_obj("The Void", 0, 3)]),
            mission(2, vec![dungeon_obj("The Shatters", 0, 3)]),
        ]);
        let items = build_taskbar_items(&view, |_| true, Vec::new(), None, true);
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|i| matches!(i, TaskbarItem::Single(_))));
    }

    #[test]
    fn locked_cooldown_task_surfaces_only_matching_objective() {
        // A choice mission on cooldown, locked onto its Ocean Trench objective:
        // only that objective becomes a single, non-choice chip at its full
        // requirement, and only it is reproduced in the tooltip.
        let mut e = mission(
            1,
            vec![
                dungeon_obj("Sprite World", 0, 4),
                dungeon_obj("Ocean Trench", 0, 4),
                dungeon_obj("The Nest", 0, 4),
            ],
        );
        e.one_of = true;
        let t = build_locked_cooldown_task(&e, 1, None).unwrap();
        assert!(!t.choice, "a locked pill is not a choice");
        assert_eq!(t.objs.len(), 1);
        assert_eq!(t.objs[0].remaining, 4, "full requirement while on cooldown");
        assert_eq!(t.tooltip_objs, vec![1]);
    }
}

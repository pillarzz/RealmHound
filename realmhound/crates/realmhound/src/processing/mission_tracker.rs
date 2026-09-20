//! Seasonal mission tracking model (worker-owned).
//!
//! Combines three inputs into a single authoritative view:
//! - `getClientSeasons` HTTP definitions (names, objectives, rewards),
//! - `getPlayerMissions` HTTP progress (server-authoritative absolute progress
//!   and claim `state`) - the baseline,
//! - packet-165 `prog/` deltas applied live on top between HTTP fetches.
//!
//! The server baseline always wins: a fresh `getPlayerMissions` fetch replaces
//! locally-accumulated progress, so a mid-session launch or a game-side desync
//! (the "completed -> unclaimable -> rolled back" glitch) reconciles on the next
//! refresh. Live deltas only ever move the numbers forward between fetches.

use std::collections::{HashMap, HashSet};

use realmhound_core::api::{
    ClientSeasons, Cond, CondKind, MissionState, PlayerMissions, ProgEntry, Reward, Season,
};

/// Live per-mission runtime state.
#[derive(Debug, Clone, Default)]
struct MissionRuntime {
    /// Absolute per-sub-objective progress (index-aligned with the def conds).
    progress: Vec<i32>,
    /// Server-reported status; `None` until a `getPlayerMissions` fetch.
    state: Option<MissionState>,
    /// Unix seconds the mission was last claimed (anchors the reset countdown).
    claimed_at: Option<i64>,
}

/// A pending reward claim awaiting its server ack (packet 164).
#[derive(Debug, Clone, Copy)]
struct PendingClaim {
    season_id: i32,
    mission_id: i32,
}

/// Worker-owned seasonal mission tracker.
#[derive(Debug, Default)]
pub struct MissionTracker {
    /// HTTP mission definitions, if fetched.
    defs: Option<ClientSeasons>,
    /// Primary season id: the `current` tree, used only for the header title.
    /// The view itself merges every tree in `defs`.
    season_id: Option<i32>,
    /// Per-mission runtime, keyed by `(tree/season id, mission id)` so missions
    /// from different trees never collide.
    runtime: HashMap<(i32, i32), MissionRuntime>,
    /// Fully-unlocked tree ids (`<TreesUnlocked>`): every mission in these trees
    /// is available. From the last `getPlayerMissions`.
    trees_unlocked: HashSet<i32>,
    /// Individually locked `(tree, mission)` from `<LockedMissions>`.
    locked: HashSet<(i32, i32)>,
    /// Individually unlocked `(tree, mission)` from `<UnlockedMissions>`.
    unlocked: HashSet<(i32, i32)>,
    /// Trees the last `getPlayerMissions` actually reported a `<Season>` block
    /// for. Within a seen tree, a mission with no progress entry is locked.
    seen_trees: HashSet<i32>,
    /// `(tree, mission)` the server surfaced as unlocked: it either carried a
    /// progress entry in its `<Season>` block or appeared in `<UnlockedMissions>`.
    server_unlocked: HashSet<(i32, i32)>,
    /// Account name this state belongs to; used to discard stale/mule data.
    account: Option<String>,
    /// Outstanding claims keyed by request id (from packet 163).
    pending_claims: HashMap<i8, PendingClaim>,
    /// Bumps on every observable change so the worker can gate snapshot publish.
    generation: u64,
}

impl MissionTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Current change generation (publish gate).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn touch(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// Bind tracking to an account. Switching to a *different* known account
    /// clears all state so a mule's data never bleeds into the main's view.
    ///
    /// A `None` account means the identity is momentarily unknown (e.g. a mule
    /// Update packet just triggered a mismatch, or the main is reconnecting on
    /// an area change) -- not an account switch. Clearing on `None` would wipe
    /// the account-agnostic definitions and the verified main's progress every
    /// time the detected name flapped, degrading the panel to "No data" until
    /// the next Refresh. So `None` keeps the current state untouched.
    pub fn set_account(&mut self, account: Option<String>) {
        let Some(new) = account else {
            return;
        };
        if self
            .account
            .as_deref()
            .is_some_and(|a| a.eq_ignore_ascii_case(&new))
        {
            return;
        }
        self.account = Some(new);
        self.runtime.clear();
        self.pending_claims.clear();
        self.trees_unlocked.clear();
        self.locked.clear();
        self.unlocked.clear();
        self.seen_trees.clear();
        self.server_unlocked.clear();
        self.season_id = None;
        self.defs = None;
        self.touch();
    }

    /// Apply fetched mission definitions. Adopts the `current` season (else the
    /// first) as the primary tree for the header title only; every tree is shown.
    pub fn apply_definitions(&mut self, defs: ClientSeasons) {
        if self.season_id.is_none() {
            self.season_id = defs
                .seasons
                .iter()
                .find(|s| s.current == 1)
                .or_else(|| defs.seasons.first())
                .map(|s| s.id);
        }
        self.defs = Some(defs);
        self.touch();
    }

    /// Apply the server-authoritative progress baseline. Replaces local progress
    /// and claim state for every mission the server reports, plus the tree-level
    /// lock state used to bucket locked missions.
    pub fn apply_player_missions(&mut self, pm: PlayerMissions) {
        if let Some(id) = pm.season_id {
            self.season_id = Some(id);
        }
        self.trees_unlocked = pm.trees_unlocked.iter().copied().collect();
        self.locked = pm.locked_missions.iter().copied().collect();
        self.unlocked = pm.unlocked_missions.iter().copied().collect();
        // Presence in a tree's <Season> block is the server's unlock signal: a
        // reported mission is unlocked, an omitted one (in a seen tree) is locked.
        self.seen_trees = pm.missions.iter().map(|m| m.tree_id).collect();
        self.server_unlocked = pm
            .missions
            .iter()
            .map(|m| (m.tree_id, m.mission_id))
            .chain(pm.unlocked_missions.iter().copied())
            .collect();
        for m in pm.missions {
            let rt = self.runtime.entry((m.tree_id, m.mission_id)).or_default();
            rt.progress = m.progress;
            rt.state = Some(m.state);
            rt.claimed_at = m.claimed_at;
        }
        self.touch();
    }

    /// Apply a live packet-165 progress delta (additive). Accepts deltas for any
    /// tree present in the definitions; ignored entirely until definitions are
    /// loaded, since a Refresh brings the server-authoritative baseline and
    /// pre-Refresh diffs would only produce stale/placeholder state.
    pub fn apply_prog(&mut self, entry: &ProgEntry) {
        let Some(defs) = self.defs.as_ref() else {
            return;
        };
        if defs.season(entry.season_id).is_none() {
            return;
        }
        let key = (entry.season_id, entry.mission_id);
        // A pure zero delta (login-sync presence signal) carries no progress;
        // ensure the mission exists but never overwrite an existing count.
        let all_zero = entry.values.iter().all(|v| *v == 0);
        let rt = self.runtime.entry(key).or_default();
        if all_zero {
            if rt.progress.is_empty() {
                rt.progress = vec![0; entry.values.len()];
                self.touch();
            }
            return;
        }
        if rt.progress.len() < entry.values.len() {
            rt.progress.resize(entry.values.len(), 0);
        }
        for (slot, delta) in rt.progress.iter_mut().zip(entry.values.iter()) {
            *slot += *delta;
        }
        self.touch();
    }

    /// Record an outgoing claim request (packet 163), pending its server ack.
    pub fn record_claim_request(&mut self, request_id: i8, season_id: i32, mission_id: i32) {
        self.pending_claims.insert(
            request_id,
            PendingClaim {
                season_id,
                mission_id,
            },
        );
    }

    /// Resolve a claim ack (packet 164). On success the matching mission flips to
    /// `Claimed`; failures just drop the pending entry.
    pub fn confirm_claim(&mut self, request_id: i8, success: bool) {
        let Some(pending) = self.pending_claims.remove(&request_id) else {
            return;
        };
        if !success {
            return;
        }
        let rt = self
            .runtime
            .entry((pending.season_id, pending.mission_id))
            .or_default();
        rt.state = Some(MissionState::Claimed);
        // Anchor the cooldown to the actual claim moment. Without this a
        // repeatable mission re-claimed from packets keeps a stale (past)
        // `claimed_at`, so its `reset_at` lands in the past and the view
        // treats the cooldown as already elapsed -- resetting progress and
        // dropping it back to "In progress" instead of "On cooldown".
        rt.claimed_at = Some(now_unix());
        self.touch();
    }

    /// Reset repeatable missions whose claim cooldown has elapsed. Scans every
    /// tree so cooldowns in any battlepass tree reset.
    pub fn expire_cooldowns(&mut self, now: i64) -> bool {
        let Some(defs) = self.defs.as_ref() else {
            return false;
        };

        let mut changed = false;
        for season in &defs.seasons {
            for def in &season.missions {
                if !def.name.contains("Repeatable") {
                    continue;
                }
                let Some(rt) = self.runtime.get_mut(&(season.id, def.id)) else {
                    continue;
                };
                let Some(reset_at) = rt
                    .claimed_at
                    .zip(def.cooldown_secs())
                    .map(|(claimed, cooldown)| claimed + cooldown)
                else {
                    continue;
                };
                if rt.state == Some(MissionState::Claimed) && reset_at <= now {
                    rt.progress.fill(0);
                    rt.state = Some(MissionState::InProgress);
                    rt.claimed_at = None;
                    changed = true;
                }
            }
        }

        if changed {
            self.touch();
        }
        changed
    }

    /// The primary season (the `current` tree), used only for the header title.
    fn primary_season(&self) -> Option<&Season> {
        let defs = self.defs.as_ref()?;
        let id = self.season_id?;
        defs.season(id)
    }
    /// Build a UI-facing snapshot of the current mission state. Merges every
    /// active tree in the definitions into one list; each entry is tagged with
    /// its tree name so the panel can label missions from different trees.
    pub fn build_view(&self) -> MissionView {
        let Some(defs) = self.defs.as_ref() else {
            // Definitions not loaded yet: don't surface raw progress diffs as
            // placeholder rows. The panel shows a "press Refresh" prompt until a
            // Refresh brings authoritative definitions + progress.
            return MissionView {
                season_id: self.season_id,
                season_name: String::new(),
                defs_loaded: false,
                entries: Vec::new(),
            };
        };

        // Show active trees (current or available). Fall back to every tree if
        // none carry the flags, so a schema tweak never blanks the panel.
        let any_active = defs
            .seasons
            .iter()
            .any(|s| s.current == 1 || s.available == 1);

        let mut entries: Vec<MissionEntryView> = Vec::new();
        for season in &defs.seasons {
            if any_active && !(season.current == 1 || season.available == 1) {
                continue;
            }
            for def in &season.missions {
                entries.push(self.build_entry(season, def));
            }
        }

        let season_name = self
            .primary_season()
            .map(|s| s.name.clone())
            .unwrap_or_default();
        MissionView {
            season_id: self.season_id,
            season_name,
            defs_loaded: true,
            entries,
        }
    }

    /// Whether mission `mission_id` within `season` counts as a satisfied
    /// prerequisite: it has been claimed, or its objectives are complete. Used to
    /// evaluate the `parentsStr` unlock graph. An unknown or unstarted mission is
    /// not satisfied, so its children stay locked.
    fn mission_satisfied(&self, season: &Season, mission_id: i32) -> bool {
        let Some(def) = season.missions.iter().find(|m| m.id == mission_id) else {
            return false;
        };
        let rt = self.runtime.get(&(season.id, mission_id));
        let claimed = matches!(rt.and_then(|r| r.state), Some(MissionState::Claimed));
        let empty: Vec<i32> = Vec::new();
        let progress = rt.map(|r| &r.progress).unwrap_or(&empty);
        claimed || def.is_complete(progress)
    }

    /// Build a single mission row, resolving live progress, lock state, rewards
    /// and objectives for `def` within `season`.
    fn build_entry(
        &self,
        season: &Season,
        def: &realmhound_core::api::MissionDef,
    ) -> MissionEntryView {
        let key = (season.id, def.id);
        let rt = self.runtime.get(&key);
        let progress: Vec<i32> = rt.map(|r| r.progress.clone()).unwrap_or_default();
        let objectives = def
            .conds
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let (kind, dungeon_name) = objective_kind(c);
                let boss_id = if matches!(kind, ObjectiveKind::KillNamed) {
                    resolve_boss_id(&c.target)
                } else {
                    None
                };
                ObjectiveView {
                    label: c.objective_label(),
                    have: progress.get(i).copied().unwrap_or(0),
                    need: c.amount,
                    kind,
                    dungeon_name,
                    boss_id,
                    target: c.target.clone(),
                }
            })
            .collect();
        let rewards = def
            .rewards
            .iter()
            .map(reward_label)
            .filter(|s| !s.is_empty())
            .collect();
        let rewards_detail: Vec<RewardView> =
            def.rewards.iter().filter_map(reward_detail).collect();
        let reward_groups: Vec<RewardGroup> = def
            .reward_groups()
            .into_iter()
            .filter_map(|idxs| {
                let rewards: Vec<RewardView> = idxs
                    .iter()
                    .filter_map(|&i| def.rewards.get(i).and_then(reward_detail))
                    .collect();
                (!rewards.is_empty()).then_some(RewardGroup {
                    one_of: rewards.len() > 1,
                    rewards,
                })
            })
            .collect();
        let icon_object_id = rewards_detail.iter().find_map(|r| match r {
            RewardView::Item { object_id, .. } if *object_id > 0 => Some(*object_id),
            _ => None,
        });
        let repeatable = def.name.contains("Repeatable");
        let name = def
            .name
            .replace("(Repeatable)", "")
            .replace("(repeatable)", "")
            .trim()
            .to_string();
        let complete = def.is_complete(&progress);
        // Lock state combines the server's explicit signals with the mission
        // tree's unlock graph. A mission is unlocked if the last
        // getPlayerMissions carried a progress entry for it (rt.is_some()), if it
        // appears in <UnlockedMissions>, via a later live delta, OR if all of its
        // `parentsStr` prerequisites are satisfied (claimed/complete). Graph
        // unlock is essential for a past tree, which omits unlocked-but-unstarted
        // missions from its <Season> block (they'd otherwise false-lock). Within a
        // tree the server reported, a mission with none of these is locked.
        let parents = def.parents();
        let graph_unlocked = parents.iter().all(|&p| self.mission_satisfied(season, p));
        let unlocked = rt.is_some()
            || self.server_unlocked.contains(&key)
            || self.unlocked.contains(&key)
            || graph_unlocked;
        let is_locked =
            self.locked.contains(&key) || (self.seen_trees.contains(&season.id) && !unlocked);
        let state = if is_locked {
            MissionState::Locked
        } else {
            rt.and_then(|r| r.state).unwrap_or(if complete {
                MissionState::Claimable
            } else {
                MissionState::InProgress
            })
        };
        // A repeatable mission that's been claimed is on cooldown; it
        // restarts `interval` after the last claim.
        let reset_at = if repeatable && state == MissionState::Claimed {
            match (rt.and_then(|r| r.claimed_at), def.cooldown_secs()) {
                (Some(claimed), Some(cd)) => Some(claimed + cd),
                _ => None,
            }
        } else {
            None
        };
        MissionEntryView {
            tree_id: season.id,
            tree_name: season.name.clone(),
            mission_id: def.id,
            name,
            desc: {
                // Prefer the reliable, structured objective description over
                // Deca's hand-typed `desc` (which can contain typos, e.g. the
                // wrong difficulty tier). Fall back to `desc` only when the
                // mission has no structured objectives.
                let objective = def.objective_description();
                if objective.is_empty() {
                    def.desc.clone()
                } else {
                    objective
                }
            },
            objectives,
            rewards,
            rewards_detail,
            reward_groups,
            category: MissionCategory::from_participants(def.participants),
            participants: def.participants,
            worn_restriction: def
                .worn_restriction
                .iter()
                .filter_map(|w| {
                    realmhound_core::assets::slot_type_from_name(&w.kind).map(|st| WornReqView {
                        slot_type: st,
                        display: if w.display.is_empty() {
                            w.kind.clone()
                        } else {
                            w.display.clone()
                        },
                    })
                })
                .collect(),
            repeatable,
            icon_object_id,
            icon_ref: def.icon_ref().map(|(sheet, idx)| (sheet.to_string(), idx)),
            state,
            complete,
            reset_at,
            one_of: def.is_one_of(),
            defs_loaded: true,
        }
    }
}

/// Current unix time in seconds (wall clock). Used to anchor a repeatable
/// mission's cooldown when a claim is confirmed from packets.
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Format a single reward as a short display string. Item names are resolved via
/// the global asset manager; unknown items fall back to their object id.
fn reward_label(r: &realmhound_core::api::Reward) -> String {
    match r.reward_type {
        1 => {
            if r.amount > 0 {
                format!("{} BXP", format_thousands(r.amount))
            } else {
                String::new()
            }
        }
        2 => {
            let id: i32 = r.target.parse().unwrap_or(0);
            let name = realmhound_core::assets::get_asset_manager().object_name(id);
            let base = match name {
                Some(n) if !n.is_empty() => n,
                _ => format!("Item #{id}"),
            };
            if r.amount > 1 {
                format!("{base} x{}", r.amount)
            } else {
                base
            }
        }
        _ => String::new(),
    }
}

fn format_thousands(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

// ---------------------------------------------------------------------------
// UI view types (cloned into the ModelSnapshot each publish).
// ---------------------------------------------------------------------------

/// How an objective is best visualised. Dungeon objectives render as discrete
/// portal tokens that light up; everything else falls back to a progress bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectiveKind {
    Dungeon,
    KillNamed,
    KillCount,
    Difficulty,
    CloseRealms,
    Other,
}

/// Mission grouping derived from the `participants` eligibility bitmask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissionCategory {
    Regular,
    Seasonal,
    Crucible,
    Event,
}

impl MissionCategory {
    fn from_participants(p: i32) -> Self {
        match p {
            255 => MissionCategory::Regular,
            2 => MissionCategory::Seasonal,
            4 => MissionCategory::Crucible,
            _ => MissionCategory::Event,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MissionCategory::Regular => "REGULAR",
            MissionCategory::Seasonal => "SEASONAL",
            MissionCategory::Crucible => "CRUCIBLE",
            MissionCategory::Event => "EVENT",
        }
    }

    /// Header/badge accent colour (r, g, b).
    pub fn color(self) -> (u8, u8, u8) {
        match self {
            MissionCategory::Regular => (0xc9, 0xb2, 0x00),
            MissionCategory::Seasonal => (0x15, 0xdc, 0xa6),
            MissionCategory::Crucible => (0xdc, 0x21, 0x15),
            MissionCategory::Event => (0xe0, 0xe0, 0xe0),
        }
    }

    /// Stable sort order for category grouping.
    pub fn order(self) -> u8 {
        match self {
            MissionCategory::Regular => 0,
            MissionCategory::Seasonal => 1,
            MissionCategory::Crucible => 2,
            MissionCategory::Event => 3,
        }
    }
}

/// A single reward, structured so the UI can render an icon plus text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RewardView {
    Bxp(i64),
    Item {
        object_id: i32,
        amount: i64,
        name: String,
    },
}

/// A group of rewards sharing the same `rewards_spread` digit. When `one_of` is
/// true the player picks a single reward from the group ("One of:"); otherwise
/// the single reward is granted outright. Groups are ANDed together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewardGroup {
    pub rewards: Vec<RewardView>,
    pub one_of: bool,
}

fn objective_kind(c: &Cond) -> (ObjectiveKind, Option<String>) {
    match c.kind() {
        // Timed conds also target a named dungeon, so render portal tokens.
        CondKind::Dungeon | CondKind::Timed => (ObjectiveKind::Dungeon, Some(c.target.clone())),
        CondKind::KillNamed => (ObjectiveKind::KillNamed, None),
        CondKind::KillCount => (ObjectiveKind::KillCount, None),
        CondKind::DifficultyRange | CondKind::DifficultyExact => (ObjectiveKind::Difficulty, None),
        CondKind::CloseRealms => (ObjectiveKind::CloseRealms, None),
        _ => (ObjectiveKind::Other, None),
    }
}

/// Resolve a `KillNamed` target ("shtrs Twilight Archmage") to an enemy-sprite
/// object id. Mission targets prefix the boss name with a lowercase dungeon
/// abbreviation, so try the whole string first, then drop leading tokens until
/// a known display name matches (preferring enemy sprites).
fn resolve_boss_id(target: &str) -> Option<i32> {
    let am = realmhound_core::assets::get_asset_manager();
    let mut s = target.trim();
    loop {
        if !s.is_empty() {
            if let Some(id) = am.object_id_for_display_name(s) {
                return Some(realmhound_core::assets::normalize_train_sprite(id, s));
            }
            // Some encounter bosses carry an internal id_name that differs from
            // their display name (e.g. "Big Barnacle" for Man-eating Barnacle),
            // and the cond target uses that id_name. Fall back to the id_name
            // index before dropping a leading token so those still get a sprite.
            if let Some(id) = am.object_id_for_name(s) {
                return Some(realmhound_core::assets::normalize_train_sprite(id, s));
            }
        }
        match s.split_once(' ') {
            Some((_, rest)) if !rest.trim().is_empty() => s = rest.trim(),
            _ => return None,
        }
    }
}

fn reward_detail(r: &Reward) -> Option<RewardView> {
    match r.reward_type {
        1 => (r.amount > 0).then_some(RewardView::Bxp(r.amount)),
        2 => {
            let id: i32 = r.target.parse().unwrap_or(0);
            let name = realmhound_core::assets::get_asset_manager()
                .object_name(id)
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| format!("Item #{id}"));
            Some(RewardView::Item {
                object_id: id,
                amount: r.amount.max(1),
                name,
            })
        }
        _ => None,
    }
}

/// A single sub-objective's progress for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectiveView {
    pub label: String,
    pub have: i32,
    pub need: i32,
    pub kind: ObjectiveKind,
    /// Dungeon name for portal-token resolution (present for `Dungeon` kind).
    pub dungeon_name: Option<String>,
    /// Resolved enemy-sprite object id for `KillNamed` bosses, when the target
    /// name matches a known game object; `None` otherwise.
    pub boss_id: Option<i32>,
    /// Raw cond target tag (e.g. `VETERAN_ENCOUNTER`, `BEACON_GUARDIAN`), used
    /// to drive category hover tooltips. Empty when the cond has no target.
    pub target: String,
}

/// A resolved worn-equipment restriction for display and eligibility: the
/// character must have an item of `slot_type` equipped (e.g. an Orb). `display`
/// is the human label shown in the tooltip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WornReqView {
    pub slot_type: i32,
    pub display: String,
}

/// A mission row for the tracklist UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissionEntryView {
    /// Tree (season) id this mission belongs to.
    pub tree_id: i32,
    /// Human-readable tree name (e.g. "The Forgotten Legacy"), shown as a per-
    /// mission label when more than one tree is visible.
    pub tree_name: String,
    pub mission_id: i32,
    pub name: String,
    pub desc: String,
    pub objectives: Vec<ObjectiveView>,
    pub rewards: Vec<String>,
    /// Structured rewards for icon + text rendering.
    pub rewards_detail: Vec<RewardView>,
    /// Rewards grouped by AND / "one of" semantics for display.
    pub reward_groups: Vec<RewardGroup>,
    /// Category derived from the `participants` bitmask.
    pub category: MissionCategory,
    /// Raw eligibility bitmask (`255`=all, `2`=seasonal, `4`=crucible). `0` when
    /// unknown (placeholder rows before definitions load).
    pub participants: i32,
    /// Worn-equipment restrictions (resolved from `wornRestriction`): the
    /// character must have an item of one of these slot types equipped to make
    /// progress. Empty when unrestricted.
    pub worn_restriction: Vec<WornReqView>,
    /// True when the mission title is flagged `(Repeatable)`.
    pub repeatable: bool,
    /// Reward-sprite fallback for the mission icon (first item reward).
    pub icon_object_id: Option<i32>,
    /// Unique in-game mission icon as `(sheet_name, index)` from the `icon`
    /// field; preferred over `icon_object_id` when present.
    pub icon_ref: Option<(String, i32)>,
    pub state: MissionState,
    /// Locally-computed completion (advisory; server `state` is authoritative).
    pub complete: bool,
    /// Unix seconds when a repeatable mission on cooldown restarts
    /// (`claimed_at + interval`). `None` for non-repeatable or active missions.
    pub reset_at: Option<i64>,
    /// True when only one objective needs completing ("ONE OF BELOW").
    pub one_of: bool,
    /// False for placeholder rows shown before definitions load.
    pub defs_loaded: bool,
}

impl MissionEntryView {
    /// Identity that is unique across trees. Mission ids restart at 1 in every
    /// tree, so the panel, taskbar and persisted prefs must key on this instead
    /// of the bare `mission_id` to avoid cross-tree collisions.
    pub fn uid(&self) -> i64 {
        ((self.tree_id as i64) << 32) | (self.mission_id as u32 as i64)
    }
}

/// Cloned snapshot of the mission tracker for the UI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MissionView {
    pub season_id: Option<i32>,
    pub season_name: String,
    pub defs_loaded: bool,
    pub entries: Vec<MissionEntryView>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use realmhound_core::api::parse_client_seasons;

    const DEFS: &str = r#"
    {
      "seasons": [{
        "name": "The Mad God's Tempest", "current": 1, "id": 52,
        "missions": [
          { "id": 1, "name": "Blown Away", "desc": "Defeat 75 enemies",
            "conds": [{ "type": 2, "target": "", "amount": 75 }],
            "rewards": [{ "type": 1, "target": "", "amount": 1400000 }] },
          { "id": 15, "name": "Follow the Draft",
            "desc": "Complete 4 runs of one listed dungeon",
            "conds": [{ "type": 3, "target": "Davy Jones' Locker", "amount": 4 },
                      { "type": 3, "target": "Ocean Trench", "amount": 4 },
                      { "type": 3, "target": "Puppet Master's Encore", "amount": 4 },
                      { "type": 3, "target": "Mountain Temple", "amount": 4 }],
            "rewards": [{ "type": 1, "target": "", "amount": 4500000 }],
            "condSpread": "1111" }
        ]
      }]
    }"#;

    fn tracker_with_defs() -> MissionTracker {
        let mut t = MissionTracker::new();
        t.apply_definitions(parse_client_seasons(DEFS).unwrap());
        t
    }

    fn entry<'a>(v: &'a MissionView, id: i32) -> &'a MissionEntryView {
        v.entries.iter().find(|e| e.mission_id == id).unwrap()
    }

    fn entry_in<'a>(v: &'a MissionView, tree: i32, id: i32) -> &'a MissionEntryView {
        v.entries
            .iter()
            .find(|e| e.tree_id == tree && e.mission_id == id)
            .unwrap()
    }

    #[test]
    fn adopts_current_season_from_defs() {
        let t = tracker_with_defs();
        assert_eq!(t.season_id, Some(52));
        let v = t.build_view();
        assert!(v.defs_loaded);
        assert_eq!(v.season_name, "The Mad God's Tempest");
        assert_eq!(v.entries.len(), 2);
    }

    #[test]
    fn player_missions_set_baseline_and_state() {
        let mut t = tracker_with_defs();
        let pm = realmhound_core::api::parse_player_missions(
            "<Missions><Season id=\"52\">1,75,1|15,0:4:0:0,3</Season></Missions>",
        );
        t.apply_player_missions(pm);
        let v = t.build_view();
        let m1 = entry(&v, 1);
        assert_eq!(m1.objectives[0].have, 75);
        assert!(m1.complete);
        assert_eq!(m1.state, MissionState::Claimable);
        let m15 = entry(&v, 15);
        assert_eq!(m15.objectives[1].have, 4); // Ocean Trench
        assert!(m15.complete);
        assert!(m15.one_of);
        assert_eq!(m15.state, MissionState::Claimed);
    }

    #[test]
    fn live_delta_accumulates_on_baseline() {
        let mut t = tracker_with_defs();
        t.apply_player_missions(realmhound_core::api::parse_player_missions(
            "<Missions><Season id=\"52\">15,0:2:0:0,0</Season></Missions>",
        ));
        // +1 Ocean Trench (2nd sub-objective) twice -> 4.
        t.apply_prog(&ProgEntry {
            season_id: 52,
            mission_id: 15,
            values: vec![0, 1, 0, 0],
        });
        t.apply_prog(&ProgEntry {
            season_id: 52,
            mission_id: 15,
            values: vec![0, 1, 0, 0],
        });
        let v = t.build_view();
        let m15 = entry(&v, 15);
        assert_eq!(m15.objectives[1].have, 4);
        assert!(m15.complete);
    }

    #[test]
    fn zero_delta_never_overwrites_progress() {
        let mut t = tracker_with_defs();
        t.apply_player_missions(realmhound_core::api::parse_player_missions(
            "<Missions><Season id=\"52\">1,40,0</Season></Missions>",
        ));
        // Login-sync style presence signal must not reset the 40.
        t.apply_prog(&ProgEntry {
            season_id: 52,
            mission_id: 1,
            values: vec![0],
        });
        assert_eq!(entry(&t.build_view(), 1).objectives[0].have, 40);
    }

    #[test]
    fn ignores_other_season_deltas() {
        let mut t = tracker_with_defs();
        t.apply_prog(&ProgEntry {
            season_id: 999,
            mission_id: 1,
            values: vec![5],
        });
        // Season stays 52; mission 1 shows no progress.
        assert_eq!(t.season_id, Some(52));
        assert_eq!(entry(&t.build_view(), 1).objectives[0].have, 0);
    }

    #[test]
    fn claim_flow_marks_claimed_on_success_ack() {
        let mut t = tracker_with_defs();
        t.apply_player_missions(realmhound_core::api::parse_player_missions(
            "<Missions><Season id=\"52\">1,75,1</Season></Missions>",
        ));
        t.record_claim_request(6, 52, 1);
        t.confirm_claim(6, true);
        assert_eq!(entry(&t.build_view(), 1).state, MissionState::Claimed);
    }

    #[test]
    fn repeatable_cooldown_computes_reset_at() {
        // A repeatable mission (interval 5 days) that's been claimed is on
        // cooldown; its reset anchors to the last-claimed timestamp + interval.
        const RDEFS: &str = r#"
        {
          "seasons": [{
            "name": "S", "current": 1, "id": 52,
            "missions": [
              { "id": 7, "name": "Orb of the Tempest (Repeatable)",
                "desc": "d", "interval": 5.0,
                "conds": [{ "type": 2, "target": "", "amount": 1 }],
                "rewards": [{ "type": 1, "target": "", "amount": 1000 }] }
            ]
          }]
        }"#;
        let mut t = MissionTracker::new();
        t.apply_definitions(parse_client_seasons(RDEFS).unwrap());
        // 6-field entry: id,prog,state,T_updated,count,T_claimed. Claimed (3).
        t.apply_player_missions(realmhound_core::api::parse_player_missions(
            "<Missions><Season id=\"52\">7,1,3,1787053943,2,1000000</Season></Missions>",
        ));
        let v = t.build_view();
        let m7 = entry(&v, 7);
        assert_eq!(m7.state, MissionState::Claimed);
        assert!(m7.repeatable);
        // 1_000_000 + 5 * 86_400.
        assert_eq!(m7.reset_at, Some(1_432_000));
    }

    #[test]
    fn confirmed_claim_anchors_repeatable_cooldown_to_now() {
        // Re-claiming a repeatable from packets must anchor the cooldown to the
        // claim moment, not leave a stale (past) claimed_at -- otherwise the view
        // treats the cooldown as elapsed and drops it back to "In progress".
        const RDEFS: &str = r#"
        {
          "seasons": [{
            "name": "S", "current": 1, "id": 52,
            "missions": [
              { "id": 7, "name": "Orb of the Tempest (Repeatable)",
                "desc": "d", "interval": 5.0,
                "conds": [{ "type": 2, "target": "", "amount": 1 }],
                "rewards": [{ "type": 1, "target": "", "amount": 1000 }] }
            ]
          }]
        }"#;
        let mut t = MissionTracker::new();
        t.apply_definitions(parse_client_seasons(RDEFS).unwrap());
        // Prior cycle: claimed long ago (claimed_at = 1000000, cooldown elapsed).
        t.apply_player_missions(realmhound_core::api::parse_player_missions(
            "<Missions><Season id=\"52\">7,1,3,1787053943,2,1000000</Season></Missions>",
        ));
        // Player replays and re-claims in-game; only the packet ack is observed.
        t.record_claim_request(3, 52, 7);
        t.confirm_claim(3, true);
        let m7 = entry(&t.build_view(), 7).clone();
        assert_eq!(m7.state, MissionState::Claimed);
        // reset_at now anchors to a fresh claim -> in the future (now + 5 days),
        // well past the stale 1_432_000 the old timestamp would have produced.
        let reset = m7.reset_at.expect("cooldown reset_at set after claim");
        assert!(
            reset > 1_432_000,
            "reset_at should not use the stale claimed_at"
        );
    }

    #[test]
    fn expired_repeatable_accepts_new_progress() {
        const RDEFS: &str = r#"
        {
          "seasons": [{
            "name": "S", "current": 1, "id": 52,
            "missions": [
              { "id": 7, "name": "Orb of the Tempest (Repeatable)",
                "desc": "d", "interval": 5.0,
                "conds": [{ "type": 2, "target": "", "amount": 4 }],
                "rewards": [{ "type": 1, "target": "", "amount": 1000 }] }
            ]
          }]
        }"#;
        let mut t = MissionTracker::new();
        t.apply_definitions(parse_client_seasons(RDEFS).unwrap());
        t.apply_player_missions(realmhound_core::api::parse_player_missions(
            "<Missions><Season id=\"52\">7,4,3,1787053943,2,1000000</Season></Missions>",
        ));

        assert!(t.expire_cooldowns(1_432_000));
        assert!(!t.expire_cooldowns(1_432_000));
        t.apply_prog(&ProgEntry {
            season_id: 52,
            mission_id: 7,
            values: vec![1],
        });

        let view = t.build_view();
        let mission = entry(&view, 7);
        assert_eq!(mission.state, MissionState::InProgress);
        assert_eq!(mission.objectives[0].have, 1);
        assert_eq!(mission.reset_at, None);
    }

    #[test]
    fn failed_ack_does_not_claim() {
        let mut t = tracker_with_defs();
        t.apply_player_missions(realmhound_core::api::parse_player_missions(
            "<Missions><Season id=\"52\">1,75,1</Season></Missions>",
        ));
        t.record_claim_request(6, 52, 1);
        t.confirm_claim(6, false);
        assert_eq!(entry(&t.build_view(), 1).state, MissionState::Claimable);
        // Unmatched ack is a no-op.
        t.confirm_claim(9, true);
        assert_eq!(entry(&t.build_view(), 1).state, MissionState::Claimable);
    }

    #[test]
    fn switching_account_clears_state() {
        let mut t = tracker_with_defs();
        t.set_account(Some("Main".into()));
        t.apply_player_missions(realmhound_core::api::parse_player_missions(
            "<Missions><Season id=\"52\">1,75,2</Season></Missions>",
        ));
        assert!(!t.build_view().entries.is_empty() || t.runtime.contains_key(&(52, 1)));
        t.set_account(Some("Mule".into()));
        assert!(t.runtime.is_empty());
        assert_eq!(t.season_id, None);
    }

    #[test]
    fn unknown_account_flap_preserves_state() {
        // Real load order: bind the main account first, then defs + progress
        // (matches ControlMsg::ApplyMissionData in the processor).
        let mut t = MissionTracker::new();
        t.set_account(Some("Main".into()));
        t.apply_definitions(parse_client_seasons(DEFS).unwrap());
        t.apply_player_missions(realmhound_core::api::parse_player_missions(
            "<Missions><Season id=\"52\">1,75,2</Season></Missions>",
        ));
        assert!(t.runtime.contains_key(&(52, 1)));
        assert!(t.defs.is_some());
        // A mule mismatch / area-switch drops the detected name to None. This is
        // not an account switch and must not wipe the main's loaded state.
        t.set_account(None);
        assert!(t.runtime.contains_key(&(52, 1)));
        assert_eq!(t.season_id, Some(52));
        assert!(t.defs.is_some());
        // Re-binding the same main is likewise a no-op.
        t.set_account(Some("Main".into()));
        assert!(t.runtime.contains_key(&(52, 1)));
        assert!(t.defs.is_some());
    }

    // Two trees: the current battlepass (52, fully unlocked) plus a previous
    // "Forgotten Legacy" tree (53, partly unlocked). Mission ids collide across
    // trees (both start at 1), so the tracker keys everything by (tree, mission).
    // Lock state follows the `parentsStr` unlock graph: a mission unlocks once all
    // its parents are claimed/complete, so a past tree that omits an
    // unlocked-but-unstarted mission still resolves it as unlocked.
    const TWO_TREE_DEFS: &str = r#"
    {
      "seasons": [
        {
          "name": "The Mad God's Tempest", "current": 1, "available": 1, "id": 52,
          "missions": [
            { "id": 1, "name": "Blown Away", "desc": "Defeat 75 enemies",
              "conds": [{ "type": 2, "target": "", "amount": 75 }],
              "rewards": [{ "type": 1, "target": "", "amount": 1400000 }] },
            { "id": 2, "name": "Gods of the Gale", "desc": "Defeat 10 gods",
              "conds": [{ "type": 2, "target": "", "amount": 10 }],
              "rewards": [{ "type": 1, "target": "", "amount": 1400000 }] }
          ]
        },
        {
          "name": "The Forgotten Legacy", "current": 0, "available": 1, "id": 53,
          "missions": [
            { "id": 1, "name": "The Realm Remembers", "desc": "Kill any 75 enemies",
              "conds": [{ "type": 2, "target": "", "amount": 75 }],
              "rewards": [{ "type": 1, "target": "", "amount": 1500 }] },
            { "id": 2, "name": "Gods of Old", "desc": "Defeat a boss",
              "conds": [{ "type": 2, "target": "", "amount": 1 }],
              "rewards": [{ "type": 1, "target": "", "amount": 1500 }],
              "parentsStr": "1" },
            { "id": 3, "name": "Trial by Experience", "desc": "Earn XP",
              "conds": [{ "type": 2, "target": "", "amount": 1 }],
              "rewards": [{ "type": 1, "target": "", "amount": 1500 }],
              "parentsStr": "2" }
          ]
        }
      ]
    }"#;

    #[test]
    fn merges_all_active_trees_with_lock_state() {
        let mut t = MissionTracker::new();
        t.apply_definitions(parse_client_seasons(TWO_TREE_DEFS).unwrap());
        // Tree 52 reports both missions (all unlocked). Tree 53 reports only
        // mission 1, claimed/complete. Its mission 2 is omitted from the <Season>
        // block yet must still resolve as unlocked, because its parent (1) is
        // satisfied. Mission 3's parent (2) is unstarted, so it stays locked.
        t.apply_player_missions(realmhound_core::api::parse_player_missions(concat!(
            "<Missions><TreesUnlocked>52,53</TreesUnlocked>",
            "<Season id=\"52\">1,75,1|2,10,0</Season>",
            "<Season id=\"53\">1,75,2</Season></Missions>",
        )));
        let v = t.build_view();

        // Every def mission from both trees is present in one merged list.
        assert_eq!(v.entries.len(), 5);
        // Header still anchors to the current tree.
        assert_eq!(v.season_name, "The Mad God's Tempest");

        // Tree 52 mission 1: server-reported claimable, not locked.
        let m1 = entry_in(&v, 52, 1);
        assert_eq!(m1.state, MissionState::Claimable);
        assert_eq!(m1.tree_name, "The Mad God's Tempest");

        // Forgotten Legacy root (id 1) is unlocked.
        let root = entry_in(&v, 53, 1);
        assert_ne!(root.state, MissionState::Locked);
        assert_eq!(root.tree_name, "The Forgotten Legacy");

        // Graph unlock: mission 2 has a satisfied parent, so it is unlocked even
        // though the server omitted it and it has zero progress (Sprite World case).
        assert_ne!(entry_in(&v, 53, 2).state, MissionState::Locked);
        // Mission 3's parent is unstarted, so it remains locked.
        assert_eq!(entry_in(&v, 53, 3).state, MissionState::Locked);
        // The colliding-id tree-52 missions stay unlocked (never Locked).
        assert_ne!(entry_in(&v, 52, 2).state, MissionState::Locked);
    }

    #[test]
    fn live_delta_applies_to_secondary_tree() {
        // A packet-165 delta for the second tree (season 53) must be accepted and
        // keyed to that tree, and it unlocks a previously-locked mission.
        let mut t = MissionTracker::new();
        t.apply_definitions(parse_client_seasons(TWO_TREE_DEFS).unwrap());
        t.apply_player_missions(realmhound_core::api::parse_player_missions(concat!(
            "<Missions><TreesUnlocked>52,53</TreesUnlocked>",
            "<Season id=\"52\">1,0,0|2,0,0</Season>",
            "<Season id=\"53\">1,0,0</Season></Missions>",
        )));
        // Mission 2 of tree 53 starts locked (omitted from its Season block).
        assert_eq!(entry_in(&t.build_view(), 53, 2).state, MissionState::Locked);
        t.apply_prog(&ProgEntry {
            season_id: 53,
            mission_id: 2,
            values: vec![10],
        });
        let v = t.build_view();
        assert_eq!(entry_in(&v, 53, 2).objectives[0].have, 10);
        assert_ne!(entry_in(&v, 53, 2).state, MissionState::Locked);
    }
}

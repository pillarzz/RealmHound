//! SQLite persistence for reconstructed boss fights.
//!
//! Mirrors the concurrency model of [`crate::loot::LootDatabase`]: a single
//! writer connection owned by the worker thread (WAL + busy timeout) so the UI
//! can open a read-only connection concurrently.

use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Result as SqlResult};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::types::{
    CompletedFight, DamageProvenance, DamageTakenProvenance, FightParticipant, FightSelection,
    ParticipantEndStatus, PetInfo,
};

/// Database schema version for migrations.
pub const SCHEMA_VERSION: i32 = 50;

/// Highest combat-history schema version this build can validate and open. Used
/// by flat-layout migration to reject databases written by a newer build.
pub const SUPPORTED_SCHEMA_VERSION: i32 = SCHEMA_VERSION;

/// Core tables every valid combat-history database must contain, independent of
/// schema version. Used by flat-layout migration to validate a backup.
pub const REQUIRED_TABLES: &[&str] = &["fights", "fight_participants"];

/// How long before a fight starts a correlated loot drop may register (ms).
/// Bag packets occasionally arrive a moment before the death event is parsed.
pub const LOOT_LINK_PRE_MS: i64 = 60_000;

/// How long after a fight ends a correlated loot drop may register (ms). Bags
/// dropped offscreen only appear when the player later walks into the area, so
/// this trailing window is generous.
pub const LOOT_LINK_POST_MS: i64 = 600_000;

/// A stored fight row (header) as read back for the UI.
#[derive(Debug, Clone)]
pub struct FightRecord {
    /// Database row id.
    pub id: i64,
    /// Fight start (epoch millis).
    pub started_at: i64,
    /// Fight end (epoch millis).
    pub ended_at: i64,
    /// Dungeon / map name.
    pub dungeon: String,
    /// When the local player entered this dungeon instance (epoch millis), or
    /// `None` outside a groupable dungeon and for rows recorded before this was
    /// tracked. Drives the encounter card's total dungeon time.
    pub dungeon_entered_at: Option<i64>,
    /// Map seed.
    pub map_seed: i32,
    /// Boss object type.
    pub boss_object_type: i32,
    /// Resolved boss name.
    pub boss_name: String,
    /// Boss max HP.
    pub boss_max_hp: i32,
    /// Boss HP when first observed.
    pub boss_start_hp: i32,
    /// Local player's object id in this fight.
    pub local_object_id: i32,
    /// Local player's character id (0 if unknown / pre-v2 rows).
    pub local_char_id: i32,
    /// Whether the boss was killed.
    pub killed: bool,
    /// Local player's close calls (HP dips below 20% max) during this fight.
    /// 0 for pre-feature rows.
    pub local_close_calls: i32,
    /// For an aggregated aux summary row (e.g. Spectral Key, Overseer Eyesmall),
    /// the number of distinct member instances seen this run; drives the "xN"
    /// count. `None` for ordinary boss fights and legacy rows.
    pub aux_member_count: Option<i32>,
    /// Participants (sorted by damage descending as stored).
    pub participants: Vec<ParticipantRecord>,
}

impl FightRecord {
    /// Fight duration in milliseconds.
    pub fn duration_ms(&self) -> i64 {
        (self.ended_at - self.started_at).max(0)
    }
}

/// A running per-dungeon time counter row. Accumulated independently of the
/// Combat History tracked-boss toggles: every dungeon run entered contributes
/// its elapsed time (entry -> final boss death when killed, else entry -> exit).
#[derive(Debug, Clone)]
pub struct DungeonTimeTotal {
    /// Normalized dungeon / map name.
    pub dungeon: String,
    /// Total time spent across all runs of this dungeon, in milliseconds.
    pub total_time_ms: i64,
    /// Number of runs in which the final boss was killed.
    pub completions: i64,
}

/// A grouped multi-phase encounter: its ordered phases plus an aggregated
/// participant roster summed across all phases.
#[derive(Debug, Clone)]
pub struct EncounterRecord {
    /// Opaque run id (selection identity).
    pub run_id: String,
    /// Curated encounter id.
    pub encounter_id: String,
    /// Canonical display name.
    pub display_name: String,
    /// Dungeon / map name.
    pub dungeon: String,
    /// Run start (epoch millis) - earliest phase start.
    pub started_at: i64,
    /// Run end (epoch millis) - latest phase end.
    pub ended_at: i64,
    /// Whether the anchor (or any phase) was killed.
    pub killed: bool,
    /// Object type of the run anchor (last real boss) for the header icon.
    pub anchor_object_type: i32,
    /// Member phase fights, ordered by start time.
    pub phases: Vec<FightRecord>,
    /// Participant roster aggregated across every phase.
    pub roster: Vec<ParticipantRecord>,
}

impl EncounterRecord {
    /// Sum of the per-phase active durations in milliseconds (excludes the idle
    /// gaps between phases, so encounter DPS is not diluted by downtime).
    pub fn active_duration_ms(&self) -> i64 {
        // Active combat time counts only the "big boss" phases: add-on rows
        // (aggregated aux summaries and treasure crates) overlap the main fight
        // in wall-clock and would inflate the total past the dungeon time. Union
        // the remaining intervals so rare simultaneous bosses are not double
        // counted either.
        let mut intervals: Vec<(i64, i64)> = self
            .phases
            .iter()
            .filter(|p| !is_addon_phase(p))
            .map(|p| (p.started_at, p.ended_at.max(p.started_at)))
            .collect();
        intervals.sort_by_key(|iv| iv.0);
        let mut total = 0i64;
        let mut cur: Option<(i64, i64)> = None;
        for (s, e) in intervals {
            match cur {
                None => cur = Some((s, e)),
                Some((cs, ce)) if s <= ce => cur = Some((cs, ce.max(e))),
                Some((cs, ce)) => {
                    total += ce - cs;
                    cur = Some((s, e));
                }
            }
        }
        if let Some((cs, ce)) = cur {
            total += ce - cs;
        }
        total
    }

    /// Total dungeon time in milliseconds: from the moment the local player
    /// entered the dungeon instance to the final boss death (latest phase end).
    /// Falls back to the earliest phase start for legacy rows lacking a recorded
    /// entry time, and clamps the start to no later than the earliest phase so it
    /// can never read as shorter than the fighting itself.
    pub fn total_dungeon_ms(&self) -> i64 {
        let earliest_phase_start = self
            .phases
            .iter()
            .map(|p| p.started_at)
            .min()
            .unwrap_or(self.started_at)
            .min(self.started_at);
        let effective_start = match self
            .phases
            .iter()
            .filter_map(|p| p.dungeon_entered_at)
            .min()
        {
            Some(entry) => entry.min(earliest_phase_start),
            None => earliest_phase_start,
        };
        (self.ended_at - effective_start).max(0)
    }

    /// Total attributed damage across the roster.
    pub fn total_damage(&self) -> i64 {
        self.roster.iter().map(|p| p.damage).sum()
    }

    /// Total local-player close calls across every phase.
    pub fn total_close_calls(&self) -> i64 {
        self.phases.iter().map(|p| p.local_close_calls as i64).sum()
    }
}

/// A stored participant row.
#[derive(Debug, Clone)]
pub struct ParticipantRecord {
    /// Player object id.
    pub object_id: i32,
    /// Object type (class or skin id).
    pub object_type: i32,
    /// Skin id (0 = default class sprite). `object_type` stays the class id.
    pub skin_id: i32,
    /// Clothing/accessory dye textures (0 = none).
    pub tex1: u32,
    pub tex2: u32,
    /// Player name.
    pub name: String,
    /// Equipment ids (weapon, ability, armor, ring).
    pub equipment: [i32; 4],
    /// Per-slot enchant ids matching `equipment` (weapon, ability, armor, ring).
    pub equipment_enchants: [Vec<u16>; 4],
    /// Attributed damage.
    pub damage: i64,
    /// Attributed hits.
    pub hits: i64,
    /// Whether this is the local player.
    pub is_local: bool,
    /// Damage provenance.
    pub provenance: DamageProvenance,
    /// End-of-fight status (present / died / nexused) for the death icon.
    pub end_status: ParticipantEndStatus,
    /// The player's associated pet, when matched. `None` for rows
    /// recorded before pet capture existed.
    pub pet: Option<PetInfo>,
    /// Total damage taken from fight enemies (post-defense), or `None` when
    /// unavailable / not recorded (pre-feature rows). Rendered as "-" when `None`.
    pub damage_taken: Option<i64>,
    /// How `damage_taken` was derived (meaningful only when `Some`).
    pub damage_taken_provenance: DamageTakenProvenance,
    /// Damage negated by defense and other post-defense mitigation (the "Mell stat"), local player only, or `None` for remote players and
    /// pre-feature rows. Rendered as "-" when `None`.
    pub damage_blocked: Option<i64>,
    /// Damage dealt to Oryx 3 while Guarded (subset of `damage`), or `None` for
    /// pre-feature rows.
    pub guarded_damage: Option<i64>,
    /// Hits landed on Oryx 3 while Guarded (subset of `hits`), or `None` for
    /// pre-feature rows.
    pub guarded_hits: Option<i64>,
}

/// Combat history database manager.
pub struct CombatDatabase {
    conn: Connection,
}

/// A lightweight fight summary for the list view (no full participant rows).
///
/// Loading the list avoids the N+1 cost of pulling every participant for every
/// fight; only [`CombatDatabase::fight_detail`] hydrates the roster.
#[derive(Debug, Clone)]
pub struct FightSummary {
    /// Database row id.
    pub id: i64,
    /// Fight start (epoch millis).
    pub started_at: i64,
    /// Fight end (epoch millis).
    pub ended_at: i64,
    /// Dungeon / map name.
    pub dungeon: String,
    /// Map instance seed (0 when unknown). Correlates a card with Loot History
    /// drops from the same map instance.
    pub map_seed: i32,
    /// Boss object type.
    pub boss_object_type: i32,
    /// Resolved boss name.
    pub boss_name: String,
    /// Boss max HP.
    pub boss_max_hp: i32,
    /// Boss HP when first observed.
    pub boss_start_hp: i32,
    /// Local player's character id (0 if unknown).
    pub local_char_id: i32,
    /// Whether the boss was killed.
    pub killed: bool,
    /// Number of participants attributed to this fight.
    pub participant_count: i64,
    /// Local participant's object type (class/skin id) for the character icon.
    pub local_object_type: Option<i32>,
    /// Local participant's recorded skin id (0 = default), used to render the
    /// card's character icon with its stored appearance when the character is no
    /// longer in the live account cache (e.g. after death).
    pub local_skin_id: i32,
    /// Local participant's recorded cloth dye.
    pub local_tex1: u32,
    /// Local participant's recorded accessory dye.
    pub local_tex2: u32,
    /// Whether the local player died during this fight/encounter.
    pub local_died: bool,
    /// Local player's close calls summed across the row's phases.
    pub local_close_calls: i64,
    /// For a grouped multi-phase encounter, its opaque run id; `None` for a
    /// standalone fight. When set, `id` is 0 and the row represents a group.
    pub encounter_run_id: Option<String>,
    /// For a grouped encounter, its curated id (e.g. "cultist_hideout").
    pub encounter_id: Option<String>,
    /// Number of phases folded into this row (1 for a standalone fight).
    pub phase_count: i64,
    /// Killed bosses in this row as `(object_type, name)` pairs, in kill order.
    /// For a standalone fight this is just the fought boss; for a grouped
    /// encounter it lists the distinct real (non-aux) bosses that were killed.
    /// Drives the boss line on the history card; the object type lets the UI
    /// drop filtered-out members (e.g. treasure crates) from the line.
    pub killed_bosses: Vec<(i32, String)>,
    /// "Flawless" performance marker: set only for Exaltation
    /// dungeon boss cards where at least one participant finished the fight
    /// (never died/nexused), dealt a substantial share of the boss's HP, and
    /// took zero observed damage across every tracked entity in the card. The UI
    /// greys the participant-count marker when set.
    pub flawless: bool,
    /// "Lone fighter" marker: set only for Exaltation dungeon cards where the
    /// boss was killed, the local player finished the fight (present) and dealt
    /// damage, and no other named participant dealt any attributed damage across
    /// the main boss or any additional tracked entity in the card. Computed at
    /// read time from stored participant data, so it applies to legacy cards.
    pub lone_fighter: bool,
    /// "Last hero standing" marker: set only for Exaltation dungeon cards where
    /// the boss was killed, the local player finished (present) at the main-boss
    /// anchor, and every other participant died or nexused (with at least one
    /// such other participant). Computed at read time from stored data.
    pub last_hero_standing: bool,
    /// "Most damage taken" marker: set only for Exaltation dungeon cards where the
    /// boss was killed and the local player took strictly more damage than every
    /// other detected teammate (with at least one other participant). Computed at
    /// read time from stored data.
    pub most_damage_taken: bool,
}

impl FightSummary {
    /// Fight duration in milliseconds.
    pub fn duration_ms(&self) -> i64 {
        (self.ended_at - self.started_at).max(0)
    }

    /// The selection identity for this row (group run id, or single fight id).
    pub fn selection(&self) -> FightSelection {
        match &self.encounter_run_id {
            Some(run) => FightSelection::Encounter(run.clone()),
            None => FightSelection::Single(self.id),
        }
    }

    /// Whether this row groups multiple phases of an encounter.
    pub fn is_encounter(&self) -> bool {
        self.encounter_run_id.is_some()
    }
}

/// Aggregated local-participant stats for a grouped encounter, gathered in one
/// pass over its phases. Backs the character icon and the "Died" status marker.
#[derive(Debug, Clone, Default)]
struct LocalStats {
    participant_count: i64,
    local_object_type: Option<i32>,
    local_skin_id: i32,
    local_tex1: u32,
    local_tex2: u32,
    local_died: bool,
}

/// Filters for querying stored fights. All fields are optional; an empty query
/// returns the most recent fights.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FightQuery {
    /// Free-text match against boss name or dungeon (case-insensitive LIKE).
    pub text: Option<String>,
    /// Exact dungeon name.
    pub dungeon: Option<String>,
    /// Exact boss object type.
    pub boss_object_type: Option<i32>,
    /// Exact local character id (0/unknown fights are excluded when set).
    pub char_id: Option<i32>,
    /// Exact participant IGN: restrict to fights this player took part in.
    pub player_name: Option<String>,
    /// Exact participant count: restrict to cards whose roster size matches.
    /// Standalone fights count all participant rows; grouped encounters count
    /// distinct object ids across every phase, matching the displayed count.
    pub participant_count: Option<i64>,
    /// Only fights started at or after this epoch-ms.
    pub after: Option<i64>,
    /// Only fights started at or before this epoch-ms.
    pub before: Option<i64>,
    /// Restrict to these boss-group keys (see `BossGroup::as_str`). `None` (or
    /// empty) applies no group filter. Fights with a NULL `boss_group` (ungated)
    /// are excluded whenever a non-empty filter is set.
    pub groups: Option<Vec<String>>,
    /// Secret-stat card filters. When any are set, `list_fights` keeps only cards
    /// matching at least one of the enabled flags (union), computed at read time.
    /// "Lone fighter" card.
    pub filter_lone_fighter: bool,
    /// "Last hero standing" card.
    pub filter_last_hero: bool,
    /// "Most damage taken" card.
    pub filter_most_damage_taken: bool,
    /// Card where the local player had at least one close call.
    pub filter_close_calls: bool,
}

impl CombatDatabase {
    /// Open or create the combat database (writer connection) at an explicit path.
    pub fn open_writer(path: &Path) -> SqlResult<Self> {
        Self::open_writer_with_loot(path, None)
    }

    /// Open or create the combat database (writer connection), pairing it with a
    /// loot-history database so migrations that correlate against recorded loot
    /// (the v50 Legacy Lair of Draconis completion backfill) can read it. Pass
    /// `None` for maintenance writers that have no paired loot DB; loot-dependent
    /// migrations then defer to a later paired open.
    pub fn open_writer_with_loot(path: &Path, loot_path: Option<&Path>) -> SqlResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        Self::apply_concurrency_pragmas(&conn)?;
        let mut db = Self { conn };
        db.initialize(loot_path)?;
        Ok(db)
    }

    /// Open a read-only connection (for the UI) at an explicit path. The writer
    /// must have created the file/schema first.
    pub fn open_reader_at(path: &Path) -> SqlResult<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(Self { conn })
    }

    /// Open an in-memory database (for tests).
    pub fn open_in_memory() -> SqlResult<Self> {
        let conn = Connection::open_in_memory()?;
        let mut db = Self { conn };
        db.initialize(None)?;
        Ok(db)
    }

    /// Checkpoint the WAL and close, so a following relaunch leaves no `-wal` tail.
    pub fn close(self) -> SqlResult<()> {
        let _: (i64, i64, i64) =
            self.conn
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?;
        self.conn.close().map_err(|(_, err)| err)
    }

    fn apply_concurrency_pragmas(conn: &Connection) -> SqlResult<()> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let _: String = conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
        Ok(())
    }

    fn initialize(&mut self, loot_path: Option<&Path>) -> SqlResult<()> {
        let version: i32 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or(0);
        if version == 0 {
            self.create_tables()?;
            self.conn
                .execute_batch(&format!("PRAGMA user_version = {}", SCHEMA_VERSION))?;
        } else if version < SCHEMA_VERSION {
            self.migrate(version)?;
        }
        // Retroactively fold trustworthy fights recorded before their encounter
        // was added to the curated registry.
        self.backfill_encounter_runs()?;
        // Legacy Lair of Draconis dragons self-destruct into loot balloon chests,
        // so their fights are never scored as kills. When paired with the loot DB,
        // mark such dragons complete from the chest bags they dropped. Idempotent
        // and best-effort, so it runs every open until the loot DB is available.
        self.complete_legacy_lod_dragons_from_loot(loot_path)?;
        // Legacy Lair of Draconis' Ivory Wyvern portal only drops once all four
        // dragons are defeated, so an Ivory Wyvern fight proves the preceding
        // Lair run was a full clear. Complete such runs from that evidence.
        self.backfill_legacy_lod_from_ivory()?;
        Ok(())
    }

    /// Group already-recorded fights whose boss belongs to a curated multi-phase
    /// encounter but were stored before that encounter existed in the registry
    /// (so `encounter_run_id IS NULL`).
    ///
    /// Idempotent: only ungrouped curated-member fights with the same trustworthy
    /// evidence required by the v5 migration are touched. A large time gap starts
    /// a fresh run so a recycled seed across sessions cannot merge unrelated clears.
    fn backfill_encounter_runs(&mut self) -> SqlResult<()> {
        // Beyond this gap, two same-seed fights are treated as separate runs.
        const RUN_GAP_MS: i64 = 30 * 60 * 1000;

        struct Row {
            id: i64,
            otype: i32,
            dungeon: String,
            seed: i32,
            char_id: i32,
            started: i64,
            ended: i64,
            killed: bool,
        }
        let rows: Vec<Row> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, boss_object_type, dungeon, map_seed, local_char_id, started_at, ended_at, killed
                 FROM fights WHERE encounter_run_id IS NULL ORDER BY map_seed, started_at",
            )?;
            let it = stmt.query_map([], |r| {
                Ok(Row {
                    id: r.get(0)?,
                    otype: r.get(1)?,
                    dungeon: r.get(2)?,
                    seed: r.get(3)?,
                    char_id: r.get(4)?,
                    started: r.get(5)?,
                    ended: r.get(6)?,
                    killed: r.get::<_, i32>(7)? != 0,
                })
            })?;
            it.collect::<SqlResult<Vec<_>>>()?
        };

        struct Run {
            run_id: String,
            enc_id: String,
            dungeon: String,
            ids: Vec<i64>,
            start: i64,
            end: i64,
            killed: bool,
        }
        let mut open: HashMap<(String, i32, String, i32), Run> = HashMap::new();
        let mut runs: Vec<Run> = Vec::new();
        for row in rows {
            let Some(enc) = crate::assets::encounter_for_boss_type(row.otype) else {
                continue;
            };
            if row.char_id == 0
                || row.seed == 0
                || !super::tracker::is_groupable_dungeon(&row.dungeon)
            {
                continue;
            }
            let key = (
                enc.id.to_string(),
                row.char_id,
                row.dungeon.clone(),
                row.seed,
            );
            let start_new = match open.get(&key) {
                Some(r) => row.started - r.end > RUN_GAP_MS,
                None => true,
            };
            if start_new {
                if let Some(prev) = open.remove(&key) {
                    runs.push(prev);
                }
                open.insert(
                    key,
                    Run {
                        run_id: format!("bf-{}-{:x}", enc.id, row.started),
                        enc_id: enc.id.to_string(),
                        dungeon: row.dungeon,
                        ids: vec![row.id],
                        start: row.started,
                        end: row.ended,
                        killed: row.killed,
                    },
                );
            } else {
                let r = open.get_mut(&key).expect("open run exists for key");
                r.ids.push(row.id);
                r.start = r.start.min(row.started);
                r.end = r.end.max(row.ended);
                r.killed |= row.killed;
            }
        }
        runs.extend(open.into_values());
        if runs.is_empty() {
            return Ok(());
        }

        let tx = self.conn.transaction()?;
        for run in &runs {
            tx.execute(
                r#"INSERT OR REPLACE INTO encounter_runs
                   (run_id, encounter_id, dungeon, started_at, ended_at, killed)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
                params![
                    run.run_id,
                    run.enc_id,
                    run.dungeon,
                    run.start,
                    run.end,
                    run.killed as i32
                ],
            )?;
            for id in &run.ids {
                tx.execute(
                    "UPDATE fights SET encounter_id = ?1, encounter_run_id = ?2 WHERE id = ?3",
                    params![run.enc_id, run.run_id, id],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Apply incremental migrations from `from_version` to the current schema.
    fn migrate(&mut self, from_version: i32) -> SqlResult<()> {
        if from_version < 2 {
            // v1 -> v2: add the stable local character id (0 for pre-existing rows).
            let tx = self.conn.transaction()?;
            tx.execute(
                "ALTER TABLE fights ADD COLUMN local_char_id INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
            tx.execute_batch("PRAGMA user_version = 2")?;
            tx.commit()?;
        }
        if from_version < 3 {
            // v2 -> v3: multi-phase encounter grouping. Existing rows keep NULL
            // encounter fields and continue to behave as standalone fights.
            let tx = self.conn.transaction()?;
            tx.execute_batch(
                r#"
                ALTER TABLE fights ADD COLUMN encounter_id TEXT;
                ALTER TABLE fights ADD COLUMN encounter_run_id TEXT;
                CREATE TABLE IF NOT EXISTS encounter_runs (
                    run_id TEXT PRIMARY KEY,
                    encounter_id TEXT NOT NULL,
                    dungeon TEXT NOT NULL,
                    started_at INTEGER NOT NULL,
                    ended_at INTEGER NOT NULL,
                    killed INTEGER NOT NULL DEFAULT 0
                );
                CREATE INDEX IF NOT EXISTS idx_fights_run ON fights(encounter_run_id);
                PRAGMA user_version = 3;
                "#,
            )?;
            tx.commit()?;
        }
        if from_version < 4 {
            // v3 -> v4: per-participant equipment enchantments. Legacy rows keep
            // NULL and hydrate as four empty enchant slots.
            let tx = self.conn.transaction()?;
            tx.execute_batch(
                r#"
                ALTER TABLE fight_participants ADD COLUMN enchants TEXT;
                PRAGMA user_version = 4;
                "#,
            )?;
            tx.commit()?;
        }
        if from_version < 5 {
            // v4 -> v5: group every boss of a dungeon instance under one card.
            // Historical fights were grouped per curated encounter (or not at
            // all); re-key trustworthy fights by dungeon instance so an
            // instance's bosses share one run. Runs entirely in-transaction.
            self.migrate_generic_grouping()?;
            self.conn.execute_batch("PRAGMA user_version = 5")?;
        }
        if from_version < 6 {
            // v5 -> v6: prior startup backfills accepted weak evidence and could
            // merge unrelated legacy fights. Remove their synthetic `bf-`
            // assignments, then rebuild all trustworthy runs with the v5 rules.
            let tx = self.conn.transaction()?;
            tx.execute(
                "UPDATE fights SET encounter_id = NULL, encounter_run_id = NULL
                 WHERE encounter_run_id LIKE 'bf-%'",
                [],
            )?;
            tx.execute("DELETE FROM encounter_runs WHERE run_id LIKE 'bf-%'", [])?;
            tx.commit()?;
            self.migrate_generic_grouping()?;
            self.conn.execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_encounter_runs_recent
                 ON encounter_runs(ended_at DESC, started_at DESC);
                 PRAGMA user_version = 6;",
            )?;
        }
        if from_version < 7 {
            // v6 -> v7: persisted boss-group classification for Combat History
            // settings/filters. Add the column, backfill every row
            // from its stored dungeon/boss, then create the lookup index.
            if !self.column_exists("fights", "boss_group")? {
                self.conn
                    .execute_batch("ALTER TABLE fights ADD COLUMN boss_group TEXT;")?;
            }
            self.backfill_boss_groups()?;
            self.conn.execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_fights_group ON fights(boss_group);
                 PRAGMA user_version = 7;",
            )?;
        }
        if from_version < 8 {
            // v7 -> v8: the Biome Minibosses group was split into three Heroes of
            // Oryx tiers, and Beach Bum moved from Adept Encounters
            // to Adept Heroes. Recompute every row's boss_group so historical
            // fights land in the new groups.
            self.reclassify_boss_groups()?;
            self.conn.execute_batch("PRAGMA user_version = 8")?;
        }
        if from_version < 9 {
            // v8 -> v9: store each participant's skin id and dye textures so the
            // DPS breakdown can render skinned/dyed player sprites.
            // Old rows default to 0 (class sprite, no dye).
            let mut batch = String::new();
            if !self.column_exists("fight_participants", "skin_id")? {
                batch.push_str(
                    "ALTER TABLE fight_participants ADD COLUMN skin_id INTEGER NOT NULL DEFAULT 0;",
                );
            }
            if !self.column_exists("fight_participants", "tex1")? {
                batch.push_str(
                    "ALTER TABLE fight_participants ADD COLUMN tex1 INTEGER NOT NULL DEFAULT 0;",
                );
            }
            if !self.column_exists("fight_participants", "tex2")? {
                batch.push_str(
                    "ALTER TABLE fight_participants ADD COLUMN tex2 INTEGER NOT NULL DEFAULT 0;",
                );
            }
            batch.push_str("PRAGMA user_version = 9;");
            self.conn.execute_batch(&batch)?;
        }
        if from_version < 10 {
            // v9 -> v10: member adds of grouped realm encounters whose own
            // object type lacks the encounter's group label (e.g. Pentaract
            // Towers vs. the invisible New Pentaract marker) were classified as
            // ungated and dropped out of every group filter. `boss_group` now
            // falls back to the encounter anchor's group, so recompute every row
            // to make historical encounter fights filterable again.
            self.reclassify_boss_groups()?;
            self.conn.execute_batch("PRAGMA user_version = 10")?;
        }
        if from_version < 11 {
            // v10 -> v11: per-participant death/nexus status. Legacy
            // rows default to "present" (no icon), which is the correct fallback
            // since the status was not recorded before.
            let mut batch = String::new();
            if !self.column_exists("fight_participants", "end_status")? {
                batch.push_str(
                    "ALTER TABLE fight_participants ADD COLUMN end_status TEXT NOT NULL DEFAULT 'present';",
                );
            }
            if !self.column_exists("fight_participants", "grave_type")? {
                batch.push_str(
                    "ALTER TABLE fight_participants ADD COLUMN grave_type INTEGER NOT NULL DEFAULT 0;",
                );
            }
            batch.push_str("PRAGMA user_version = 11;");
            self.conn.execute_batch(&batch)?;
        }
        if from_version < 12 {
            // v11 -> v12: seasonal/special bosses and treasure crates gained
            // their own groups. Reclassify every row so historical
            // special bosses and crates land in the new categories and stay
            // filterable / gateable. Crates remain grouped inside their dungeon
            // cards; the filter hides them at read time when unchecked.
            self.reclassify_boss_groups()?;
            self.conn.execute_batch("PRAGMA user_version = 12")?;
        }
        if from_version < 13 {
            // v12 -> v13: retroactively group historical realm set-piece
            // encounters (Pentaract, Assembled Giant, ...) whose member fights
            // were recorded standalone before realm grouping covered them. New
            // fights already group at write time; this brings history in line.
            self.regroup_realm_encounters()?;
            self.conn.execute_batch("PRAGMA user_version = 13")?;
        }
        if from_version < 14 {
            // v13 -> v14: fold leftover standalone fights in groupable dungeons
            // (notably treasure crates like Sunken Treasure / Infested Chest) into their dungeon instance's card. Older
            // builds recorded these with a NULL run id; new fights already group
            // at write time. This brings history in line.
            self.regroup_dungeon_instances()?;
            self.conn.execute_batch("PRAGMA user_version = 14")?;
        }
        if from_version < 15 {
            // v14 -> v15: reclassify boss groups so historical realm encounters
            // missing their catalog label (New Ghost Ship, whose ObjectID entry
            // ships with empty labels) pick up the curated Adept Encounter
            // override and stop being hidden by every group filter.
            self.reclassify_boss_groups()?;
            self.conn.execute_batch("PRAGMA user_version = 15")?;
        }
        if from_version < 16 {
            // v15 -> v16: add pet columns so newly recorded fights can show each
            // player's pet. Old rows default to no pet. Guarded so a
            // fresh schema (which already has the columns) can re-run migrations.
            if !self.column_exists("fight_participants", "pet_type")? {
                self.conn.execute_batch(
                    "ALTER TABLE fight_participants ADD COLUMN pet_type INTEGER NOT NULL DEFAULT 0;",
                )?;
            }
            if !self.column_exists("fight_participants", "pet_abilities")? {
                self.conn.execute_batch(
                    "ALTER TABLE fight_participants ADD COLUMN pet_abilities TEXT;",
                )?;
            }
            self.conn.execute_batch("PRAGMA user_version = 16;")?;
        }
        if from_version < 17 {
            // v16 -> v17: reclassify boss groups for curated type overrides added
            // for the realm bosses whose ObjectID entry lacks a tier label (Alien
            // Reactor, Killer Bee Nest + Beehemoths, Janus / Stone Guardians, and
            // Slumbering Dragon). Recompute every row so historical fights land in
            // their correct group and stay filterable.
            self.reclassify_boss_groups()?;
            self.conn.execute_batch("PRAGMA user_version = 17")?;
        }
        if from_version < 18 {
            // v17 -> v18: retroactively fold historical Slumbering Dragon fights
            // and their Dragon's Treasure into one realm-grouped card. New fights
            // already group at write time; this brings history in line.
            self.regroup_realm_encounters()?;
            self.conn.execute_batch("PRAGMA user_version = 18")?;
        }
        if from_version < 19 {
            // v18 -> v19: Oryx the Mad God 2 (2354) fought from the realm is the
            // Wine Cellar boss. Relabel historical fights logged as "Realm" to
            // "Wine Cellar" so they classify as Expert dungeon bosses and show
            // the Wine Cellar portal / card, then reclassify and fold them into a
            // Wine Cellar dungeon instance card.
            self.conn.execute(
                "UPDATE fights SET dungeon = 'Wine Cellar'
                 WHERE boss_object_type = 2354 AND dungeon <> 'Wine Cellar'",
                [],
            )?;
            self.reclassify_boss_groups()?;
            self.regroup_dungeon_instances()?;
            self.conn.execute_batch("PRAGMA user_version = 19")?;
        }
        if from_version < 20 {
            // v19 -> v20: Janus the Doorwarden (8200) and the Stone Guardians
            // (3448/3449) fought from the realm belong to Oryx's Castle. Relabel
            // historical fights logged as "Realm" so they classify as Adept
            // dungeon bosses and show the Oryx's Castle portal / card, then
            // reclassify and fold them into a dungeon instance card.
            self.conn.execute(
                "UPDATE fights SET dungeon = 'Oryx''s Castle'
                 WHERE boss_object_type IN (8200, 3448, 3449)
                   AND dungeon <> 'Oryx''s Castle'",
                [],
            )?;
            self.reclassify_boss_groups()?;
            self.regroup_dungeon_instances()?;
            self.conn.execute_batch("PRAGMA user_version = 20")?;
        }
        if from_version < 21 {
            // v20 -> v21: the Satellite Cores in the Alien Invasion dungeons
            // (Malogia / Forax / Untaris / Katalund and their Neo variants) are
            // lootable crates, not dungeon bosses. They are now curated treasure
            // crates; reclassify historical fights so they land in the Treasure
            // crates category.
            self.reclassify_boss_groups()?;
            self.conn.execute_batch("PRAGMA user_version = 21")?;
        }
        if from_version < 22 {
            // v21 -> v22: per-participant damage taken (nullable, plus a
            // provenance marker). Legacy rows keep NULL so the UI shows "-"
            // rather than a false zero.
            let mut batch = String::new();
            if !self.column_exists("fight_participants", "damage_taken")? {
                batch.push_str("ALTER TABLE fight_participants ADD COLUMN damage_taken INTEGER;");
            }
            if !self.column_exists("fight_participants", "damage_taken_provenance")? {
                batch.push_str(
                    "ALTER TABLE fight_participants ADD COLUMN damage_taken_provenance TEXT;",
                );
            }
            batch.push_str("PRAGMA user_version = 22;");
            self.conn.execute_batch(&batch)?;
        }
        if from_version < 23 {
            // v22 -> v23: per-participant "Guarded" damage/hits for Oryx 3.
            // Nullable so legacy rows show "-" rather than 0.
            let mut batch = String::new();
            if !self.column_exists("fight_participants", "guarded_damage")? {
                batch.push_str("ALTER TABLE fight_participants ADD COLUMN guarded_damage INTEGER;");
            }
            if !self.column_exists("fight_participants", "guarded_hits")? {
                batch.push_str("ALTER TABLE fight_participants ADD COLUMN guarded_hits INTEGER;");
            }
            batch.push_str("PRAGMA user_version = 23;");
            self.conn.execute_batch(&batch)?;
        }
        if from_version < 24 {
            // v23 -> v24: Moonlight Village adjustments. The
            // Challenge Gate and MV Easy Dropper are non-boss objects that were
            // tracked as fights before they were curated out; delete those
            // historical rows (and their participants) and drop any run left
            // empty. The MV Fishing Loot objects are now treasure crates;
            // reclassify historical fights so they land in the Treasure crates
            // category (as the Satellite Core reclassification did in v21).
            self.purge_fights_by_type(&[20553, 20577])?;
            self.reclassify_boss_groups()?;
            self.conn.execute_batch("PRAGMA user_version = 24")?;
        }
        if from_version < 25 {
            // v24 -> v25: more encounter minion spam. Well of Souls
            // Skeleton (Possessed Skeleton), Lich King Grave, Flying Behemoth
            // Tornado, Sentient Monolith Protector, and Aerial Warship Crew are
            // label-less adds that were tracked as their own fights before being
            // curated out; purge those historical rows and recompute run spans.
            self.purge_fights_by_type(&[16933, 16971, 34462, 34587, 51077])?;
            self.conn.execute_batch("PRAGMA user_version = 25")?;
        }
        if from_version < 26 {
            // v25 -> v26: Angry Hornets are Hornet's Nest adds now curated out and
            // aggregated into the Nest's card. Unlike the label-less
            // spam purged above, an Angry Hornet is a Hornet's Nest encounter
            // member, so fold historical standalone rows into the Nest's realm
            // encounter run instead of deleting them.
            self.regroup_realm_encounters()?;
            self.conn.execute_batch("PRAGMA user_version = 26")?;
        }
        if from_version < 27 {
            // v26 -> v27: Legion Missionary Holy/Chaos Orbs are Beacon Guardian
            // adds now curated out and aggregated into the Missionary's card.
            // Like the Angry Hornets, they are encounter members, so
            // fold historical standalone orb/boss rows into the Legion Missionary
            // realm encounter run instead of deleting them.
            self.regroup_realm_encounters()?;
            self.conn.execute_batch("PRAGMA user_version = 27")?;
        }
        if from_version < 28 {
            // v27 -> v28: per-participant "Damage blocked" (the "Mell stat"): damage negated by defense and other post-defense
            // mitigation, local player only. Nullable so legacy rows show "-"
            // rather than a false zero.
            let mut batch = String::new();
            if !self.column_exists("fight_participants", "damage_blocked")? {
                batch.push_str("ALTER TABLE fight_participants ADD COLUMN damage_blocked INTEGER;");
            }
            batch.push_str("PRAGMA user_version = 28;");
            self.conn.execute_batch(&batch)?;
        }
        if from_version < 29 {
            // v28 -> v29: Adult Baneserpent head/neck/telegraph segments share the DisplayId "Adult Baneserpent" but carry no boss
            // labels; a lingering segment fought without the real body (34456) in
            // view was tracked as a spurious "Escaped" card. Now curated out, so
            // purge those historical non-body rows (the body's cards stay).
            self.purge_fights_by_type(&[34457, 34458, 34459, 34467, 34468, 34469, 34474])?;
            self.conn.execute_batch("PRAGMA user_version = 29")?;
        }
        if from_version < 30 {
            // v29 -> v30: the Goblin Patriarch Adept Encounter's "Goblin Villager"
            // minions (34552, 34556) carry no boss labels but event-scale above
            // the HP fallback, so they were tracked as spurious cards. Now curated
            // out; purge those historical rows and recompute run spans.
            self.purge_fights_by_type(&[34552, 34556])?;
            self.conn.execute_batch("PRAGMA user_version = 30")?;
        }
        if from_version < 31 {
            // v30 -> v31: the Rat Extermination minigame's City Rat minions
            // (18000, 18003, 18047, 18049) carry no boss labels but event-scale
            // above the HP fallback, so they were tracked as spurious cards. Now
            // curated out; purge those historical rows and recompute run spans.
            // Only the Mammoth City Rat (18007) stays tracked.
            self.purge_fights_by_type(&[18000, 18003, 18047, 18049])?;
            self.conn.execute_batch("PRAGMA user_version = 31")?;
        }
        if from_version < 32 {
            // v31 -> v32: rename the Marble Colossus survival-phase segment labels
            // from "First Coming" / "Second Coming" to "Pre-survival" /
            // "Post-survival" on historical records so old cards match the new
            // naming.
            self.conn.execute(
                "UPDATE fights SET boss_name = 'Marble Colossus (Pre-survival)' \
                 WHERE boss_name = 'Marble Colossus (First Coming)'",
                [],
            )?;
            self.conn.execute(
                "UPDATE fights SET boss_name = 'Marble Colossus (Post-survival)' \
                 WHERE boss_name = 'Marble Colossus (Second Coming)'",
                [],
            )?;
            self.conn.execute_batch("PRAGMA user_version = 32")?;
        }
        if from_version < 33 {
            // v32 -> v33: more Rat Extermination minigame minions leak past the HP
            // fallback with no boss labels (Small Rat 18001, Medium Rat 18002/18048,
            // Golden Rat 18050), tracked as spurious cards. Now curated out; purge
            // those historical rows and recompute run spans. Only the Mammoth Rat
            // (18007) stays tracked.
            self.purge_fights_by_type(&[18001, 18002, 18048, 18050])?;
            self.conn.execute_batch("PRAGMA user_version = 33")?;
        }
        if from_version < 34 {
            // v33 -> v34: three loot crates were mis-tracked. DS Master Rat Box
            // (46404) shipped with MINIBOSS/BOSSFIGHT labels so it logged as a
            // miniboss; the Ruins Lamp / Magic Lamp (28867) and DS Golden Rat
            // (29023) are minion loot piñatas. All three are now curated treasure
            // crates, so recompute every fight's group to move existing rows into
            // the Treasure crates category.
            self.reclassify_boss_groups()?;
            self.conn.execute_batch("PRAGMA user_version = 34")?;
        }
        if from_version < 35 {
            // v34 -> v35: add the per-fight local close-call counter.
            // Existing rows predate the feature, so they default to 0.
            if !self.column_exists("fights", "local_close_calls")? {
                self.conn.execute_batch(
                    "ALTER TABLE fights ADD COLUMN local_close_calls INTEGER NOT NULL DEFAULT 0;",
                )?;
            }
            self.conn.execute_batch("PRAGMA user_version = 35")?;
        }
        if from_version < 36 {
            // v35 -> v36: ledger for the lifetime "Lone fighter" / "Last hero
            // standing" awards. Kept independent of `fights` (no FK) so a card
            // deletion never revokes an award, and each card is counted once.
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS stat_awards (
                    card_key TEXT PRIMARY KEY,
                    char_id INTEGER NOT NULL,
                    lone INTEGER NOT NULL DEFAULT 0,
                    last INTEGER NOT NULL DEFAULT 0
                );",
            )?;
            self.conn.execute_batch("PRAGMA user_version = 36")?;
        }
        if from_version < 37 {
            // v36 -> v37: extend the award ledger with the lifetime "Most damage
            // taken" tally. Older ledgers add the column defaulting to 0; the next
            // reconcile backfills it from history.
            if !self.column_exists("stat_awards", "most")? {
                self.conn.execute_batch(
                    "ALTER TABLE stat_awards ADD COLUMN most INTEGER NOT NULL DEFAULT 0;",
                )?;
            }
            self.conn.execute_batch("PRAGMA user_version = 37")?;
        }
        if from_version < 38 {
            // v37 -> v38: the Spectral Penitentiary CellBranch tutorial objects
            // (24454-24459) and the Administration Turret (24061) are label-less
            // Characters that clear the HP fallback, so they were tracked as
            // spurious cards before being curated out. Purge those historical
            // rows and recompute run spans.
            self.purge_fights_by_type(&[24061, 24454, 24455, 24456, 24457, 24458, 24459])?;
            self.conn.execute_batch("PRAGMA user_version = 38")?;
        }
        if from_version < 39 {
            // v38 -> v39: aggregated aux summary rows (Spectral Key, Overseer
            // Eyesmall, ...) now carry a distinct-member count so the row can show
            // "xN". Nullable; legacy and non-aux rows stay NULL.
            if !self.column_exists("fights", "aux_member_count")? {
                self.conn
                    .execute_batch("ALTER TABLE fights ADD COLUMN aux_member_count INTEGER;")?;
            }
            self.conn.execute_batch("PRAGMA user_version = 39")?;
        }
        if from_version < 40 {
            // v39 -> v40: re-purge the Administration Turret (24061) and CellBranch
            // tutorial objects (24454-24459). The v38 purge gained the turret type
            // after some databases had already advanced past v38, so those rows
            // survived; this migration guarantees the removal runs once for them.
            self.purge_fights_by_type(&[24061, 24454, 24455, 24456, 24457, 24458, 24459])?;
            self.conn.execute_batch("PRAGMA user_version = 40")?;
        }
        if from_version < 41 {
            // v40 -> v41: SpecPen branch-door / cell / floor switches and the
            // Gretch-branch gravestones are label-less Characters that cleared the
            // HP fallback and logged as spurious cards before being curated out.
            // Purge those historical rows and recompute run spans. The Doctor
            // Lobotomik "Sentipede" head/segment rows are intentionally NOT purged:
            // the display-time aux collapse folds them into one head row while
            // keeping their damage, so deleting them would lose that damage.
            self.purge_fights_by_type(&[
                23708, 23747, 23840, 23932, 24412, 44354, 44355, 44356, 44357, 44358, 44359, 44360,
                44361,
            ])?;
            self.conn.execute_batch("PRAGMA user_version = 41")?;
        }
        if from_version < 42 {
            // v41 -> v42: record when the local player entered a dungeon instance
            // so the encounter card can measure total dungeon time from entry to
            // the final boss death instead of from first boss engagement. Legacy
            // rows keep NULL and fall back to the earliest phase start.
            if !self.column_exists("fights", "dungeon_entered_at")? {
                self.conn
                    .execute_batch("ALTER TABLE fights ADD COLUMN dungeon_entered_at INTEGER;")?;
            }
            self.conn.execute_batch("PRAGMA user_version = 42")?;
        }
        if from_version < 43 {
            // v42 -> v43: running per-dungeon time counter, independent of the
            // Combat History tracked-boss toggles. Accumulates time spent per
            // dungeon instance (entry -> final boss death when killed, else entry
            // -> exit for runs the player left/died in), surfaced in Trophy Hall.
            self.conn.execute_batch(
                r#"CREATE TABLE IF NOT EXISTS dungeon_time_totals (
                    dungeon TEXT PRIMARY KEY,
                    total_time_ms INTEGER NOT NULL DEFAULT 0,
                    completions INTEGER NOT NULL DEFAULT 0
                );"#,
            )?;
            // One-time backfill from existing Combat History so users without a
            // RealmShark export still see time totals instead of a wall of zeros.
            self.backfill_dungeon_time_totals()?;
            self.conn.execute_batch("PRAGMA user_version = 43")?;
        }
        if from_version < 44 {
            // v43 -> v44: Alien Reactor Adept (56341) and Veteran (56342) were
            // reclassified from Adept/Veteran Encounters to Adept/Veteran Heroes
            // of Oryx. Recompute stored fights so legacy runs match the new
            // grouping in Combat History filters and tracking settings.
            self.reclassify_boss_groups()?;
            self.conn.execute_batch("PRAGMA user_version = 44")?;
        }
        if from_version < 45 {
            // v44 -> v45: persist solo HP-gap reconciliation evidence on new
            // fights. `reached_zero` (boss HP truly hit 0) and `joined_late`
            // (local player arrived mid-fight) let a future migration safely
            // reconcile stored runs. Existing rows predate the signals and stay
            // as-is (forward-only); they default to 0.
            if !self.column_exists("fights", "reached_zero")? {
                self.conn.execute_batch(
                    "ALTER TABLE fights ADD COLUMN reached_zero INTEGER NOT NULL DEFAULT 0;",
                )?;
            }
            if !self.column_exists("fights", "joined_late")? {
                self.conn.execute_batch(
                    "ALTER TABLE fights ADD COLUMN joined_late INTEGER NOT NULL DEFAULT 0;",
                )?;
            }
            self.conn.execute_batch("PRAGMA user_version = 45")?;
        }
        if from_version < 46 {
            // v45 -> v46: relabel legacy Marble Colossus cards whose survival
            // split never fired (an observed heal-back HP tick was culled), so
            // the whole fight was stored as a single "(Pre-survival)" segment.
            // Retroactive splitting is impossible from stored aggregates, so we
            // just drop the misleading suffix on high-confidence collapsed
            // full-kills: type 45073, seen from full HP, no "(Post-survival)"
            // sibling in the run, and -- the decisive signal -- the local player
            // SURVIVED to the kill (end_status = 'present'). A survived kill spans
            // both phases, so the collapsed card really is the whole fight. A
            // death / nexus in either phase (end_status 'died' / 'nexused') keeps
            // the "(Pre-survival)" label: that run was abandoned, not completed.
            // Idempotent (the suffix is gone after one run). Guarded on the
            // end_status column, which every real DB has had since v11; the
            // check only skips artificial minimal-schema fixtures in tests.
            if self.column_exists("fight_participants", "end_status")? {
                self.conn.execute_batch(
                    "UPDATE fights
                     SET boss_name = substr(boss_name, 1, length(boss_name) - length(' (Pre-survival)'))
                     WHERE boss_object_type = 45073
                       AND killed = 1
                       AND boss_start_hp = boss_max_hp
                       AND boss_name LIKE '% (Pre-survival)'
                       AND encounter_run_id IS NOT NULL
                       AND EXISTS (
                         SELECT 1 FROM fight_participants p
                         WHERE p.fight_id = fights.id
                           AND fights.local_object_id != 0
                           AND p.object_id = fights.local_object_id
                           AND p.end_status = 'present')
                       AND NOT EXISTS (
                         SELECT 1 FROM fights f2
                         WHERE f2.encounter_run_id = fights.encounter_run_id
                           AND f2.boss_name LIKE '% (Post-survival)');",
                )?;
            }
            self.conn.execute_batch("PRAGMA user_version = 46")?;
        }
        if from_version < 47 {
            // v46 -> v47: Legacy The Shatters and Legacy Woodland Labyrinth were
            // brought back this season. Their "Retro" arena mobs all cleared the
            // HP fallback and bloated the dungeon cards. Only the real bosses are
            // tracked now (Shatters: Forgotten Sentinel, Twilight Archmage +
            // aux-folded Blizzard/Inferno, and the King 52560; Woodland: the three
            // Megamoth forms), so purge every curated-out Retro entity from stored
            // runs and recompute run spans. Blizzard/Inferno (52563/52564) are
            // kept: they fold into the Twilight Archmage card as aux rows at
            // display time. The King (52560) is a real boss and must NOT be
            // purged. The Megamoth forms are kept and re-anchored on the final
            // Murderous Megamoth by the display-time grouping.
            self.purge_fights_by_type(&[
                // Legacy The Shatters trash.
                52513, 52522, 52531, 52532, 52533, 52534, 52535, 52536, 52537, 52538, 52539, 52540,
                52541, 52542, 52543, 52544, 52545, 52546, 52547, 52548, 52549, 52550, 52551, 52552,
                52553, 52554, 52555, 52558, 52559, 52562, 52568, 52569, 52570, 52571, 52572, 52573,
                52574, 52575, 52579, 52580, 52581, 52582, 52583, 52584, 52585, 52590, 52591, 52592,
                // Legacy Woodland Labyrinth trash.
                52653, 52654, 52655, 52657, 52658, 52659, 52660,
            ])?;
            self.conn.execute_batch("PRAGMA user_version = 47")?;
        }
        if from_version < 48 {
            // v47 -> v48: Towering Perfection (Sprite Forest realm event) is now a
            // curated realm-grouped encounter -- its core (47909) and the Lower /
            // Upper Imperfection segments (47916 / 47917) fold into one
            // "Towering Perfection" card. Historically each logged as its own
            // standalone card. Regroup folds them into a shared run, latching the
            // run's `killed` from any observed core kill. (Loot-based completion
            // for escaped runs is applied live only; the combat DB has no loot
            // data to backfill it from.)
            self.regroup_realm_encounters()?;
            self.conn.execute_batch("PRAGMA user_version = 48")?;
        }
        if from_version < 49 {
            // v48 -> v49: two Combat History tracking cleanups plus a Towering
            // Perfection fold.
            // 1. Cube Deity's respawnable swarming adds (Deity Overseer 47928,
            //    Deity Defender 47929) and Ravenous Rot's last-phase tentacles
            //    (Ravenous Rot Overgrowth 16993) are no longer tracked -- only the
            //    main bosses (Cube Deity 47927, Ravenous Rot 16990) are. Purge the
            //    historical add cards.
            self.purge_fights_by_type(&[16993, 47928, 47929])?;
            // 2. Towering Perfection's four Toppled Cube segments (44894 / 44895 /
            //    47914 / 47915) are now curated members of the realm-grouped
            //    encounter. Re-run the realm regroup so their historical
            //    standalone cards fold into the existing "Towering Perfection"
            //    runs (the regroup pre-seeds existing runs to avoid duplicates).
            self.regroup_realm_encounters()?;
            self.conn.execute_batch("PRAGMA user_version = 49")?;
        }
        if from_version < 50 {
            // v49 -> v50: the Galleon Admiral (34465) is a Bilgewater's Galleon
            // add, not its own boss -- only Bilgewater's Galleon is tracked. Purge
            // the historical add cards.
            self.purge_fights_by_type(&[34465])?;
            self.conn.execute_batch("PRAGMA user_version = 50")?;
        }
        Ok(())
    }

    /// Seed `dungeon_time_totals` from the existing fight history. Grouped runs
    /// contribute their full span (`encounter_runs`); standalone dungeon fights
    /// (no curated encounter) contribute individually. Realm / hubs are excluded.
    /// Legacy runs measured time from first boss engagement rather than dungeon
    /// entry, so these totals are a lower bound -- still far better than zeros.
    fn backfill_dungeon_time_totals(&self) -> SqlResult<()> {
        // dungeon -> (total_time_ms, completions)
        let mut totals: HashMap<String, (i64, i64)> = HashMap::new();

        let mut add = |dungeon: String, started: i64, ended: i64, killed: i64| {
            if !super::tracker::is_groupable_dungeon(&dungeon) {
                return;
            }
            let elapsed = (ended - started).max(0);
            if elapsed <= 0 {
                return;
            }
            let entry = totals.entry(dungeon).or_insert((0, 0));
            entry.0 += elapsed;
            entry.1 += (killed != 0) as i64;
        };

        {
            let mut stmt = self
                .conn
                .prepare("SELECT dungeon, started_at, ended_at, killed FROM encounter_runs")?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })?;
            for row in rows {
                let (dungeon, started, ended, killed) = row?;
                add(dungeon, started, ended, killed);
            }
        }
        {
            let mut stmt = self.conn.prepare(
                "SELECT dungeon, started_at, ended_at, killed FROM fights WHERE encounter_id IS NULL",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })?;
            for row in rows {
                let (dungeon, started, ended, killed) = row?;
                add(dungeon, started, ended, killed);
            }
        }

        for (dungeon, (time_ms, completions)) in totals {
            self.conn.execute(
                r#"INSERT INTO dungeon_time_totals (dungeon, total_time_ms, completions)
                   VALUES (?1, ?2, ?3)
                   ON CONFLICT(dungeon) DO UPDATE SET
                       total_time_ms = total_time_ms + excluded.total_time_ms,
                       completions = completions + excluded.completions"#,
                params![dungeon, time_ms, completions],
            )?;
        }
        Ok(())
    }

    /// Retroactively fold historical realm-grouped encounter fights
    /// into shared encounter runs so multi-part realm bosses (Pentaract towers,
    /// Assembled Giant parts, ...) show as one card instead of several. Only
    /// touches currently-standalone fights (`encounter_run_id IS NULL`) that map
    /// to a realm-grouped encounter outside a groupable dungeon, keyed by
    /// (character, map seed, encounter) with a time-gap split.
    fn regroup_realm_encounters(&mut self) -> SqlResult<()> {
        const RUN_GAP_MS: i64 = 30 * 60 * 1000;

        struct Row {
            id: i64,
            otype: i32,
            dungeon: String,
            seed: i32,
            char_id: i32,
            started: i64,
            ended: i64,
            killed: bool,
        }
        let rows: Vec<Row> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, boss_object_type, dungeon, map_seed, local_char_id,
                        started_at, ended_at, killed
                 FROM fights WHERE encounter_run_id IS NULL ORDER BY started_at, id",
            )?;
            let it = stmt.query_map([], |r| {
                Ok(Row {
                    id: r.get(0)?,
                    otype: r.get(1)?,
                    dungeon: r.get(2)?,
                    seed: r.get(3)?,
                    char_id: r.get(4)?,
                    started: r.get(5)?,
                    ended: r.get(6)?,
                    killed: r.get::<_, i32>(7)? != 0,
                })
            })?;
            it.collect::<SqlResult<Vec<_>>>()?
        };

        struct Run {
            run_id: String,
            enc_id: &'static str,
            dungeon: String,
            ids: Vec<i64>,
            start: i64,
            end: i64,
            killed: bool,
        }
        let mut open: HashMap<(i32, i32, &'static str), Run> = HashMap::new();
        let mut runs: Vec<Run> = Vec::new();

        // Pre-seed the open map with existing realm-grouped runs so newly-curated
        // standalone members (e.g. Towering Perfection's Toppled Cubes, added to
        // the encounter after those runs were first grouped) merge into the
        // existing card instead of forming a duplicate run. Keyed identically to
        // the standalone pass, (character, map seed, encounter); the existing run
        // id and its (possibly loot-latched) killed flag are preserved and only
        // extended, never rebuilt.
        {
            struct Existing {
                run_id: String,
                enc_id: &'static str,
                dungeon: String,
                char_id: i32,
                seed: i32,
                start: i64,
                end: i64,
                killed: bool,
            }
            let mut existing: HashMap<String, Existing> = HashMap::new();
            let mut stmt = self.conn.prepare(
                "SELECT f.encounter_run_id, f.boss_object_type, f.dungeon,
                        f.local_char_id, f.map_seed, f.started_at, f.ended_at, r.killed
                 FROM fights f JOIN encounter_runs r ON r.run_id = f.encounter_run_id
                 WHERE f.encounter_run_id IS NOT NULL",
            )?;
            let it = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i32>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i32>(3)?,
                    r.get::<_, i32>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, i32>(7)? != 0,
                ))
            })?;
            for row in it {
                let (run_id, otype, dungeon, char_id, seed, started, ended, killed) = row?;
                let Some(enc) = crate::assets::encounter_for_boss_type(otype) else {
                    continue;
                };
                if super::tracker::is_groupable_dungeon(&dungeon)
                    || !crate::assets::encounter_realm_grouped(enc.id)
                {
                    continue;
                }
                let e = existing.entry(run_id.clone()).or_insert(Existing {
                    run_id,
                    enc_id: enc.id,
                    dungeon,
                    char_id: 0,
                    seed: 0,
                    start: started,
                    end: ended,
                    killed,
                });
                if e.char_id == 0 {
                    e.char_id = char_id;
                }
                if e.seed == 0 {
                    e.seed = seed;
                }
                e.start = e.start.min(started);
                e.end = e.end.max(ended);
                e.killed |= killed;
            }
            for (_, e) in existing {
                if e.char_id == 0 || e.seed == 0 {
                    continue;
                }
                let key = (e.char_id, e.seed, e.enc_id);
                // On the rare recycled-seed collision keep the latest-ending run,
                // so freshly-logged standalone members attach to the recent card.
                if open.get(&key).is_some_and(|r| r.end >= e.end) {
                    continue;
                }
                open.insert(
                    key,
                    Run {
                        run_id: e.run_id,
                        enc_id: e.enc_id,
                        dungeon: e.dungeon,
                        ids: Vec::new(),
                        start: e.start,
                        end: e.end,
                        killed: e.killed,
                    },
                );
            }
        }

        for row in rows {
            let Some(enc) = crate::assets::encounter_for_boss_type(row.otype) else {
                continue;
            };
            if super::tracker::is_groupable_dungeon(&row.dungeon)
                || !crate::assets::encounter_realm_grouped(enc.id)
                || row.char_id == 0
                || row.seed == 0
            {
                continue;
            }
            let key = (row.char_id, row.seed, enc.id);
            let start_new = match open.get(&key) {
                Some(r) => row.started - r.end > RUN_GAP_MS,
                None => true,
            };
            if start_new {
                if let Some(prev) = open.remove(&key) {
                    runs.push(prev);
                }
                open.insert(
                    key,
                    Run {
                        run_id: format!(
                            "rr-{}-{}-{:x}-{}",
                            enc.id, row.char_id, row.seed as u32, row.id
                        ),
                        enc_id: enc.id,
                        dungeon: row.dungeon.clone(),
                        ids: vec![row.id],
                        start: row.started,
                        end: row.ended,
                        killed: row.killed,
                    },
                );
            } else {
                let r = open.get_mut(&key).expect("open run exists for key");
                r.ids.push(row.id);
                r.start = r.start.min(row.started);
                r.end = r.end.max(row.ended);
                r.killed = r.killed || row.killed;
            }
        }
        for (_, r) in open.drain() {
            runs.push(r);
        }

        let tx = self.conn.transaction()?;
        for r in &runs {
            // Pre-seeded existing runs that gained no new members need no rewrite.
            if r.ids.is_empty() {
                continue;
            }
            for id in &r.ids {
                tx.execute(
                    "UPDATE fights SET encounter_id = ?1, encounter_run_id = ?2 WHERE id = ?3",
                    params![r.enc_id, r.run_id, id],
                )?;
            }
            tx.execute(
                r#"INSERT INTO encounter_runs (run_id, encounter_id, dungeon, started_at, ended_at, killed)
                   VALUES (?1, 'dungeon_run', ?2, ?3, ?4, ?5)
                   ON CONFLICT(run_id) DO UPDATE SET
                       started_at = MIN(started_at, excluded.started_at),
                       ended_at   = MAX(ended_at, excluded.ended_at),
                       killed     = MAX(killed, excluded.killed)"#,
                params![r.run_id, r.dungeon, r.start, r.end, r.killed as i32],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Fold leftover standalone fights recorded inside a groupable dungeon into
    /// that dungeon instance's card. At write time every
    /// fight in a groupable dungeon shares the instance run id, but older builds
    /// left some rows -- notably treasure crates (Sunken Treasure, Infested
    /// Chest, ...) -- with a NULL run id, so they showed as separate cards.
    ///
    /// Each leftover joins an existing sibling run for the same
    /// (character, dungeon, map seed) instance when one exists within a time-gap
    /// window; otherwise leftovers of one instance form their own run. Only
    /// touches `encounter_run_id IS NULL` rows so it is idempotent.
    fn regroup_dungeon_instances(&mut self) -> SqlResult<()> {
        const RUN_GAP_MS: i64 = 30 * 60 * 1000;

        // Existing dungeon-instance runs, so a leftover can join the boss card of
        // its own instance instead of forming a duplicate one.
        struct Existing {
            run_id: String,
            start: i64,
            end: i64,
        }
        let mut existing: HashMap<(i32, String, i32), Vec<Existing>> = HashMap::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT encounter_run_id, local_char_id, dungeon, map_seed,
                        MIN(started_at), MAX(ended_at)
                 FROM fights WHERE encounter_run_id IS NOT NULL
                 GROUP BY encounter_run_id",
            )?;
            let it = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i32>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i32>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            })?;
            for row in it {
                let (run_id, char_id, dungeon, seed, start, end) = row?;
                existing
                    .entry((char_id, dungeon, seed))
                    .or_default()
                    .push(Existing { run_id, start, end });
            }
        }

        struct Row {
            id: i64,
            dungeon: String,
            seed: i32,
            char_id: i32,
            started: i64,
            ended: i64,
            killed: bool,
        }
        let rows: Vec<Row> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, dungeon, map_seed, local_char_id, started_at, ended_at, killed
                 FROM fights WHERE encounter_run_id IS NULL ORDER BY started_at, id",
            )?;
            let it = stmt.query_map([], |r| {
                Ok(Row {
                    id: r.get(0)?,
                    dungeon: r.get(1)?,
                    seed: r.get(2)?,
                    char_id: r.get(3)?,
                    started: r.get(4)?,
                    ended: r.get(5)?,
                    killed: r.get::<_, i32>(6)? != 0,
                })
            })?;
            it.collect::<SqlResult<Vec<_>>>()?
        };

        struct Run {
            run_id: String,
            dungeon: String,
            ids: Vec<i64>,
            start: i64,
            end: i64,
            killed: bool,
        }
        // (fight id, run id, dungeon, started, ended, killed) for leftovers that
        // adopt an existing instance run.
        let mut adopt: Vec<(i64, String, String, i64, i64, bool)> = Vec::new();
        let mut open: HashMap<(i32, String, i32), Run> = HashMap::new();
        let mut new_runs: Vec<Run> = Vec::new();

        for row in rows {
            if !super::tracker::is_groupable_dungeon(&row.dungeon)
                || row.char_id == 0
                || row.seed == 0
            {
                continue;
            }
            let key = (row.char_id, row.dungeon.clone(), row.seed);
            if let Some(list) = existing.get(&key) {
                if let Some(e) = list.iter().find(|e| {
                    row.started <= e.end + RUN_GAP_MS && row.ended >= e.start - RUN_GAP_MS
                }) {
                    adopt.push((
                        row.id,
                        e.run_id.clone(),
                        row.dungeon.clone(),
                        row.started,
                        row.ended,
                        row.killed,
                    ));
                    continue;
                }
            }
            let start_new = match open.get(&key) {
                Some(r) => row.started - r.end > RUN_GAP_MS,
                None => true,
            };
            if start_new {
                if let Some(prev) = open.remove(&key) {
                    new_runs.push(prev);
                }
                open.insert(
                    key,
                    Run {
                        run_id: format!("di-{:x}-{}", row.seed as u32, row.id),
                        dungeon: row.dungeon.clone(),
                        ids: vec![row.id],
                        start: row.started,
                        end: row.ended,
                        killed: row.killed,
                    },
                );
            } else {
                let r = open.get_mut(&key).expect("open run exists for key");
                r.ids.push(row.id);
                r.start = r.start.min(row.started);
                r.end = r.end.max(row.ended);
                r.killed |= row.killed;
            }
        }
        for (_, r) in open.drain() {
            new_runs.push(r);
        }

        if adopt.is_empty() && new_runs.is_empty() {
            return Ok(());
        }

        let tx = self.conn.transaction()?;
        for (id, run_id, dungeon, start, end, killed) in &adopt {
            tx.execute(
                "UPDATE fights SET encounter_id = 'dungeon_run', encounter_run_id = ?1 WHERE id = ?2",
                params![run_id, id],
            )?;
            tx.execute(
                r#"INSERT INTO encounter_runs (run_id, encounter_id, dungeon, started_at, ended_at, killed)
                   VALUES (?1, 'dungeon_run', ?2, ?3, ?4, ?5)
                   ON CONFLICT(run_id) DO UPDATE SET
                       started_at = MIN(started_at, excluded.started_at),
                       ended_at   = MAX(ended_at, excluded.ended_at),
                       killed     = MAX(killed, excluded.killed)"#,
                params![run_id, dungeon, start, end, *killed as i32],
            )?;
        }
        for r in &new_runs {
            for id in &r.ids {
                tx.execute(
                    "UPDATE fights SET encounter_id = 'dungeon_run', encounter_run_id = ?1 WHERE id = ?2",
                    params![r.run_id, id],
                )?;
            }
            tx.execute(
                r#"INSERT INTO encounter_runs (run_id, encounter_id, dungeon, started_at, ended_at, killed)
                   VALUES (?1, 'dungeon_run', ?2, ?3, ?4, ?5)
                   ON CONFLICT(run_id) DO UPDATE SET
                       started_at = MIN(started_at, excluded.started_at),
                       ended_at   = MAX(ended_at, excluded.ended_at),
                       killed     = MAX(killed, excluded.killed)"#,
                params![r.run_id, r.dungeon, r.start, r.end, r.killed as i32],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Whether `table` has a column named `column` (used to keep additive
    /// migrations idempotent even if a column already exists).
    fn column_exists(&self, table: &str, column: &str) -> SqlResult<bool> {
        let mut stmt = self
            .conn
            .prepare(&format!("PRAGMA table_info({})", table))?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let name: String = row.get(1)?;
            if name == column {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Populate the `boss_group` column for rows where it is NULL, classifying
    /// each from its persisted dungeon name, boss object type, and boss name.
    /// Idempotent: only untouched (NULL) rows are updated.
    fn backfill_boss_groups(&mut self) -> SqlResult<()> {
        let rows: Vec<(i64, String, i32, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, dungeon, boss_object_type, boss_name
                 FROM fights WHERE boss_group IS NULL",
            )?;
            let it = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
            it.collect::<SqlResult<Vec<_>>>()?
        };
        if rows.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        for (id, dungeon, otype, name) in rows {
            let group = crate::assets::boss_group(&dungeon, otype, &name).map(|g| g.as_str());
            tx.execute(
                "UPDATE fights SET boss_group = ?1 WHERE id = ?2",
                params![group, id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Recompute the `boss_group` for every row from its persisted dungeon/boss
    /// and update rows whose classification changed. Used when the group
    /// taxonomy changes (v7 -> v8: Biome Minibosses split into Heroes of Oryx
    /// tiers). Unlike [`backfill_boss_groups`] this touches non-NULL rows too.
    fn reclassify_boss_groups(&mut self) -> SqlResult<()> {
        let rows: Vec<(i64, String, i32, String, Option<String>)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, dungeon, boss_object_type, boss_name, boss_group FROM fights",
            )?;
            let it = stmt.query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?;
            it.collect::<SqlResult<Vec<_>>>()?
        };
        if rows.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        for (id, dungeon, otype, name, current) in rows {
            let group =
                crate::assets::boss_group(&dungeon, otype, &name).map(|g| g.as_str().to_string());
            if group != current {
                tx.execute(
                    "UPDATE fights SET boss_group = ?1 WHERE id = ?2",
                    params![group, id],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Delete every stored fight (and its participants) whose boss object type is
    /// in `types`, then drop any encounter run left with no phases. Used to purge
    /// objects that were tracked as fights before being curated out of the boss
    /// set (e.g. Challenge Gate, MV Easy Dropper).
    fn purge_fights_by_type(&mut self, types: &[i32]) -> SqlResult<()> {
        if types.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        for &otype in types {
            tx.execute(
                "DELETE FROM fight_participants WHERE fight_id IN
                 (SELECT id FROM fights WHERE boss_object_type = ?1)",
                params![otype],
            )?;
            tx.execute(
                "DELETE FROM fights WHERE boss_object_type = ?1",
                params![otype],
            )?;
        }
        // Recompute the span of every run that still has phases; a purged phase
        // may have defined the run's earliest start or latest end.
        tx.execute(
            "UPDATE encounter_runs SET
               started_at = (SELECT MIN(started_at) FROM fights
                             WHERE encounter_run_id = encounter_runs.run_id),
               ended_at = (SELECT MAX(ended_at) FROM fights
                           WHERE encounter_run_id = encounter_runs.run_id)
             WHERE run_id IN (SELECT DISTINCT encounter_run_id FROM fights
                              WHERE encounter_run_id IS NOT NULL)",
            [],
        )?;
        tx.execute(
            "DELETE FROM encounter_runs WHERE run_id NOT IN
             (SELECT DISTINCT encounter_run_id FROM fights
              WHERE encounter_run_id IS NOT NULL)",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Retroactively complete Legacy Lair of Draconis dragon fights whose loot the
    /// paired loot DB attributed to the dragon's self-destruct chest. A legacy
    /// dragon fight is marked `killed = 1` when a chest bag exists in the same map
    /// instance within the fight's loot-link window, since the chest only spawns
    /// once the dragon is defeated.
    ///
    /// Best-effort and idempotent: it only flips still-escaped dragon fights, so it
    /// can run on every open. When the paired loot DB is absent or unreadable it is
    /// a no-op, letting a later paired open repair the history instead.
    fn complete_legacy_lod_dragons_from_loot(&mut self, loot_path: Option<&Path>) -> SqlResult<()> {
        let Some(path) = loot_path else {
            return Ok(());
        };
        if !path.exists() {
            return Ok(());
        }
        let loot = match Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("[COMBAT] Skipping Legacy LoD completion; loot DB unreadable: {e}");
                return Ok(());
            }
        };
        let _ = loot.busy_timeout(std::time::Duration::from_secs(5));

        let chests: Vec<i32> = crate::assets::lod_dragon_chest_pairs()
            .iter()
            .map(|(_, chest)| *chest)
            .collect();
        let placeholders: Vec<String> = (0..chests.len()).map(|i| format!("?{}", i + 1)).collect();
        let sql = format!(
            "SELECT map_seed, mob_type, timestamp FROM loot_drops
             WHERE map_seed != 0 AND mob_type IN ({})",
            placeholders.join(", ")
        );
        let mut stmt = match loot.prepare(&sql) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("[COMBAT] Skipping Legacy LoD completion; loot query failed: {e}");
                return Ok(());
            }
        };
        let drops: Vec<(i32, i32, i64)> =
            match stmt.query_map(rusqlite::params_from_iter(chests.iter()), |r| {
                Ok((
                    r.get::<_, i32>(0)?,
                    r.get::<_, i32>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            }) {
                Ok(rows) => rows.filter_map(Result::ok).collect(),
                Err(e) => {
                    tracing::warn!(
                        "[COMBAT] Skipping Legacy LoD completion; loot read failed: {e}"
                    );
                    return Ok(());
                }
            };
        if drops.is_empty() {
            return Ok(());
        }

        let tx = self.conn.transaction()?;
        for (map_seed, chest_type, ts) in drops {
            let Some(dragon) = crate::assets::boss_for_loot_emitter(chest_type) else {
                continue;
            };
            // The fight window widened by the loot-link tolerances: the chest bag
            // registers at or just after the dragon's self-destruct. Complete only
            // the single closest fight so a reconnect fragment in the same instance
            // cannot turn one chest drop into several kills.
            tx.execute(
                "UPDATE fights SET killed = 1 WHERE id = (
                     SELECT id FROM fights
                     WHERE killed = 0 AND map_seed = ?1 AND boss_object_type = ?2
                       AND started_at <= ?3 AND ended_at >= ?4
                     ORDER BY ABS(ended_at - ?5) LIMIT 1
                 )",
                params![
                    map_seed,
                    dragon,
                    ts + LOOT_LINK_PRE_MS,
                    ts - LOOT_LINK_POST_MS,
                    ts
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Complete every Legacy Lair of Draconis run that a recorded Ivory Wyvern
    /// fight proves was fully cleared. Idempotent and evidence-only, so it runs on
    /// every open to repair legacy history and pick up runs finalized after their
    /// Ivory fight was inserted.
    fn backfill_legacy_lod_from_ivory(&mut self) -> SqlResult<()> {
        let ivories: Vec<(i64, i32)> = {
            let mut stmt = self.conn.prepare(
                "SELECT started_at, local_char_id FROM fights
                 WHERE boss_object_type = ?1 ORDER BY started_at",
            )?;
            let rows = stmt
                .query_map(params![crate::assets::LEGACY_LOD_IVORY_BOSS], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?
                .collect::<SqlResult<Vec<_>>>()?;
            rows
        };
        for (started_at, char_id) in ivories {
            self.complete_lod_dragons_before_ivory(started_at, char_id)?;
        }
        Ok(())
    }

    /// Mark the Legacy Lair of Draconis run that immediately preceded an Ivory
    /// Wyvern fight fully complete: the Ivory portal only drops once all four
    /// dragons are defeated, so every recorded dragon in that run was killed even
    /// if the local player only observed (and never scored) some of them. Only
    /// still-escaped dragon fights are flipped, so it is safe to call repeatedly.
    fn complete_lod_dragons_before_ivory(
        &mut self,
        ivory_started_at: i64,
        char_id: i32,
    ) -> SqlResult<()> {
        // Pre-v2 rows have no character identity; without it "same character"
        // cannot be established, so inference would leak across players.
        if char_id == 0 {
            return Ok(());
        }
        // A generous cap on the gap between clearing the Lair and entering the
        // Ivory chamber; still tight enough that an Ivory fight is matched to the
        // run it actually followed rather than an earlier session's Lair.
        const FOLLOWUP_WINDOW_MS: i64 = 20 * 60 * 1000;
        // Beyond this gap two same-seed dragon fights are separate instances,
        // matching the generic run-grouping split.
        const RUN_GAP_MS: i64 = 30 * 60 * 1000;
        // Dragon object types are trusted integer constants, so they can be
        // inlined into the IN clause.
        let dragon_list = crate::assets::lod_dragon_chest_pairs()
            .iter()
            .map(|(d, _)| d.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let earliest = ivory_started_at - FOLLOWUP_WINDOW_MS;

        // Identify the Lair run by its latest dragon fight ending at or just
        // before the Ivory fight for the same character.
        let find_sql = format!(
            "SELECT id, encounter_run_id, map_seed, ended_at FROM fights
             WHERE local_char_id = ?1 AND ended_at <= ?2 AND ended_at >= ?3
               AND boss_object_type IN ({dragon_list})
             ORDER BY ended_at DESC LIMIT 1"
        );
        let found: Option<(i64, Option<String>, i32, i64)> = self
            .conn
            .query_row(
                &find_sql,
                params![char_id, ivory_started_at, earliest],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((fight_id, run_id, map_seed, anchor_ended)) = found else {
            return Ok(());
        };

        // Flip every recorded dragon in that run. Prefer the run grouping; fall
        // back to the same map instance (bounded to the anchor's time cluster so a
        // recycled seed cannot flip an unrelated instance), then to the single
        // anchoring fight.
        if let Some(run_id) = run_id {
            self.conn.execute(
                &format!(
                    "UPDATE fights SET killed = 1
                     WHERE killed = 0 AND encounter_run_id = ?1
                       AND boss_object_type IN ({dragon_list})"
                ),
                params![run_id],
            )?;
        } else if map_seed != 0 {
            self.conn.execute(
                &format!(
                    "UPDATE fights SET killed = 1
                     WHERE killed = 0 AND local_char_id = ?1 AND map_seed = ?2
                       AND ended_at BETWEEN ?3 AND ?4
                       AND boss_object_type IN ({dragon_list})"
                ),
                params![
                    char_id,
                    map_seed,
                    anchor_ended - RUN_GAP_MS,
                    anchor_ended + RUN_GAP_MS
                ],
            )?;
        } else {
            self.conn.execute(
                "UPDATE fights SET killed = 1 WHERE killed = 0 AND id = ?1",
                params![fight_id],
            )?;
        }
        Ok(())
    }

    /// Retroactively group fights recorded before generic (non-curated) dungeon
    /// grouping existed. Only fights with trustworthy identity (a non-zero local
    /// character id AND a non-zero map seed) are regrouped, keyed by
    /// (char, dungeon, seed) with a time-gap split so a recycled seed across
    /// sessions cannot merge two unrelated clears. Fights lacking that evidence
    /// keep whatever grouping they already had (curated or standalone) rather
    /// than risk a bad merge. `encounter_runs` is rebuilt from the final fight
    /// assignments so no orphan rows survive.
    fn migrate_generic_grouping(&mut self) -> SqlResult<()> {
        const RUN_GAP_MS: i64 = 30 * 60 * 1000;

        struct Row {
            id: i64,
            dungeon: String,
            seed: i32,
            char_id: i32,
            started: i64,
            ended: i64,
            enc_id: Option<String>,
            run_id: Option<String>,
        }
        let rows: Vec<Row> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, dungeon, map_seed, local_char_id, started_at, ended_at,
                        encounter_id, encounter_run_id
                 FROM fights ORDER BY started_at, id",
            )?;
            let it = stmt.query_map([], |r| {
                Ok(Row {
                    id: r.get(0)?,
                    dungeon: r.get(1)?,
                    seed: r.get(2)?,
                    char_id: r.get(3)?,
                    started: r.get(4)?,
                    ended: r.get(5)?,
                    enc_id: r.get(6)?,
                    run_id: r.get(7)?,
                })
            })?;
            it.collect::<SqlResult<Vec<_>>>()?
        };

        // Group trustworthy fights by dungeon instance, splitting on time gaps.
        // A run id from a live session ("{nonce}-{counter}") is authoritative: it
        // marks a real map instance, so two DISTINCT live ids in one tentative
        // group are two separate clears (recycled seed) and must never merge.
        fn is_live_run_id(id: &str) -> bool {
            !id.starts_with("bf-") && !id.starts_with("dg-")
        }
        struct Group {
            synth: String,
            existing: std::collections::BTreeSet<String>,
            members: Vec<(i64, String)>,
            end: i64,
        }
        let mut open: HashMap<(i32, String, i32), Group> = HashMap::new();
        // fight id -> (new run id, new encounter id) for evidence fights.
        let mut assign: HashMap<i64, (String, String)> = HashMap::new();
        let flush = |g: Group, assign: &mut HashMap<i64, (String, String)>| {
            let live: Vec<&String> = g.existing.iter().filter(|s| is_live_run_id(s)).collect();
            // Two distinct live instances collided on (char, seed, time): leave
            // every fight exactly as-is rather than risk merging separate clears.
            if live.len() >= 2 {
                return;
            }
            let run_id = if let Some(live_id) = live.first() {
                (*live_id).clone() // fold others into the authoritative live run
            } else if let Some(min_existing) = g.existing.iter().next() {
                min_existing.clone() // smallest legacy (bf-/dg-) id for stability
            } else {
                g.synth // all members were standalone: deterministic synthetic id
            };
            for (id, enc) in g.members {
                assign.insert(id, (run_id.clone(), enc));
            }
        };
        for row in &rows {
            let groupable = super::tracker::is_groupable_dungeon(&row.dungeon);
            let has_evidence = row.char_id != 0 && row.seed != 0;
            if !(groupable && has_evidence) {
                continue;
            }
            // Keep a curated encounter id (so aux repr types still resolve);
            // otherwise use the "dungeon_run" sentinel.
            let enc = row
                .enc_id
                .clone()
                .filter(|e| e != "dungeon_run")
                .unwrap_or_else(|| "dungeon_run".to_string());
            let key = (row.char_id, row.dungeon.clone(), row.seed);
            let start_new = match open.get(&key) {
                Some(g) => row.started - g.end > RUN_GAP_MS,
                None => true,
            };
            if start_new {
                if let Some(prev) = open.remove(&key) {
                    flush(prev, &mut assign);
                }
                // Synthetic fallback keyed by the first fight id -> globally unique
                // across groups (each fight belongs to exactly one group).
                let synth = format!("dg-{}-{:x}-{}", row.char_id, row.seed as u32, row.id);
                let mut existing = std::collections::BTreeSet::new();
                if let Some(r) = &row.run_id {
                    existing.insert(r.clone());
                }
                open.insert(
                    key,
                    Group {
                        synth,
                        existing,
                        members: vec![(row.id, enc)],
                        end: row.ended,
                    },
                );
            } else {
                let g = open.get_mut(&key).expect("open group exists for key");
                if let Some(r) = &row.run_id {
                    g.existing.insert(r.clone());
                }
                g.members.push((row.id, enc));
                g.end = g.end.max(row.ended);
            }
        }
        for (_, g) in open.drain() {
            flush(g, &mut assign);
        }

        let tx = self.conn.transaction()?;
        for (id, (run_id, enc_id)) in &assign {
            tx.execute(
                "UPDATE fights SET encounter_run_id = ?1, encounter_id = ?2 WHERE id = ?3",
                params![run_id, enc_id, id],
            )?;
        }
        // Rebuild encounter_runs from the final fight assignments (orphan-free).
        tx.execute("DELETE FROM encounter_runs", [])?;
        tx.execute(
            r#"INSERT INTO encounter_runs (run_id, encounter_id, dungeon, started_at, ended_at, killed)
               SELECT encounter_run_id, 'dungeon_run',
                      MIN(dungeon), MIN(started_at), MAX(ended_at), MAX(killed)
               FROM fights
               WHERE encounter_run_id IS NOT NULL
               GROUP BY encounter_run_id"#,
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn create_tables(&self) -> SqlResult<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS fights (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                started_at INTEGER NOT NULL,
                ended_at INTEGER NOT NULL,
                dungeon TEXT NOT NULL,
                dungeon_entered_at INTEGER,
                map_seed INTEGER NOT NULL,
                boss_object_type INTEGER NOT NULL,
                boss_name TEXT NOT NULL,
                boss_max_hp INTEGER NOT NULL,
                boss_start_hp INTEGER NOT NULL,
                local_object_id INTEGER NOT NULL,
                local_char_id INTEGER NOT NULL DEFAULT 0,
                killed INTEGER NOT NULL,
                encounter_id TEXT,
                encounter_run_id TEXT,
                boss_group TEXT,
                local_close_calls INTEGER NOT NULL DEFAULT 0,
                aux_member_count INTEGER,
                reached_zero INTEGER NOT NULL DEFAULT 0,
                joined_late INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS stat_awards (
                card_key TEXT PRIMARY KEY,
                char_id INTEGER NOT NULL,
                lone INTEGER NOT NULL DEFAULT 0,
                last INTEGER NOT NULL DEFAULT 0,
                most INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS encounter_runs (
                run_id TEXT PRIMARY KEY,
                encounter_id TEXT NOT NULL,
                dungeon TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                ended_at INTEGER NOT NULL,
                killed INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS fight_participants (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                fight_id INTEGER NOT NULL,
                object_id INTEGER NOT NULL,
                object_type INTEGER NOT NULL,
                name TEXT NOT NULL,
                equip0 INTEGER NOT NULL,
                equip1 INTEGER NOT NULL,
                equip2 INTEGER NOT NULL,
                equip3 INTEGER NOT NULL,
                enchants TEXT,
                damage INTEGER NOT NULL,
                hits INTEGER NOT NULL,
                is_local INTEGER NOT NULL,
                provenance TEXT NOT NULL,
                skin_id INTEGER NOT NULL DEFAULT 0,
                tex1 INTEGER NOT NULL DEFAULT 0,
                tex2 INTEGER NOT NULL DEFAULT 0,
                end_status TEXT NOT NULL DEFAULT 'present',
                grave_type INTEGER NOT NULL DEFAULT 0,
                pet_type INTEGER NOT NULL DEFAULT 0,
                pet_abilities TEXT,
                damage_taken INTEGER,
                damage_taken_provenance TEXT,
                guarded_damage INTEGER,
                guarded_hits INTEGER,
                damage_blocked INTEGER,
                FOREIGN KEY (fight_id) REFERENCES fights(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS dungeon_time_totals (
                dungeon TEXT PRIMARY KEY,
                total_time_ms INTEGER NOT NULL DEFAULT 0,
                completions INTEGER NOT NULL DEFAULT 0
            );

            CREATE INDEX IF NOT EXISTS idx_fights_started ON fights(started_at);
            CREATE INDEX IF NOT EXISTS idx_fights_run ON fights(encounter_run_id);
            CREATE INDEX IF NOT EXISTS idx_fights_group ON fights(boss_group);
            CREATE INDEX IF NOT EXISTS idx_participants_fight ON fight_participants(fight_id);
            CREATE INDEX IF NOT EXISTS idx_encounter_runs_recent
                ON encounter_runs(ended_at DESC, started_at DESC);
            "#,
        )?;
        Ok(())
    }

    /// Add one dungeon run's elapsed time (and, when the boss was killed, one
    /// completion) to the running per-dungeon counter. `dungeon` is the
    /// normalized map name. Independent of the Combat History tracked-boss
    /// toggles: every dungeon run entered contributes its time.
    pub fn add_dungeon_time(
        &mut self,
        dungeon: &str,
        elapsed_ms: i64,
        completed: bool,
    ) -> SqlResult<()> {
        self.conn.execute(
            r#"INSERT INTO dungeon_time_totals (dungeon, total_time_ms, completions)
               VALUES (?1, ?2, ?3)
               ON CONFLICT(dungeon) DO UPDATE SET
                   total_time_ms = total_time_ms + excluded.total_time_ms,
                   completions = completions + excluded.completions"#,
            params![dungeon, elapsed_ms, completed as i64],
        )?;
        Ok(())
    }

    /// A cheap change signature for the per-dungeon time counters: the sum of
    /// all accumulated time. Lets the UI detect newly flushed run time without
    /// re-reading every row each frame.
    pub fn dungeon_time_signature(&self) -> SqlResult<i64> {
        self.conn.query_row(
            "SELECT COALESCE(SUM(total_time_ms), 0) FROM dungeon_time_totals",
            [],
            |row| row.get(0),
        )
    }

    /// Read the running per-dungeon time counters.
    pub fn dungeon_time_totals(&self) -> SqlResult<Vec<DungeonTimeTotal>> {
        let mut stmt = self
            .conn
            .prepare("SELECT dungeon, total_time_ms, completions FROM dungeon_time_totals")?;
        let rows = stmt.query_map([], |row| {
            Ok(DungeonTimeTotal {
                dungeon: row.get(0)?,
                total_time_ms: row.get(1)?,
                completions: row.get(2)?,
            })
        })?;
        rows.collect()
    }

    /// Insert a completed fight and its participants. Returns the new fight id.
    pub fn insert_fight(&mut self, fight: &CompletedFight) -> SqlResult<i64> {
        let boss_group =
            crate::assets::boss_group(&fight.dungeon, fight.boss_object_type, &fight.boss_name)
                .map(|g| g.as_str());
        let tx = self.conn.transaction()?;
        tx.execute(
            r#"INSERT INTO fights
               (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
                boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
                encounter_id, encounter_run_id, boss_group, local_close_calls, aux_member_count,
                dungeon_entered_at, reached_zero, joined_late)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)"#,
            params![
                fight.started_at,
                fight.ended_at,
                fight.dungeon,
                fight.map_seed,
                fight.boss_object_type,
                fight.boss_name,
                fight.boss_max_hp,
                fight.boss_start_hp,
                fight.local_object_id,
                fight.local_char_id,
                fight.killed as i32,
                fight.encounter_id,
                fight.encounter_run_id,
                boss_group,
                fight.local_close_calls,
                fight.aux_member_count,
                fight.dungeon_entered_at,
                fight.reached_zero as i32,
                fight.joined_late as i32,
            ],
        )?;
        let fight_id = tx.last_insert_rowid();

        // Maintain the parent encounter-run row (one per grouped run). Its span
        // widens to cover every member phase and `killed` latches once any phase
        // is killed. The parent id is always the "dungeon_run" sentinel so the
        // grouped card is headlined by its dungeon regardless of which phase was
        // inserted first (curated ids live only on the individual fight rows).
        if let (Some(_enc_id), Some(run_id)) = (&fight.encounter_id, &fight.encounter_run_id) {
            tx.execute(
                r#"INSERT INTO encounter_runs (run_id, encounter_id, dungeon, started_at, ended_at, killed)
                   VALUES (?1, 'dungeon_run', ?2, ?3, ?4, ?5)
                   ON CONFLICT(run_id) DO UPDATE SET
                       started_at = MIN(started_at, excluded.started_at),
                       ended_at   = MAX(ended_at, excluded.ended_at),
                       killed     = MAX(killed, excluded.killed)"#,
                params![
                    run_id,
                    fight.dungeon,
                    fight.started_at,
                    fight.ended_at,
                    fight.killed as i32,
                ],
            )?;
        }

        for p in &fight.participants {
            tx.execute(
                r#"INSERT INTO fight_participants
                   (fight_id, object_id, object_type, name, equip0, equip1, equip2, equip3,
                    enchants, damage, hits, is_local, provenance, skin_id, tex1, tex2,
                    end_status, grave_type, pet_type, pet_abilities, damage_taken,
                    damage_taken_provenance, guarded_damage, guarded_hits, damage_blocked)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)"#,
                params![
                    fight_id,
                    p.object_id,
                    p.object_type,
                    p.name,
                    p.equipment[0],
                    p.equipment[1],
                    p.equipment[2],
                    p.equipment[3],
                    serialize_enchants(&p.equipment_enchants),
                    p.damage,
                    p.hits as i64,
                    p.is_local as i32,
                    p.provenance.as_str(),
                    p.skin_id,
                    p.tex1 as i32,
                    p.tex2 as i32,
                    p.end_status.as_str(),
                    p.end_status.grave_type(),
                    p.pet.as_ref().map(|pet| pet.pet_type).unwrap_or(0),
                    p.pet.as_ref().map(|pet| serialize_pet_abilities(&pet.abilities)),
                    p.damage_taken,
                    p.damage_taken.map(|_| p.damage_taken_provenance.as_str()),
                    p.guarded_damage,
                    p.guarded_hits.map(|h| h as i64),
                    p.damage_blocked,
                ],
            )?;
        }

        tx.commit()?;
        // The Ivory Wyvern portal only drops once the Lair's four dragons are
        // defeated, so completing this fight retroactively confirms the preceding
        // Lair run was a full clear. These fights are non-Exaltation; if that
        // classification changes, return their selections for award reconciliation.
        if crate::assets::is_legacy_lod_ivory_boss(fight.boss_object_type) {
            self.complete_lod_dragons_before_ivory(fight.started_at, fight.local_char_id)?;
        }
        Ok(fight_id)
    }

    /// Read the most recent fights (headers + participants), newest first.
    pub fn recent_fights(&self, limit: i64) -> SqlResult<Vec<FightRecord>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT id, started_at, ended_at, dungeon, map_seed, boss_object_type,
                      boss_name, boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
                      local_close_calls, aux_member_count, dungeon_entered_at
               FROM fights ORDER BY started_at DESC LIMIT ?1"#,
        )?;
        let rows = stmt.query_map(params![limit], |row| Self::map_fight_header(row))?;
        let mut fights: Vec<FightRecord> = rows.collect::<SqlResult<_>>()?;
        for fight in &mut fights {
            fight.participants = self.participants_for(fight.id)?;
        }
        Ok(fights)
    }

    fn map_fight_header(row: &rusqlite::Row) -> SqlResult<FightRecord> {
        Ok(FightRecord {
            id: row.get(0)?,
            started_at: row.get(1)?,
            ended_at: row.get(2)?,
            dungeon: row.get(3)?,
            map_seed: row.get(4)?,
            boss_object_type: row.get(5)?,
            boss_name: row.get(6)?,
            boss_max_hp: row.get(7)?,
            boss_start_hp: row.get(8)?,
            local_object_id: row.get(9)?,
            local_char_id: row.get(10)?,
            killed: row.get::<_, i32>(11)? != 0,
            local_close_calls: row.get(12)?,
            aux_member_count: row.get(13)?,
            dungeon_entered_at: row.get(14)?,
            participants: Vec::new(),
        })
    }

    /// Query lightweight fight summaries for the list view, newest first.
    ///
    /// Group-aware: standalone fights appear as themselves, while every phase of
    /// a curated multi-phase encounter is folded into ONE summary keyed by its
    /// run id. Grouping, filtering and the final limit all operate on groups, so
    /// a filter that matches one phase returns the whole encounter and the limit
    /// never slices an encounter in half.
    pub fn list_fights(&self, query: &FightQuery, limit: i64) -> SqlResult<Vec<FightSummary>> {
        // With a secret-stat filter active the qualifying cards may sit past the
        // page limit, so fetch the whole (already group/date-narrowed) candidate
        // set, tag flags, filter, then trim. Without one, behave as before.
        let stat_filter = query.filter_lone_fighter
            || query.filter_last_hero
            || query.filter_most_damage_taken
            || query.filter_close_calls;
        let fetch_limit = if stat_filter { i64::MAX } else { limit };
        let mut out = self.standalone_summaries(query, fetch_limit, None)?;
        out.extend(self.encounter_summaries(query, fetch_limit, None)?);
        out.sort_by(|a, b| {
            b.ended_at
                .cmp(&a.ended_at)
                .then(b.started_at.cmp(&a.started_at))
        });
        for s in &mut out {
            s.flawless = self.card_is_flawless(s)?;
            let (lone, last, most_dmg) = self.card_secret_stats(s)?;
            s.lone_fighter = lone;
            s.last_hero_standing = last;
            s.most_damage_taken = most_dmg;
        }
        if stat_filter {
            out.retain(|s| {
                (query.filter_lone_fighter && s.lone_fighter)
                    || (query.filter_last_hero && s.last_hero_standing)
                    || (query.filter_most_damage_taken && s.most_damage_taken)
                    || (query.filter_close_calls && s.local_close_calls > 0)
            });
        }
        out.truncate(limit.max(0) as usize);
        Ok(out)
    }

    /// Whether an Exaltation card earns the "Lone fighter" and/or "Last hero
    /// standing" markers. Both gate on the boss being killed in an Exaltation
    /// (7+ grave-difficulty) dungeon and read only already-stored participant
    /// data, so they apply to legacy cards.
    ///
    /// - **Lone fighter**: the local player finished the card (present at the
    ///   main-boss anchor) and dealt damage, and no other named participant dealt
    ///   any attributed damage across the main boss or any additional tracked
    ///   entity in the card. Unattributed damage is ignored (it is not a
    ///   participant).
    /// - **Last hero standing**: the local player finished at the main-boss
    ///   anchor and every other participant died or nexused, with at least one
    ///   such other participant (a real group existed). Only positive death/nexus
    ///   evidence at the anchor qualifies, so all-`present` legacy rows never flag.
    /// - **Most damage taken**: on the main-boss anchor phase(s), the local
    ///   player took strictly more damage than every other detected teammate,
    ///   with at least one such teammate. Only anchor-phase damage-taken counts:
    ///   the local figure is self-computed and usually exists only for the anchor
    ///   phase, whereas remote players are observed across every auxiliary phase,
    ///   so a card-wide sum would unfairly favour teammates.
    fn card_secret_stats(&self, s: &FightSummary) -> SqlResult<(bool, bool, bool)> {
        if !s.killed {
            return Ok((false, false, false));
        }
        if crate::assets::boss_group(&s.dungeon, s.boss_object_type, &s.boss_name)
            != Some(crate::assets::BossGroup::Exaltation)
        {
            return Ok((false, false, false));
        }
        // Every fight in the card, flagged as a main-boss (anchor) phase.
        let fights: Vec<(i64, bool)> = match &s.encounter_run_id {
            Some(run) => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, boss_object_type FROM fights WHERE encounter_run_id = ?1",
                )?;
                let rows = stmt.query_map(params![run], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i32>(1)? == s.boss_object_type,
                    ))
                })?;
                rows.collect::<SqlResult<Vec<_>>>()?
            }
            None => vec![(s.id, true)],
        };
        if !fights.iter().any(|(_, is_boss)| *is_boss) {
            return Ok((false, false, false));
        }

        let combine = |acc: Option<ParticipantEndStatus>, s: ParticipantEndStatus| {
            Some(match acc {
                Some(prev) => combine_end_status(prev, s),
                None => s,
            })
        };

        let mut local_damage: i64 = 0;
        let mut local_damage_taken: i64 = 0;
        let mut local_seen = false;
        let mut local_anchor: Option<ParticipantEndStatus> = None;
        // object_id -> (total damage, combined anchor status, anchor-phase damage taken)
        let mut others: std::collections::HashMap<i32, (i64, Option<ParticipantEndStatus>, i64)> =
            std::collections::HashMap::new();

        for (fid, is_boss) in &fights {
            let mut stmt = self.conn.prepare(
                "SELECT object_id, is_local, damage, end_status, grave_type, provenance, damage_taken
                 FROM fight_participants WHERE fight_id = ?1",
            )?;
            let rows = stmt.query_map(params![fid], |r| {
                Ok((
                    r.get::<_, i32>(0)?,
                    r.get::<_, i32>(1)? != 0,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i32>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, Option<i64>>(6)?.unwrap_or(0),
                ))
            })?;
            for row in rows {
                let (oid, is_local, damage, status_str, grave, prov, damage_taken) = row?;
                let status = ParticipantEndStatus::from_str(&status_str, grave);
                if is_local {
                    local_seen = true;
                    local_damage += damage;
                    // Damage-taken is compared on the main-boss anchor phase(s)
                    // only. The local figure is self-computed and typically exists
                    // solely for the anchor phase, while remote players are
                    // observed across every auxiliary phase, so summing card-wide
                    // would unfairly inflate teammates' totals.
                    if *is_boss {
                        local_damage_taken += damage_taken;
                        local_anchor = combine(local_anchor, status);
                    }
                } else {
                    // Unattributed ("Unknown"/unresolved) damage is not a detected
                    // player and must never disqualify a solo/last-hero card.
                    if DamageProvenance::from_str(&prov) == DamageProvenance::Unresolved {
                        continue;
                    }
                    let e = others.entry(oid).or_insert((0, None, 0));
                    e.0 += damage;
                    if *is_boss {
                        e.1 = combine(e.1, status);
                        e.2 += damage_taken;
                    }
                }
            }
        }

        if !local_seen {
            return Ok((false, false, false));
        }
        let local_present = matches!(local_anchor, Some(ParticipantEndStatus::Present));
        // Lone: local is the sole damage dealer across the whole card (boss phases
        // and any additional tracked entities).
        let lone =
            local_present && local_damage > 0 && others.values().all(|(dmg, _, _)| *dmg <= 0);
        // Last hero: consider only players who reached the boss anchor. Someone who
        // only touched an auxiliary entity and left never "finished the boss", so
        // they neither qualify nor disqualify. All boss-present others must have
        // died or nexused, and at least one such other must exist.
        let boss_others: Vec<&ParticipantEndStatus> = others
            .values()
            .filter_map(|(_, st, _)| st.as_ref())
            .collect();
        let last = local_present
            && !boss_others.is_empty()
            && boss_others.iter().all(|st| {
                matches!(
                    st,
                    ParticipantEndStatus::Died { .. } | ParticipantEndStatus::Nexused
                )
            });
        // Most damage taken: local took strictly more damage than every other
        // detected teammate across the card. Requires a real team (>=1 other) and
        // a positive local figure, so solo cards and data-less legacy rows never
        // flag.
        let most_damage_taken = local_damage_taken > 0
            && !others.is_empty()
            && others.values().all(|(_, _, dt)| local_damage_taken > *dt);
        Ok((lone, last, most_damage_taken))
    }

    /// Reconcile the lifetime award ledger against the current history and return
    /// the authoritative per-character lifetime totals
    /// (`char_id -> (lone, last, most)`). Called after inserts and once at startup
    /// (its first run backfills every historical qualifying card).
    ///
    /// The ledger is a persistent projection of each Exaltation card's current
    /// award flags, keyed by card. Existing cards are re-evaluated on every run so
    /// a late-arriving disqualifier (e.g. an auxiliary entity finalized after the
    /// boss) corrects the record; deleted cards are never revisited, so their last
    /// awarded value survives deletion. Totals are summed from the ledger, making
    /// the returned counts self-healing across crashes and repeated runs.
    pub fn reconcile_stat_awards(
        &mut self,
    ) -> SqlResult<std::collections::HashMap<i32, (i64, i64, i64)>> {
        let query = FightQuery {
            groups: Some(vec![crate::assets::BossGroup::Exaltation
                .as_str()
                .to_string()]),
            ..Default::default()
        };
        let cards = self.list_fights(&query, i64::MAX)?;
        let mut previous_owners = HashSet::new();
        for s in &cards {
            if s.local_char_id == 0 {
                continue;
            }
            let key = Self::stat_award_card_key(s);
            if let Some(previous_owner) = self
                .conn
                .query_row(
                    "SELECT char_id FROM stat_awards WHERE card_key = ?1",
                    params![&key],
                    |row| row.get::<_, i32>(0),
                )
                .optional()?
                .filter(|owner| *owner != 0 && *owner != s.local_char_id)
            {
                previous_owners.insert(previous_owner);
            }
            self.conn.execute(
                "INSERT INTO stat_awards (card_key, char_id, lone, last, most)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(card_key) DO UPDATE SET char_id = ?2, lone = ?3, last = ?4, most = ?5",
                params![
                    key,
                    s.local_char_id,
                    s.lone_fighter as i64,
                    s.last_hero_standing as i64,
                    s.most_damage_taken as i64
                ],
            )?;
        }
        let mut totals: std::collections::HashMap<i32, (i64, i64, i64)> =
            std::collections::HashMap::new();
        let mut stmt = self.conn.prepare(
            "SELECT char_id, COALESCE(SUM(lone), 0), COALESCE(SUM(last), 0), COALESCE(SUM(most), 0)
             FROM stat_awards GROUP BY char_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i32>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?;
        for row in rows {
            let (char_id, lone, last, most) = row?;
            totals.insert(char_id, (lone, last, most));
        }
        for previous_owner in previous_owners {
            totals.entry(previous_owner).or_insert((0, 0, 0));
        }
        Ok(totals)
    }

    /// Reconcile only the Exaltation cards changed by a persistence batch.
    ///
    /// Missing or non-Exaltation selections are deliberately ignored so a
    /// deleted card's last lifetime award remains in the ledger.
    pub fn reconcile_stat_awards_for_selections(
        &mut self,
        selections: &[FightSelection],
    ) -> SqlResult<HashMap<i32, (i64, i64, i64)>> {
        if selections.is_empty() {
            return Ok(HashMap::new());
        }
        // Leaves room for group and LIMIT binds below SQLite's default
        // 999-variable ceiling.
        const SELECTION_CHUNK_SIZE: usize = 400;

        let mut fight_ids = HashSet::new();
        let mut run_ids = HashSet::new();
        for selection in selections {
            match selection {
                FightSelection::Single(id) => {
                    fight_ids.insert(*id);
                }
                FightSelection::Encounter(run_id) => {
                    run_ids.insert(run_id.clone());
                }
            }
        }

        let query = FightQuery {
            groups: Some(vec![crate::assets::BossGroup::Exaltation
                .as_str()
                .to_string()]),
            ..Default::default()
        };
        let fight_ids: Vec<i64> = fight_ids.into_iter().collect();
        let run_ids: Vec<String> = run_ids.into_iter().collect();
        let mut cards = Vec::new();
        for chunk in fight_ids.chunks(SELECTION_CHUNK_SIZE) {
            cards.extend(self.standalone_summaries(&query, i64::MAX, Some(chunk))?);
        }
        for chunk in run_ids.chunks(SELECTION_CHUNK_SIZE) {
            cards.extend(self.encounter_summaries(&query, i64::MAX, Some(chunk))?);
        }

        let mut updates = Vec::new();
        for card in &cards {
            if card.local_char_id == 0 {
                continue;
            }
            let key = Self::stat_award_card_key(card);
            let previous_owner = self
                .conn
                .query_row(
                    "SELECT char_id FROM stat_awards WHERE card_key = ?1",
                    params![&key],
                    |row| row.get::<_, i32>(0),
                )
                .optional()?;
            let (lone, last, most) = self.card_secret_stats(card)?;
            updates.push((
                key,
                card.local_char_id,
                previous_owner,
                lone as i64,
                last as i64,
                most as i64,
            ));
        }

        let tx = self.conn.transaction()?;
        let mut affected_char_ids = HashSet::new();
        for (key, current_owner, previous_owner, lone, last, most) in updates {
            tx.execute(
                "INSERT INTO stat_awards (card_key, char_id, lone, last, most)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(card_key) DO UPDATE SET char_id = ?2, lone = ?3, last = ?4, most = ?5",
                params![key, current_owner, lone, last, most],
            )?;
            if let Some(char_id) = previous_owner.filter(|id| *id != 0) {
                affected_char_ids.insert(char_id);
            }
            affected_char_ids.insert(current_owner);
        }

        let mut totals = HashMap::new();
        for char_id in affected_char_ids {
            let total = tx.query_row(
                "SELECT COALESCE(SUM(lone), 0), COALESCE(SUM(last), 0),
                        COALESCE(SUM(most), 0)
                 FROM stat_awards WHERE char_id = ?1",
                params![char_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )?;
            totals.insert(char_id, total);
        }
        tx.commit()?;
        Ok(totals)
    }

    fn stat_award_card_key(card: &FightSummary) -> String {
        match &card.encounter_run_id {
            Some(run) => format!("E{run}"),
            None => format!("S{}", card.id),
        }
    }

    /// Sum every character's lifetime close calls from the persisted fight cards
    /// (`local_close_calls` across all fight rows they own). Each low-HP crossing
    /// is attributed to exactly one fight, so summing raw rows never double
    /// counts. Callers apply the result as a monotonic backfill (raising a
    /// character's tally, never lowering it).
    pub fn reconcile_close_calls(&self) -> SqlResult<std::collections::HashMap<i32, i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT local_char_id, COALESCE(SUM(local_close_calls), 0)
             FROM fights WHERE local_char_id != 0 GROUP BY local_char_id",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i32>(0)?, r.get::<_, i64>(1)?)))?;
        let mut totals: std::collections::HashMap<i32, i64> = std::collections::HashMap::new();
        for row in rows {
            let (char_id, total) = row?;
            if total > 0 {
                totals.insert(char_id, total);
            }
        }
        Ok(totals)
    }

    /// Minimum share of the boss's max HP a participant must deal to count as
    /// having contributed "substantial" damage for the flawless marker.
    const FLAWLESS_MIN_BOSS_DAMAGE_FRACTION: f64 = 0.05;

    /// Whether an Exaltation card earns the "flawless" marker: at
    /// least one participant who finished the fight (never died/nexused) dealt at
    /// least [`FLAWLESS_MIN_BOSS_DAMAGE_FRACTION`] of the boss's HP and took zero
    /// observed damage across every tracked entity in the card. `damage_taken`
    /// of `None`/`Some(0)` counts as "took zero" (a positive value is required to
    /// disqualify), since remote players who are never hit record no value.
    fn card_is_flawless(&self, s: &FightSummary) -> SqlResult<bool> {
        if s.boss_max_hp <= 0 {
            return Ok(false);
        }
        if crate::assets::boss_exempt_from_flawless(s.boss_object_type) {
            return Ok(false);
        }
        if crate::assets::boss_group(&s.dungeon, s.boss_object_type, &s.boss_name)
            != Some(crate::assets::BossGroup::Exaltation)
        {
            return Ok(false);
        }
        // Every fight in the card, flagged as a main-boss (headline type) phase.
        let fights: Vec<(i64, bool)> = match &s.encounter_run_id {
            Some(run) => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, boss_object_type FROM fights WHERE encounter_run_id = ?1",
                )?;
                let rows = stmt.query_map(params![run], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i32>(1)? == s.boss_object_type,
                    ))
                })?;
                rows.collect::<SqlResult<Vec<_>>>()?
            }
            None => vec![(s.id, true)],
        };
        let boss_ids: std::collections::HashSet<i64> = fights
            .iter()
            .filter(|(_, b)| *b)
            .map(|(id, _)| *id)
            .collect();
        if boss_ids.is_empty() {
            return Ok(false);
        }
        // Per participant across the whole card: (boss damage, took any positive
        // damage, finished the fight everywhere). `any_taken_recorded` guards
        // against legacy rows (recorded before damage-taken tracking) whose
        // all-NULL values would otherwise read as a factual zero taken.
        let mut acc: std::collections::HashMap<i32, (i64, bool, bool)> =
            std::collections::HashMap::new();
        let mut any_taken_recorded = false;
        for (fid, _) in &fights {
            let is_boss_fight = boss_ids.contains(fid);
            let mut stmt = self.conn.prepare(
                "SELECT object_id, damage, damage_taken, end_status
                 FROM fight_participants WHERE fight_id = ?1",
            )?;
            let rows = stmt.query_map(params![fid], |r| {
                Ok((
                    r.get::<_, i32>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?;
            for row in rows {
                let (oid, damage, taken, status) = row?;
                let e = acc.entry(oid).or_insert((0, false, true));
                if is_boss_fight {
                    e.0 += damage;
                }
                if taken.is_some() {
                    any_taken_recorded = true;
                }
                if taken.map(|t| t > 0).unwrap_or(false) {
                    e.1 = true;
                }
                if status != "present" {
                    e.2 = false;
                }
            }
        }
        if !any_taken_recorded {
            return Ok(false);
        }
        let threshold =
            (s.boss_max_hp as f64 * Self::FLAWLESS_MIN_BOSS_DAMAGE_FRACTION).ceil() as i64;
        Ok(acc.values().any(|(boss_dmg, took_positive, finished)| {
            *finished && !*took_positive && *boss_dmg >= threshold
        }))
    }

    /// Summaries for standalone (non-grouped) fights only.
    fn standalone_summaries(
        &self,
        query: &FightQuery,
        limit: i64,
        fight_ids: Option<&[i64]>,
    ) -> SqlResult<Vec<FightSummary>> {
        if fight_ids.is_some_and(|ids| ids.is_empty()) {
            return Ok(Vec::new());
        }
        let mut sql = String::from(
            r#"SELECT f.id, f.started_at, f.ended_at, f.dungeon, f.boss_object_type,
                      f.boss_name, f.boss_max_hp, f.boss_start_hp, f.local_char_id, f.killed,
                      (SELECT COUNT(*) FROM fight_participants p WHERE p.fight_id = f.id),
                      (SELECT p.object_type FROM fight_participants p
                         WHERE p.fight_id = f.id AND p.is_local = 1 LIMIT 1),
                      (SELECT p.skin_id FROM fight_participants p
                         WHERE p.fight_id = f.id AND p.is_local = 1 LIMIT 1),
                      (SELECT p.tex1 FROM fight_participants p
                         WHERE p.fight_id = f.id AND p.is_local = 1 LIMIT 1),
                      (SELECT p.tex2 FROM fight_participants p
                         WHERE p.fight_id = f.id AND p.is_local = 1 LIMIT 1),
                      (SELECT p.end_status FROM fight_participants p
                         WHERE p.fight_id = f.id AND p.is_local = 1 LIMIT 1),
                      f.map_seed, f.local_close_calls
               FROM fights f"#,
        );
        let mut conds: Vec<String> = vec!["f.encounter_run_id IS NULL".to_string()];
        let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        Self::apply_filters(query, &mut conds, &mut binds);
        if let Some(fight_ids) = fight_ids.filter(|ids| !ids.is_empty()) {
            conds.push(format!(
                "f.id IN ({})",
                std::iter::repeat_n("?", fight_ids.len())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            for id in fight_ids {
                binds.push(Box::new(*id));
            }
        }
        sql.push_str(" WHERE ");
        sql.push_str(&conds.join(" AND "));
        sql.push_str(" ORDER BY f.ended_at DESC LIMIT ?");
        binds.push(Box::new(limit));

        let mut stmt = self.conn.prepare(&sql)?;
        let params = rusqlite::params_from_iter(binds.iter().map(|b| b.as_ref()));
        let rows = stmt.query_map(params, |row| {
            let boss_object_type: i32 = row.get(4)?;
            let stored_name: String = row.get(5)?;
            let boss_name = display_boss_name(boss_object_type, &stored_name);
            Ok(FightSummary {
                id: row.get(0)?,
                started_at: row.get(1)?,
                ended_at: row.get(2)?,
                dungeon: row.get(3)?,
                map_seed: row.get(16)?,
                boss_object_type,
                boss_name: boss_name.clone(),
                boss_max_hp: row.get(6)?,
                boss_start_hp: row.get(7)?,
                local_char_id: row.get(8)?,
                killed: row.get::<_, i32>(9)? != 0,
                participant_count: row.get(10)?,
                local_object_type: row.get(11)?,
                local_skin_id: row.get::<_, Option<i32>>(12)?.unwrap_or(0),
                local_tex1: row.get::<_, Option<i32>>(13)?.unwrap_or(0) as u32,
                local_tex2: row.get::<_, Option<i32>>(14)?.unwrap_or(0) as u32,
                local_died: row.get::<_, Option<String>>(15)?.as_deref() == Some("died"),
                local_close_calls: row.get(17)?,
                encounter_run_id: None,
                encounter_id: None,
                phase_count: 1,
                killed_bosses: vec![(boss_object_type, boss_name)],
                flawless: false,
                lone_fighter: false,
                last_hero_standing: false,
                most_damage_taken: false,
            })
        })?;
        rows.collect()
    }

    /// The anchor phase of a run: the last real (non-aux) boss engaged, which is
    /// the final/main boss in a linear dungeon. Aux damage-summary fights (nests,
    /// hives, orbs, messengers, cores) are excluded from candidacy so a big add
    /// can never hijack the headline or completion state. Returns
    /// `(boss_object_type, boss_max_hp, boss_start_hp, killed)`.
    fn run_anchor(&self, run_id: &str) -> SqlResult<Option<(i32, i32, i32, bool)>> {
        let mut stmt = self.conn.prepare(
            "SELECT boss_object_type, boss_max_hp, boss_start_hp, killed, ended_at
             FROM fights WHERE encounter_run_id = ?1",
        )?;
        let rows: Vec<PhaseStat> = stmt
            .query_map(params![run_id], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get::<_, i32>(3)? != 0,
                    r.get(4)?,
                ))
            })?
            .collect::<SqlResult<_>>()?;
        Ok(pick_anchor(&rows))
    }

    /// One summary per encounter run. The headline is the dungeon name; boss
    /// stats/icon and completion come from the run anchor (last real boss); the
    /// participant count is distinct across all phases.
    fn encounter_summaries(
        &self,
        query: &FightQuery,
        limit: i64,
        run_ids: Option<&[String]>,
    ) -> SqlResult<Vec<FightSummary>> {
        if limit <= 0 || run_ids.is_some_and(|ids| ids.is_empty()) {
            return Ok(Vec::new());
        }

        let mut sql = String::from(
            "SELECT r.run_id, r.encounter_id, r.dungeon, r.started_at, r.ended_at, r.killed
             FROM encounter_runs r",
        );
        let mut conds = Vec::new();
        let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        Self::apply_encounter_filters(query, &mut conds, &mut binds);
        if let Some(run_ids) = run_ids.filter(|ids| !ids.is_empty()) {
            conds.push(format!(
                "r.run_id IN ({})",
                std::iter::repeat_n("?", run_ids.len())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            for run_id in run_ids {
                binds.push(Box::new(run_id.clone()));
            }
        }
        if !conds.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conds.join(" AND "));
        }
        sql.push_str(" ORDER BY r.ended_at DESC, r.started_at DESC LIMIT ?");
        binds.push(Box::new(limit));

        let mut stmt = self.conn.prepare(&sql)?;
        let params = rusqlite::params_from_iter(binds.iter().map(|b| b.as_ref()));
        let runs: Vec<(String, String, String, i64, i64, i32)> = stmt
            .query_map(params, |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })?
            .collect::<SqlResult<_>>()?;

        if runs.is_empty() {
            return Ok(Vec::new());
        }

        let placeholders = std::iter::repeat_n("?", runs.len())
            .collect::<Vec<_>>()
            .join(", ");
        let run_ids: Vec<&str> = runs.iter().map(|(id, ..)| id.as_str()).collect();
        let fight_sql = format!(
            "SELECT encounter_run_id, boss_object_type, boss_max_hp, boss_start_hp,
                    killed, ended_at, boss_name, local_char_id, started_at, map_seed,
                    local_close_calls
             FROM fights WHERE encounter_run_id IN ({placeholders})"
        );
        let mut stmt = self.conn.prepare(&fight_sql)?;
        let mut fights_by_run: HashMap<
            String,
            Vec<(i32, i32, i32, bool, i64, String, i32, i64, i32, i64)>,
        > = HashMap::new();
        for row in stmt.query_map(rusqlite::params_from_iter(run_ids.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get::<_, i32>(4)? != 0,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
                row.get(10)?,
            ))
        })? {
            let (
                run_id,
                otype,
                max_hp,
                start_hp,
                killed,
                ended_at,
                name,
                char_id,
                started_at,
                map_seed,
                close_calls,
            ) = row?;
            fights_by_run.entry(run_id).or_default().push((
                otype,
                max_hp,
                start_hp,
                killed,
                ended_at,
                name,
                char_id,
                started_at,
                map_seed,
                close_calls,
            ));
        }

        let participant_sql = format!(
            "SELECT f.encounter_run_id, COUNT(DISTINCT p.object_id),
                    MAX(CASE WHEN p.is_local = 1 THEN p.object_type END),
                    MAX(CASE WHEN p.is_local = 1 THEN p.skin_id END),
                    MAX(CASE WHEN p.is_local = 1 THEN p.tex1 END),
                    MAX(CASE WHEN p.is_local = 1 THEN p.tex2 END),
                    MAX(CASE WHEN p.is_local = 1 AND p.end_status = 'died' THEN 1 ELSE 0 END)
             FROM fights f LEFT JOIN fight_participants p ON p.fight_id = f.id
             WHERE f.encounter_run_id IN ({placeholders})
             GROUP BY f.encounter_run_id"
        );
        let mut stmt = self.conn.prepare(&participant_sql)?;
        let participant_stats: HashMap<String, LocalStats> = stmt
            .query_map(rusqlite::params_from_iter(run_ids.iter()), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    LocalStats {
                        participant_count: row.get(1)?,
                        local_object_type: row.get(2)?,
                        local_skin_id: row.get::<_, Option<i32>>(3)?.unwrap_or(0),
                        local_tex1: row.get::<_, Option<i32>>(4)?.unwrap_or(0) as u32,
                        local_tex2: row.get::<_, Option<i32>>(5)?.unwrap_or(0) as u32,
                        local_died: row.get::<_, i32>(6)? != 0,
                    },
                ))
            })?
            .collect::<SqlResult<_>>()?;

        let mut out = Vec::new();
        for (run_id, encounter_id, dungeon, started_at, ended_at, stored_killed) in runs {
            let phases = fights_by_run.remove(&run_id).unwrap_or_default();
            let phase_stats: Vec<PhaseStat> =
                phases.iter().map(|p| (p.0, p.1, p.2, p.3, p.4)).collect();
            let (mut boss_object_type, boss_max_hp, boss_start_hp, killed) =
                pick_anchor(&phase_stats).unwrap_or((0, 0, 0, false));
            // A run-level `killed` completes the card only for loot-completable
            // encounters (Towering Perfection): a bag from a core that was never
            // seen dying, where the anchor phase stays escaped. Other cards ignore
            // the run flag so an intermediate-form kill never hijacks completion.
            let loot_completable = phases
                .iter()
                .find_map(|p| crate::assets::encounter_for_boss_type(p.0))
                .is_some_and(|e| crate::assets::encounter_supports_loot_completion(e.id));
            let killed = killed || (loot_completable && stored_killed != 0);
            let raw_phase_count = phases.len() as i64;
            // The card's phase count must match the collapsed detail view:
            // duplicate same-(type,name) phases of a dedup-prone boss (a
            // re-detected Sage Genji on a summoner run, a re-spawned Prismimic)
            // fold to one. Every other phase -- including genuinely multi-instance
            // bosses like Pentaract towers -- counts individually.
            let phase_count = {
                use std::collections::HashSet;
                let mut seen: HashSet<(i32, String)> = HashSet::new();
                let mut count = 0i64;
                for p in &phases {
                    if crate::assets::is_dedup_prone_boss(p.0) {
                        if seen.insert((p.0, p.5.clone())) {
                            count += 1;
                        }
                    } else {
                        count += 1;
                    }
                }
                count
            };
            // Realm-grouped encounters are titled by the encounter name (not the
            // "Realm" map name); a multi-boss dungeon run is headlined by its
            // dungeon; a lone boss keeps its own name.
            let object_types: Vec<i32> = phases.iter().map(|p| p.0).collect();
            let display_name = if let Some(name) = realm_headline(&dungeon, &object_types) {
                name.to_string()
            } else if raw_phase_count > 1 {
                dungeon.clone()
            } else {
                phases
                    .iter()
                    .max_by_key(|phase| phase.4)
                    .map(|phase| display_boss_name(phase.0, &phase.5))
                    .unwrap_or_else(|| dungeon.clone())
            };
            let local_char_id = phases
                .iter()
                .find_map(|phase| (phase.6 != 0).then_some(phase.6))
                .unwrap_or(0);
            let local = participant_stats.get(&run_id).cloned().unwrap_or_default();
            let run_close_calls: i64 = phases.iter().map(|p| p.9 as i64).sum();

            // Boss line: the distinct real (non-aux) bosses that were killed, in
            // the order the player faced them. Ordering by start time keeps the
            // facing order stable even when bosses die within moments of each
            // other, where kill (ended_at) order would scramble them.
            let mut killed_bosses: Vec<(i32, String)> = Vec::new();
            let mut ordered = phases.clone();
            ordered.sort_by_key(|phase| (phase.7, phase.4));
            for phase in &ordered {
                let is_real = crate::assets::aux_target_for_type(phase.0).is_none();
                if is_real && phase.3 && !killed_bosses.iter().any(|(t, _)| *t == phase.0) {
                    killed_bosses.push((
                        phase.0,
                        strip_boss_phase_suffix(&display_boss_name(phase.0, &phase.5)).to_string(),
                    ));
                }
            }
            if killed_bosses.is_empty() {
                for phase in &ordered {
                    let is_real = crate::assets::aux_target_for_type(phase.0).is_none();
                    if is_real && !killed_bosses.iter().any(|(t, _)| *t == phase.0) {
                        killed_bosses.push((
                            phase.0,
                            strip_boss_phase_suffix(&display_boss_name(phase.0, &phase.5))
                                .to_string(),
                        ));
                    }
                }
            }
            if killed_bosses.is_empty() {
                killed_bosses.push((
                    boss_object_type,
                    realm_headline(&dungeon, &object_types)
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| dungeon.clone()),
                ));
            }
            // Some encounters (e.g. Cultist Hideout) spawn several alternative
            // cult-leader bosses that funnel into one headline boss; only the
            // headline name is meaningful on the card. The parent encounter_runs
            // row stores a "dungeon_run" sentinel, so resolve the real encounter
            // from the member boss types instead.
            let real_enc = ordered
                .iter()
                .find_map(|phase| crate::assets::encounter_for_boss_type(phase.0));
            if let Some(enc) = real_enc {
                if crate::assets::encounter_headline_only(enc.id) {
                    killed_bosses = vec![(enc.anchor_type, enc.display_name.to_string())];
                    // The card's picture is always the headline boss's, even when
                    // only escaping segments took damage and the anchor phase was
                    // never recorded (e.g. Towering Perfection's core).
                    boss_object_type = enc.anchor_type;
                }
            }

            out.push(FightSummary {
                id: 0,
                started_at,
                ended_at,
                dungeon,
                map_seed: phases.first().map(|p| p.8).unwrap_or(0),
                boss_object_type,
                boss_name: display_name,
                boss_max_hp,
                boss_start_hp,
                local_char_id,
                killed,
                participant_count: local.participant_count,
                local_object_type: local.local_object_type,
                local_skin_id: local.local_skin_id,
                local_tex1: local.local_tex1,
                local_tex2: local.local_tex2,
                local_died: local.local_died,
                local_close_calls: run_close_calls,
                encounter_run_id: Some(run_id),
                encounter_id: Some(encounter_id),
                phase_count,
                killed_bosses,
                flawless: false,
                lone_fighter: false,
                last_hero_standing: false,
                most_damage_taken: false,
            });
        }
        Ok(out)
    }

    /// Query full fight records (with participants) matching a filter.
    pub fn search_fights(&self, query: &FightQuery, limit: i64) -> SqlResult<Vec<FightRecord>> {
        let mut sql = String::from(
            r#"SELECT id, started_at, ended_at, dungeon, map_seed, boss_object_type,
                      boss_name, boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
                      local_close_calls, aux_member_count, dungeon_entered_at
               FROM fights"#,
        );
        let mut conds: Vec<String> = Vec::new();
        let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        Self::apply_filters_unqualified(query, &mut conds, &mut binds);
        if !conds.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conds.join(" AND "));
        }
        sql.push_str(" ORDER BY started_at DESC LIMIT ?");
        binds.push(Box::new(limit));

        let mut stmt = self.conn.prepare(&sql)?;
        let params = rusqlite::params_from_iter(binds.iter().map(|b| b.as_ref()));
        let rows = stmt.query_map(params, |row| Self::map_fight_header(row))?;
        let mut fights: Vec<FightRecord> = rows.collect::<SqlResult<_>>()?;
        for fight in &mut fights {
            fight.participants = self.participants_for(fight.id)?;
        }
        Ok(fights)
    }

    /// Load a single fight (header + participants) by id.
    pub fn fight_detail(&self, fight_id: i64) -> SqlResult<Option<FightRecord>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT id, started_at, ended_at, dungeon, map_seed, boss_object_type,
                      boss_name, boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
                      local_close_calls, aux_member_count, dungeon_entered_at
               FROM fights WHERE id = ?1"#,
        )?;
        let mut rows = stmt.query_map(params![fight_id], |row| Self::map_fight_header(row))?;
        let Some(first) = rows.next() else {
            return Ok(None);
        };
        let mut fight = first?;
        fight.boss_name = display_boss_name(fight.boss_object_type, &fight.boss_name);
        fight.participants = self.participants_for(fight.id)?;
        Ok(Some(fight))
    }

    /// Load a grouped encounter (ordered phases + aggregated roster) by run id.
    pub fn encounter_detail(&self, run_id: &str) -> SqlResult<Option<EncounterRecord>> {
        let run = self.conn.query_row(
            "SELECT encounter_id, dungeon, started_at, ended_at, killed
             FROM encounter_runs WHERE run_id = ?1",
            params![run_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i32>(4)? != 0,
                ))
            },
        );
        let (encounter_id, dungeon, started_at, ended_at, stored_killed) = match run {
            Ok(r) => r,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e),
        };
        let anchor = self.run_anchor(run_id)?;
        let anchor_killed = anchor.map(|a| a.3).unwrap_or(false);
        let anchor_object_type = anchor.map(|a| a.0).unwrap_or(0);

        // Ordered phases with hydrated participants.
        let mut stmt = self.conn.prepare(
            r#"SELECT id, started_at, ended_at, dungeon, map_seed, boss_object_type,
                      boss_name, boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
                      local_close_calls, aux_member_count, dungeon_entered_at
               FROM fights WHERE encounter_run_id = ?1 ORDER BY started_at ASC, id ASC"#,
        )?;
        let mut phases: Vec<FightRecord> = stmt
            .query_map(params![run_id], |row| Self::map_fight_header(row))?
            .collect::<SqlResult<_>>()?;
        for phase in &mut phases {
            phase.participants = self.participants_for(phase.id)?;
        }

        let object_types: Vec<i32> = phases.iter().map(|p| p.boss_object_type).collect();
        // A run-level `killed` completes the card only for loot-completable
        // encounters (Towering Perfection), where the core can drop its bag
        // without ever being seen dying; other cards keep pure anchor completion.
        let loot_completable = object_types
            .iter()
            .find_map(|&t| crate::assets::encounter_for_boss_type(t))
            .is_some_and(|e| crate::assets::encounter_supports_loot_completion(e.id));
        let killed = anchor_killed || (loot_completable && stored_killed);
        let display_name = if let Some(name) = realm_headline(&dungeon, &object_types) {
            name.to_string()
        } else if phases.len() > 1 {
            dungeon.clone()
        } else {
            phases
                .first()
                .map(|p| p.boss_name.clone())
                .unwrap_or_else(|| dungeon.clone())
        };
        let roster = aggregate_roster(&phases);
        let anchor_statuses = anchor_end_statuses(&phases, anchor_object_type);
        let mut phases = collapse_duplicate_boss_phases(&collapse_aux_phases(&phases));
        for phase in &mut phases {
            phase.boss_name = display_boss_name(phase.boss_object_type, &phase.boss_name);
        }
        unify_phase_end_statuses(&mut phases, &anchor_statuses);
        // A death seen on a concurrent add-on phase (e.g. dying to a Nest boss's
        // invulnerable-phase adds) is inherited by the main boss and every other
        // phase, so it isn't hidden as a spurious Nexused on the boss card.
        propagate_aux_deaths(&mut phases);

        Ok(Some(EncounterRecord {
            run_id: run_id.to_string(),
            encounter_id,
            display_name,
            dungeon,
            started_at,
            ended_at,
            killed,
            anchor_object_type,
            phases,
            roster,
        }))
    }

    /// Delete either a single fight or a whole encounter run, transactionally.
    pub fn delete_selection(&mut self, selection: &FightSelection) -> SqlResult<()> {
        match selection {
            FightSelection::Single(id) => self.delete_fight(*id),
            FightSelection::Encounter(run_id) => {
                let tx = self.conn.transaction()?;
                tx.execute(
                    "DELETE FROM fight_participants WHERE fight_id IN
                     (SELECT id FROM fights WHERE encounter_run_id = ?1)",
                    params![run_id],
                )?;
                tx.execute(
                    "DELETE FROM fights WHERE encounter_run_id = ?1",
                    params![run_id],
                )?;
                tx.execute(
                    "DELETE FROM encounter_runs WHERE run_id = ?1",
                    params![run_id],
                )?;
                tx.commit()
            }
        }
    }

    /// Delete many selections (single fights and/or encounter runs) in one
    /// transaction. Returns the number of selections processed.
    pub fn delete_selections(&mut self, selections: &[FightSelection]) -> SqlResult<usize> {
        if selections.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.transaction()?;
        for selection in selections {
            match selection {
                FightSelection::Single(id) => {
                    tx.execute(
                        "DELETE FROM fight_participants WHERE fight_id = ?1",
                        params![id],
                    )?;
                    tx.execute("DELETE FROM fights WHERE id = ?1", params![id])?;
                }
                FightSelection::Encounter(run_id) => {
                    tx.execute(
                        "DELETE FROM fight_participants WHERE fight_id IN
                         (SELECT id FROM fights WHERE encounter_run_id = ?1)",
                        params![run_id],
                    )?;
                    tx.execute(
                        "DELETE FROM fights WHERE encounter_run_id = ?1",
                        params![run_id],
                    )?;
                    tx.execute(
                        "DELETE FROM encounter_runs WHERE run_id = ?1",
                        params![run_id],
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(selections.len())
    }

    /// Every selection identity matching a query (grouped rows as one
    /// `Encounter` selection, standalone fights as `Single`). Used by the
    /// "delete filtered" action to remove all matches, not just the visible page.
    pub fn selections_matching(&self, query: &FightQuery) -> SqlResult<Vec<FightSelection>> {
        // Secret-stat flags are computed at read time, so reuse the list path
        // (unbounded) to keep "Select filtered" identical to what the list shows.
        if query.filter_lone_fighter
            || query.filter_last_hero
            || query.filter_most_damage_taken
            || query.filter_close_calls
        {
            return self.all_selections(query, i64::MAX);
        }
        let mut sql = String::from("SELECT f.id, f.started_at, f.ended_at FROM fights f");
        let mut conds = vec!["f.encounter_run_id IS NULL".to_string()];
        let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        Self::apply_filters(query, &mut conds, &mut binds);
        sql.push_str(" WHERE ");
        sql.push_str(&conds.join(" AND "));

        let mut stmt = self.conn.prepare(&sql)?;
        let params = rusqlite::params_from_iter(binds.iter().map(|b| b.as_ref()));
        let mut selections: Vec<(FightSelection, i64, i64)> = stmt
            .query_map(params, |row| {
                Ok((
                    FightSelection::Single(row.get(0)?),
                    row.get(1)?,
                    row.get(2)?,
                ))
            })?
            .collect::<SqlResult<_>>()?;

        let mut sql =
            String::from("SELECT r.run_id, r.started_at, r.ended_at FROM encounter_runs r");
        let mut conds = Vec::new();
        let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        Self::apply_encounter_filters(query, &mut conds, &mut binds);
        if !conds.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conds.join(" AND "));
        }

        let mut stmt = self.conn.prepare(&sql)?;
        let params = rusqlite::params_from_iter(binds.iter().map(|b| b.as_ref()));
        selections.extend(
            stmt.query_map(params, |row| {
                Ok((
                    FightSelection::Encounter(row.get(0)?),
                    row.get(1)?,
                    row.get(2)?,
                ))
            })?
            .collect::<SqlResult<Vec<_>>>()?,
        );
        selections.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)));
        Ok(selections
            .into_iter()
            .map(|(selection, _, _)| selection)
            .collect())
    }

    /// Every selection identity matching a query, up to `limit`.
    ///
    /// This compatibility API retains the list query's bounded retrieval.
    pub fn all_selections(&self, query: &FightQuery, limit: i64) -> SqlResult<Vec<FightSelection>> {
        Ok(self
            .list_fights(query, limit)?
            .into_iter()
            .map(|summary| summary.selection())
            .collect())
    }

    /// Find the killed fight card a loot drop belongs to, matched by map
    /// instance (`map_seed`), boss/mob type, and drop time falling inside the
    /// fight window widened by the link tolerances. Returns the card selection
    /// (grouped run or single fight) nearest in time, or `None` when nothing
    /// matches. A drop with `map_seed == 0` or a non-boss source never matches,
    /// since the instance is unknown. Dungeon name is intentionally NOT part of
    /// the key: the loot and combat subsystems can store it differently (e.g. a
    /// raw `{s.wine_cellar}` token vs the resolved `Wine Cellar`), and the
    /// random per-instance seed already disambiguates the map.
    pub fn find_killed_fight(
        &self,
        map_seed: i32,
        mob_type: i32,
        timestamp: i64,
    ) -> SqlResult<Option<FightSelection>> {
        if map_seed == 0 || mob_type <= 0 {
            return Ok(None);
        }
        // A self-destructing boss's loot is emitted by a proxy chest (Legacy Lair
        // of Draconis dragons). Resolve the chest's drops back to the boss fight.
        let mob_type = crate::assets::boss_for_loot_emitter(mob_type).unwrap_or(mob_type);
        let lo = timestamp - LOOT_LINK_POST_MS;
        let hi = timestamp + LOOT_LINK_PRE_MS;
        let mut stmt = self.conn.prepare(
            "SELECT id, encounter_run_id FROM fights
             WHERE killed = 1 AND map_seed = ?1 AND boss_object_type = ?2
               AND ended_at >= ?3 AND started_at <= ?4
             ORDER BY ABS(ended_at - ?5) ASC, id ASC LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![map_seed, mob_type, lo, hi, timestamp], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        match rows.next() {
            Some(row) => {
                let (id, run) = row?;
                Ok(Some(match run {
                    Some(run) => FightSelection::Encounter(run),
                    None => FightSelection::Single(id),
                }))
            }
            None => Ok(None),
        }
    }

    /// Latch `killed = 1` on the encounter run(s) of a loot-completed realm
    /// event for a given map instance, keyed by the member fights' `encounter_id`
    /// and `map_seed`. Used when a bag from the event's core boss proves the
    /// event finished even though the core was never observed dying (see
    /// [`crate::assets::encounter_loot_completes`]). No-op when no matching run
    /// exists yet. The random per-instance `map_seed` scopes it to one realm.
    pub fn mark_encounter_killed_by_loot(
        &mut self,
        map_seed: i32,
        encounter_id: &str,
    ) -> SqlResult<()> {
        if map_seed == 0 {
            return Ok(());
        }
        self.conn.execute(
            "UPDATE encounter_runs SET killed = 1
             WHERE killed = 0 AND run_id IN (
                 SELECT DISTINCT encounter_run_id FROM fights
                 WHERE encounter_id = ?1 AND map_seed = ?2
                   AND encounter_run_id IS NOT NULL)",
            params![encounter_id, map_seed],
        )?;
        Ok(())
    }

    /// Delete every recorded fight, participant, and encounter run, then reclaim
    /// the freed pages with `VACUUM` so the database file shrinks on disk.
    pub fn clear_all(&mut self) -> SqlResult<()> {
        {
            let tx = self.conn.transaction()?;
            tx.execute("DELETE FROM fight_participants", [])?;
            tx.execute("DELETE FROM fights", [])?;
            tx.execute("DELETE FROM encounter_runs", [])?;
            tx.commit()?;
        }
        // VACUUM cannot run inside a transaction.
        self.conn.execute_batch("VACUUM")?;
        Ok(())
    }

    /// Total on-disk size of the combat history database in bytes, including the
    /// `-wal` and `-shm` sidecar files (WAL mode keeps recent writes there until
    /// a checkpoint). Returns 0 when the file does not yet exist. The database
    /// path is supplied explicitly so no machine-wide default is resolved here.
    pub fn database_size_bytes(path: &Path) -> u64 {
        let mut total = 0u64;
        for suffix in ["", "-wal", "-shm"] {
            let p = if suffix.is_empty() {
                path.to_path_buf()
            } else {
                let mut os = path.to_path_buf().into_os_string();
                os.push(suffix);
                PathBuf::from(os)
            };
            if let Ok(meta) = std::fs::metadata(&p) {
                total += meta.len();
            }
        }
        total
    }

    /// Build filter predicates for the list query (columns prefixed `f.`).
    fn apply_filters(
        query: &FightQuery,
        conds: &mut Vec<String>,
        binds: &mut Vec<Box<dyn rusqlite::ToSql>>,
    ) {
        Self::push_filters(query, conds, binds, "f.");
    }

    /// Build filter predicates for the search query (unqualified columns).
    fn apply_filters_unqualified(
        query: &FightQuery,
        conds: &mut Vec<String>,
        binds: &mut Vec<Box<dyn rusqlite::ToSql>>,
    ) {
        Self::push_filters(query, conds, binds, "");
    }

    fn apply_encounter_filters(
        query: &FightQuery,
        conds: &mut Vec<String>,
        binds: &mut Vec<Box<dyn rusqlite::ToSql>>,
    ) {
        if let Some(after) = query.after {
            conds.push("r.started_at >= ?".to_string());
            binds.push(Box::new(after));
        }
        if let Some(before) = query.before {
            conds.push("r.started_at <= ?".to_string());
            binds.push(Box::new(before));
        }
        if let Some(dungeon) = &query.dungeon {
            conds.push("r.dungeon = ?".to_string());
            binds.push(Box::new(dungeon.clone()));
        }
        if let Some(char_id) = query.char_id {
            conds.push(
                "EXISTS (SELECT 1 FROM fights f
                 WHERE f.encounter_run_id = r.run_id AND f.local_char_id = ?)"
                    .to_string(),
            );
            binds.push(Box::new(char_id));
        }
        if let Some(name) = query.player_name.as_ref().filter(|s| !s.is_empty()) {
            conds.push(
                "EXISTS (SELECT 1 FROM fights f
                 JOIN fight_participants p ON p.fight_id = f.id
                 WHERE f.encounter_run_id = r.run_id AND p.name = ?)"
                    .to_string(),
            );
            binds.push(Box::new(name.clone()));
        }
        if let Some(count) = query.participant_count {
            conds.push(
                "(SELECT COUNT(DISTINCT p.object_id) FROM fights f
                 JOIN fight_participants p ON p.fight_id = f.id
                 WHERE f.encounter_run_id = r.run_id) = ?"
                    .to_string(),
            );
            binds.push(Box::new(count));
        }
        if let Some(boss) = query.boss_object_type {
            conds.push(
                "EXISTS (SELECT 1 FROM fights f
                 WHERE f.encounter_run_id = r.run_id AND f.boss_object_type = ?)"
                    .to_string(),
            );
            binds.push(Box::new(boss));
        }
        if let Some(groups) = query.groups.as_ref().filter(|g| !g.is_empty()) {
            let placeholders = std::iter::repeat_n("?", groups.len())
                .collect::<Vec<_>>()
                .join(", ");
            conds.push(format!(
                "EXISTS (SELECT 1 FROM fights f
                 WHERE f.encounter_run_id = r.run_id AND f.boss_group IN ({}))",
                placeholders
            ));
            for group in groups {
                binds.push(Box::new(group.clone()));
            }
        }
        if let Some(text) = query
            .text
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            let like = format!("%{}%", text.to_lowercase());
            let canonical_ids = crate::assets::encounter_ids_matching_name(text);
            let mut text_conds = vec![
                "LOWER(r.dungeon) LIKE ?".to_string(),
                "EXISTS (SELECT 1 FROM fights f WHERE f.encounter_run_id = r.run_id
                 AND (LOWER(f.boss_name) LIKE ? OR LOWER(f.dungeon) LIKE ?))"
                    .to_string(),
            ];
            binds.push(Box::new(like.clone()));
            binds.push(Box::new(like.clone()));
            binds.push(Box::new(like));
            if !canonical_ids.is_empty() {
                text_conds.push(format!(
                    "r.encounter_id IN ({})",
                    std::iter::repeat_n("?", canonical_ids.len())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                for id in canonical_ids {
                    binds.push(Box::new(id.to_string()));
                }
            }
            conds.push(format!("({})", text_conds.join(" OR ")));
        }
    }

    fn push_filters(
        query: &FightQuery,
        conds: &mut Vec<String>,
        binds: &mut Vec<Box<dyn rusqlite::ToSql>>,
        prefix: &str,
    ) {
        if let Some(text) = query
            .text
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            conds.push(format!(
                "({p}boss_name LIKE ? OR {p}dungeon LIKE ?)",
                p = prefix
            ));
            let like = format!("%{}%", text);
            binds.push(Box::new(like.clone()));
            binds.push(Box::new(like));
        }
        if let Some(dungeon) = &query.dungeon {
            conds.push(format!("{}dungeon = ?", prefix));
            binds.push(Box::new(dungeon.clone()));
        }
        if let Some(boss) = query.boss_object_type {
            conds.push(format!("{}boss_object_type = ?", prefix));
            binds.push(Box::new(boss));
        }
        if let Some(char_id) = query.char_id {
            // Unknown-identity fights (0) are excluded from the character filter.
            conds.push(format!("{}local_char_id = ?", prefix));
            binds.push(Box::new(char_id));
        }
        if let Some(name) = query.player_name.as_ref().filter(|s| !s.is_empty()) {
            // Correlate on the outer fight's id column. The list query aliases
            // `fights` as `f`; `search_fights` uses the unqualified table, so a
            // bare `id` would resolve to `fight_participants.id` inside the
            // subquery -- qualify it explicitly.
            let fight_id = if prefix.is_empty() {
                "fights.id"
            } else {
                "f.id"
            };
            conds.push(format!(
                "EXISTS (SELECT 1 FROM fight_participants p \
                 WHERE p.fight_id = {} AND p.name = ?)",
                fight_id
            ));
            binds.push(Box::new(name.clone()));
        }
        if let Some(count) = query.participant_count {
            let fight_id = if prefix.is_empty() {
                "fights.id"
            } else {
                "f.id"
            };
            conds.push(format!(
                "(SELECT COUNT(*) FROM fight_participants p WHERE p.fight_id = {}) = ?",
                fight_id
            ));
            binds.push(Box::new(count));
        }
        if let Some(after) = query.after {
            conds.push(format!("{}started_at >= ?", prefix));
            binds.push(Box::new(after));
        }
        if let Some(before) = query.before {
            conds.push(format!("{}started_at <= ?", prefix));
            binds.push(Box::new(before));
        }
        if let Some(groups) = query.groups.as_ref().filter(|g| !g.is_empty()) {
            let placeholders = std::iter::repeat_n("?", groups.len())
                .collect::<Vec<_>>()
                .join(", ");
            conds.push(format!("{}boss_group IN ({})", prefix, placeholders));
            for g in groups {
                binds.push(Box::new(g.clone()));
            }
        }
    }

    fn participants_for(&self, fight_id: i64) -> SqlResult<Vec<ParticipantRecord>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT object_id, object_type, name, equip0, equip1, equip2, equip3,
                      enchants, damage, hits, is_local, provenance, skin_id, tex1, tex2,
                      end_status, grave_type, pet_type, pet_abilities, damage_taken,
                      damage_taken_provenance, guarded_damage, guarded_hits, damage_blocked
               FROM fight_participants WHERE fight_id = ?1 ORDER BY damage DESC, hits DESC"#,
        )?;
        let rows = stmt.query_map(params![fight_id], |row| {
            let pet_type: i32 = row.get(17)?;
            let pet = if pet_type > 0 {
                Some(PetInfo {
                    pet_type,
                    abilities: parse_pet_abilities(row.get::<_, Option<String>>(18)?.as_deref()),
                })
            } else {
                None
            };
            let damage_taken: Option<i64> = row.get(19)?;
            let damage_taken_provenance = row
                .get::<_, Option<String>>(20)?
                .map(|s| DamageTakenProvenance::from_str(&s))
                .unwrap_or(DamageTakenProvenance::Observed);
            let guarded_damage: Option<i64> = row.get(21)?;
            let guarded_hits: Option<i64> = row.get(22)?;
            let damage_blocked: Option<i64> = row.get(23)?;
            Ok(ParticipantRecord {
                object_id: row.get(0)?,
                object_type: row.get(1)?,
                name: row.get(2)?,
                equipment: [row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?],
                equipment_enchants: parse_enchants(row.get::<_, Option<String>>(7)?.as_deref()),
                damage: row.get(8)?,
                hits: row.get(9)?,
                is_local: row.get::<_, i32>(10)? != 0,
                provenance: DamageProvenance::from_str(&row.get::<_, String>(11)?),
                skin_id: row.get(12)?,
                tex1: row.get::<_, i32>(13)? as u32,
                tex2: row.get::<_, i32>(14)? as u32,
                end_status: ParticipantEndStatus::from_str(
                    &row.get::<_, String>(15)?,
                    row.get::<_, i32>(16)?,
                ),
                pet,
                damage_taken,
                damage_taken_provenance,
                damage_blocked,
                guarded_damage,
                guarded_hits,
            })
        })?;
        rows.collect()
    }

    /// Total number of stored fights.
    pub fn fight_count(&self) -> SqlResult<i64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM fights", [], |row| row.get(0))
    }

    /// Delete a single fight and its participant rows. Requires a writer
    /// connection (participants are removed explicitly so the delete does not
    /// depend on the `foreign_keys` pragma being enabled).
    pub fn delete_fight(&self, fight_id: i64) -> SqlResult<()> {
        self.conn.execute(
            "DELETE FROM fight_participants WHERE fight_id = ?1",
            [fight_id],
        )?;
        self.conn
            .execute("DELETE FROM fights WHERE id = ?1", [fight_id])?;
        Ok(())
    }

    /// Distinct dungeon names across all recorded fights (for search suggestions).
    pub fn distinct_dungeons(&self) -> SqlResult<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT dungeon FROM fights \
             WHERE dungeon IS NOT NULL AND dungeon <> '' ORDER BY dungeon",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect()
    }

    /// Distinct bosses `(object_type, name)` across all recorded fights.
    pub fn distinct_bosses(&self) -> SqlResult<Vec<(i32, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT boss_object_type, boss_name FROM fights \
             WHERE boss_object_type <> 0 AND boss_name <> '' ORDER BY boss_name",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i32>(0)?, row.get::<_, String>(1)?))
        })?;
        // Collapse the Marble Colossus survival phases (which share an object
        // type) into a single "Marble Colossus" suggestion. Filtering is by
        // object type, so the unified suggestion still matches both phases.
        let mut out: Vec<(i32, String)> = Vec::new();
        for row in rows {
            let (obj_type, name) = row?;
            let name = strip_boss_phase_suffix(&name).to_string();
            if !out.iter().any(|(t, n)| *t == obj_type && *n == name) {
                out.push((obj_type, name));
            }
        }
        Ok(out)
    }

    /// Distinct participant IGNs across all recorded fights (for player search
    /// suggestions), each paired with the sprite id and dye textures of the
    /// character they were most recently seen as. Synthetic roster placeholders
    /// ('You' for an unresolved local player, 'Unknown' for unresolved attackers)
    /// are excluded. Returns `(name, sprite_id, tex1, tex2)`.
    pub fn distinct_players(&self) -> SqlResult<Vec<(String, i32, u32, u32)>> {
        let mut stmt = self.conn.prepare(
            "SELECT name, CASE WHEN skin_id > 0 THEN skin_id ELSE object_type END, tex1, tex2 \
             FROM ( \
                SELECT p.name AS name, p.object_type AS object_type, p.skin_id AS skin_id, \
                       p.tex1 AS tex1, p.tex2 AS tex2, \
                       ROW_NUMBER() OVER ( \
                           PARTITION BY p.name ORDER BY f.ended_at DESC, f.id DESC \
                       ) AS rn \
                FROM fight_participants p \
                JOIN fights f ON f.id = p.fight_id \
                WHERE p.name <> '' AND p.name NOT IN ('You', 'Unknown') \
             ) WHERE rn = 1 \
             ORDER BY name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i32>(1)?,
                row.get::<_, i32>(2)? as u32,
                row.get::<_, i32>(3)? as u32,
            ))
        })?;
        rows.collect()
    }

    /// Distinct participant counts present across all stored cards, ascending.
    /// Backs the player-count search suggestions. Standalone fights count all
    /// participant rows; grouped encounters count distinct object ids across
    /// phases, matching the count shown on each card. Zero is excluded.
    pub fn distinct_participant_counts(&self) -> SqlResult<Vec<i64>> {
        let mut set = std::collections::BTreeSet::new();
        let mut stmt = self.conn.prepare(
            "SELECT COUNT(*) FROM fight_participants p
             JOIN fights f ON f.id = p.fight_id
             WHERE f.encounter_run_id IS NULL GROUP BY f.id",
        )?;
        for row in stmt.query_map([], |r| r.get::<_, i64>(0))? {
            set.insert(row?);
        }
        let mut stmt = self.conn.prepare(
            "SELECT COUNT(DISTINCT p.object_id) FROM fights f
             JOIN fight_participants p ON p.fight_id = f.id
             WHERE f.encounter_run_id IS NOT NULL GROUP BY f.encounter_run_id",
        )?;
        for row in stmt.query_map([], |r| r.get::<_, i64>(0))? {
            set.insert(row?);
        }
        set.remove(&0);
        Ok(set.into_iter().collect())
    }
}

/// Serialize per-slot equipment enchants for storage: four slots joined by `;`,
/// with each slot's ids joined by `,` (e.g. `"1,2;;3;"`). Round-trips via
/// [`parse_enchants`].
fn serialize_enchants(enchants: &[Vec<u16>; 4]) -> String {
    enchants
        .iter()
        .map(|slot| {
            slot.iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// Parse the stored enchant string back into four slots. A `None`/empty value
/// (legacy rows) yields four empty slots; missing trailing slots are padded and
/// segments past slot 3 are ignored; unparseable ids are skipped.
fn parse_enchants(data: Option<&str>) -> [Vec<u16>; 4] {
    let data = match data {
        Some(d) if !d.is_empty() => d,
        _ => return Default::default(),
    };
    let mut slots = data.split(';');
    std::array::from_fn(|_| {
        slots
            .next()
            .map(|slot| {
                slot.split(',')
                    .filter_map(|id| id.trim().parse::<u16>().ok())
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// Serialize pet ability type ids as a comma-separated string.
fn serialize_pet_abilities(abilities: &[i32]) -> String {
    abilities
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// Parse the stored pet ability string back into ability type ids. A
/// `None`/empty value yields an empty vec; unparseable ids are skipped.
fn parse_pet_abilities(data: Option<&str>) -> Vec<i32> {
    match data {
        Some(d) if !d.is_empty() => d
            .split(',')
            .filter_map(|id| id.trim().parse::<i32>().ok())
            .collect(),
        _ => Vec::new(),
    }
}

/// Aggregate a set of phase fights into one participant roster.
///
/// Local hits merge under a single entry (there is only one local player); remote
/// participants merge by `(object_id, name)` which is stable within a run's map
/// instance. Damage and hits sum; the equipment shown is from the phase where the
/// participant dealt the most damage; provenance is reduced worst-case so the
/// encounter total never over-claims accuracy (all-pending -> SelfPending,
/// all-exact -> SelfComputed, any mix or partial -> SelfPartial; remote stays
/// Observed).
/// Combine two end-statuses when aggregating a participant across grouped phases.
/// Priority is `Died` > `Present` > `Nexused`: a correlated grave (positive death
/// evidence) always wins, otherwise being present at the end of any phase beats a
/// nexused/out-of-range flag from another phase (e.g. a side object that finalized
/// by timeout after the boss died).
fn combine_end_status(a: ParticipantEndStatus, b: ParticipantEndStatus) -> ParticipantEndStatus {
    use ParticipantEndStatus::*;
    match (a, b) {
        (Died { .. }, _) => a,
        (_, Died { .. }) => b,
        (Present, _) | (_, Present) => Present,
        _ => Nexused,
    }
}

fn aggregate_roster(phases: &[FightRecord]) -> Vec<ParticipantRecord> {
    use std::collections::HashMap;

    struct Accum {
        rec: ParticipantRecord,
        best_equip_damage: i64,
        self_pending: u32,
        self_partial: u32,
        self_computed: u32,
        self_reconciled: u32,
        self_server_reconciled: u32,
        self_estimated: u32,
        taken_sum: i64,
        taken_any: bool,
        taken_partial: bool,
        taken_estimated: bool,
        blocked_sum: i64,
        blocked_any: bool,
        guarded_dmg_sum: i64,
        guarded_hits_sum: i64,
        guarded_any: bool,
    }

    let mut by_key: HashMap<(bool, i32, String), Accum> = HashMap::new();
    for phase in phases {
        for p in &phase.participants {
            // Local player collapses to one key; remotes key by (object_id, name).
            let key = if p.is_local {
                (true, 0, String::new())
            } else {
                (false, p.object_id, p.name.clone())
            };
            let entry = by_key.entry(key).or_insert_with(|| Accum {
                rec: ParticipantRecord {
                    object_id: p.object_id,
                    object_type: p.object_type,
                    skin_id: p.skin_id,
                    tex1: p.tex1,
                    tex2: p.tex2,
                    name: p.name.clone(),
                    equipment: p.equipment,
                    equipment_enchants: p.equipment_enchants.clone(),
                    damage: 0,
                    hits: 0,
                    is_local: p.is_local,
                    provenance: p.provenance,
                    end_status: p.end_status,
                    pet: p.pet.clone(),
                    damage_taken: None,
                    damage_taken_provenance: DamageTakenProvenance::Observed,
                    damage_blocked: None,
                    guarded_damage: None,
                    guarded_hits: None,
                },
                best_equip_damage: i64::MIN,
                self_pending: 0,
                self_partial: 0,
                self_computed: 0,
                self_reconciled: 0,
                self_server_reconciled: 0,
                self_estimated: 0,
                taken_sum: 0,
                taken_any: false,
                taken_partial: false,
                taken_estimated: false,
                blocked_sum: 0,
                blocked_any: false,
                guarded_dmg_sum: 0,
                guarded_hits_sum: 0,
                guarded_any: false,
            });
            entry.rec.damage += p.damage;
            entry.rec.hits += p.hits;
            // Combine end-status across phases with priority Died > Present >
            // Nexused. A participant present at the end of any phase was there
            // (side objects such as Nest hives finalize by timeout ~10s after the
            // boss dies and would otherwise flag every still-present player as
            // nexused); a correlated grave (Died) is positive evidence that wins.
            entry.rec.end_status = combine_end_status(entry.rec.end_status, p.end_status);
            if p.damage > entry.best_equip_damage {
                entry.best_equip_damage = p.damage;
                entry.rec.equipment = p.equipment;
                entry.rec.equipment_enchants = p.equipment_enchants.clone();
                entry.rec.object_type = p.object_type;
                entry.rec.skin_id = p.skin_id;
                entry.rec.tex1 = p.tex1;
                entry.rec.tex2 = p.tex2;
            }
            // Carry a pet forward from any phase that has one (prefer keeping an
            // existing match over a later None).
            if entry.rec.pet.is_none() && p.pet.is_some() {
                entry.rec.pet = p.pet.clone();
            }
            if entry.rec.name.is_empty() && !p.name.is_empty() {
                entry.rec.name = p.name.clone();
            }
            match p.provenance {
                DamageProvenance::SelfPending => entry.self_pending += 1,
                DamageProvenance::SelfPartial => entry.self_partial += 1,
                DamageProvenance::SelfComputed => entry.self_computed += 1,
                DamageProvenance::SelfReconciled => entry.self_reconciled += 1,
                DamageProvenance::SelfServerReconciled => entry.self_server_reconciled += 1,
                DamageProvenance::SelfEstimated => entry.self_estimated += 1,
                // Sticky across grouped phases: if the local player was absent at
                // the start of any phase, the remote figure stays partial.
                DamageProvenance::PartiallyObserved => {
                    entry.rec.provenance = DamageProvenance::PartiallyObserved;
                }
                _ => {}
            }
            if let Some(dt) = p.damage_taken {
                entry.taken_sum += dt;
                entry.taken_any = true;
                match p.damage_taken_provenance {
                    DamageTakenProvenance::Partial => entry.taken_partial = true,
                    DamageTakenProvenance::Estimated => entry.taken_estimated = true,
                    DamageTakenProvenance::Observed => {}
                }
            }
            if let Some(db) = p.damage_blocked {
                entry.blocked_sum += db;
                entry.blocked_any = true;
            }
            if let Some(gd) = p.guarded_damage {
                entry.guarded_dmg_sum += gd;
                entry.guarded_any = true;
            }
            if let Some(gh) = p.guarded_hits {
                entry.guarded_hits_sum += gh;
                entry.guarded_any = true;
            }
        }
    }

    let mut roster: Vec<ParticipantRecord> = by_key
        .into_values()
        .map(|mut a| {
            if a.rec.is_local {
                let confident = a.self_computed
                    + a.self_reconciled
                    + a.self_server_reconciled
                    + a.self_estimated;
                let total = a.self_pending + a.self_partial + confident;
                a.rec.provenance = if a.self_partial > 0 || (a.self_pending > 0 && confident > 0) {
                    DamageProvenance::SelfPartial
                } else if a.self_estimated > 0 {
                    DamageProvenance::SelfEstimated
                } else if total > 0 && a.self_pending == total {
                    DamageProvenance::SelfPending
                } else if a.self_server_reconciled > 0 {
                    DamageProvenance::SelfServerReconciled
                } else if a.self_reconciled > 0 {
                    DamageProvenance::SelfReconciled
                } else {
                    DamageProvenance::SelfComputed
                };
            }
            if a.taken_any {
                a.rec.damage_taken = Some(a.taken_sum);
                a.rec.damage_taken_provenance = if a.taken_partial {
                    DamageTakenProvenance::Partial
                } else if a.taken_estimated {
                    DamageTakenProvenance::Estimated
                } else {
                    DamageTakenProvenance::Observed
                };
            } else {
                a.rec.damage_taken = None;
            }
            a.rec.damage_blocked = if a.blocked_any {
                Some(a.blocked_sum)
            } else {
                None
            };
            if a.guarded_any {
                a.rec.guarded_damage = Some(a.guarded_dmg_sum);
                a.rec.guarded_hits = Some(a.guarded_hits_sum);
            }
            a.rec
        })
        .collect();
    roster.sort_by(|a, b| b.damage.cmp(&a.damage).then(b.hits.cmp(&a.hits)));
    roster
}

/// Whether an encounter phase is an "add-on" rather than a big boss: an
/// aggregated aux summary row (minions/keys/segments) or a treasure crate. These
/// overlap the main boss fight in wall-clock, so they are excluded from active
/// combat time.
fn is_addon_phase(p: &FightRecord) -> bool {
    crate::assets::aux_target_for_type(p.boss_object_type).is_some()
        || crate::assets::is_treasure_crate_type(p.boss_object_type)
}

/// Collapse phases that map to the same aggregated aux target (e.g. every Royal
/// Crab of a Crab Sovereign fight) into a single merged phase row, summing their
/// HP and per-participant damage. New fights already aggregate such adds into
/// one summary row at write time; this makes historical runs -- recorded before
/// the add was aggregated -- display the same way. Non-aux phases and
/// single-instance aux phases are left untouched; first-appearance order holds.
fn collapse_aux_phases(phases: &[FightRecord]) -> Vec<FightRecord> {
    use std::collections::{HashMap, HashSet};

    let mut counts: HashMap<&'static str, usize> = HashMap::new();
    for p in phases {
        if let Some(aux) = crate::assets::aux_target_for_type(p.boss_object_type) {
            *counts.entry(aux.category).or_default() += 1;
        }
    }

    let mut emitted: HashSet<&'static str> = HashSet::new();
    let mut out: Vec<FightRecord> = Vec::with_capacity(phases.len());
    for p in phases {
        let Some(aux) = crate::assets::aux_target_for_type(p.boss_object_type) else {
            out.push(p.clone());
            continue;
        };
        if counts.get(aux.category).copied().unwrap_or(0) <= 1 {
            out.push(p.clone());
            continue;
        }
        if !emitted.insert(aux.category) {
            continue; // this category's merged row was already emitted
        }
        let group: Vec<FightRecord> = phases
            .iter()
            .filter(|q| {
                crate::assets::aux_target_for_type(q.boss_object_type)
                    .is_some_and(|a| a.category == aux.category)
            })
            .cloned()
            .collect();
        let started_at = group
            .iter()
            .map(|g| g.started_at)
            .min()
            .unwrap_or(p.started_at);
        let ended_at = group.iter().map(|g| g.ended_at).max().unwrap_or(p.ended_at);
        let sum_hp = |f: &dyn Fn(&FightRecord) -> i32| {
            group
                .iter()
                .map(|g| f(g) as i64)
                .sum::<i64>()
                .min(i32::MAX as i64) as i32
        };
        out.push(FightRecord {
            id: p.id,
            started_at,
            ended_at,
            dungeon: p.dungeon.clone(),
            dungeon_entered_at: group
                .iter()
                .filter_map(|g| g.dungeon_entered_at)
                .min()
                .or(p.dungeon_entered_at),
            map_seed: p.map_seed,
            boss_object_type: aux.repr_type,
            boss_name: aux.display_name.to_string(),
            boss_max_hp: sum_hp(&|g| g.boss_max_hp),
            boss_start_hp: sum_hp(&|g| g.boss_start_hp),
            local_object_id: p.local_object_id,
            local_char_id: p.local_char_id,
            killed: group.iter().any(|g| g.killed),
            local_close_calls: group.iter().map(|g| g.local_close_calls).sum(),
            aux_member_count: if crate::assets::aux_category_hides_count(aux.category) {
                None
            } else {
                Some(group.iter().map(|g| g.aux_member_count.unwrap_or(0)).sum())
            },
            participants: aggregate_roster(&group),
        });
    }
    out
}

/// Fold duplicate phases of a dedup-prone boss (Prismimic mirror halves, a
/// Moonlight Village invulnerable-finish dancer/Umi) that share the same
/// `(boss_object_type, boss_name)` within one encounter run into a single row.
/// The combat tracker keys fights by live object id, so such a boss -- the game
/// re-spawns it as a fresh object or re-detects it while it lingers invulnerable
/// -- yields several records for one boss. Keying on the
/// name too preserves intentional same-type splits such as "First/Second
/// Coming", whose names differ by suffix. Genuinely multi-instance bosses
/// (Pentaract towers, Nest hornets) are not dedup-prone, so they stay separate.
/// First-appearance order holds; HP takes the group max (one boss, not a sum)
/// while damage rosters are aggregated. Applied after [`collapse_aux_phases`].
fn collapse_duplicate_boss_phases(phases: &[FightRecord]) -> Vec<FightRecord> {
    use std::collections::{HashMap, HashSet};

    let key = |p: &FightRecord| (p.boss_object_type, p.boss_name.clone());
    let mut counts: HashMap<(i32, String), usize> = HashMap::new();
    for p in phases {
        if crate::assets::is_dedup_prone_boss(p.boss_object_type) {
            *counts.entry(key(p)).or_default() += 1;
        }
    }

    let mut emitted: HashSet<(i32, String)> = HashSet::new();
    let mut out: Vec<FightRecord> = Vec::with_capacity(phases.len());
    for p in phases {
        if !crate::assets::is_dedup_prone_boss(p.boss_object_type) {
            out.push(p.clone());
            continue;
        }
        let k = key(p);
        if counts.get(&k).copied().unwrap_or(0) <= 1 {
            out.push(p.clone());
            continue;
        }
        if !emitted.insert(k.clone()) {
            continue; // this boss's merged row was already emitted
        }
        let group: Vec<FightRecord> = phases.iter().filter(|q| key(q) == k).cloned().collect();
        let started_at = group
            .iter()
            .map(|g| g.started_at)
            .min()
            .unwrap_or(p.started_at);
        let ended_at = group.iter().map(|g| g.ended_at).max().unwrap_or(p.ended_at);
        out.push(FightRecord {
            id: p.id,
            started_at,
            ended_at,
            dungeon: p.dungeon.clone(),
            dungeon_entered_at: group
                .iter()
                .filter_map(|g| g.dungeon_entered_at)
                .min()
                .or(p.dungeon_entered_at),
            map_seed: p.map_seed,
            boss_object_type: p.boss_object_type,
            boss_name: p.boss_name.clone(),
            boss_max_hp: group
                .iter()
                .map(|g| g.boss_max_hp)
                .max()
                .unwrap_or(p.boss_max_hp),
            boss_start_hp: group
                .iter()
                .map(|g| g.boss_start_hp)
                .max()
                .unwrap_or(p.boss_start_hp),
            local_object_id: p.local_object_id,
            local_char_id: p.local_char_id,
            killed: group.iter().any(|g| g.killed),
            local_close_calls: group.iter().map(|g| g.local_close_calls).sum(),
            aux_member_count: p.aux_member_count,
            participants: aggregate_roster(&group),
        });
    }
    out
}

/// Encounter-participant identity matching [`aggregate_roster`]: the local player
/// collapses to one key; remotes key by `(object_id, name)`.
fn participant_key(p: &ParticipantRecord) -> (bool, i32, String) {
    if p.is_local {
        (true, 0, String::new())
    } else {
        (false, p.object_id, p.name.clone())
    }
}

/// End-status of each participant in the encounter's anchor (main boss) phase,
/// combined across rows if the anchor spans several fights. Empty when there is
/// no resolved anchor, or when the anchor is itself an aux damage-summary row
/// (an all-aux encounter): trusting a late-finalizing aux phase could downgrade a
/// genuinely-present player, so callers fall back to the aggregated roster.
fn anchor_end_statuses(
    phases: &[FightRecord],
    anchor_object_type: i32,
) -> std::collections::HashMap<(bool, i32, String), ParticipantEndStatus> {
    let mut map = std::collections::HashMap::new();
    if anchor_object_type == 0 || crate::assets::aux_target_for_type(anchor_object_type).is_some() {
        return map;
    }
    for phase in phases
        .iter()
        .filter(|p| p.boss_object_type == anchor_object_type)
    {
        for p in &phase.participants {
            map.entry(participant_key(p))
                .and_modify(|s| *s = combine_end_status(*s, p.end_status))
                .or_insert(p.end_status);
        }
    }
    map
}

/// Correct spurious per-phase `Nexused` flags on **concurrent aux/side phases**
/// so they reflect the main boss's real fate. Side phases such as
/// the O3 "Messengers" finalize by timeout ~15s after the boss dies, by which
/// point still-present players have teleported/dispersed and are wrongly flagged
/// Nexused. For an aux participant flagged Nexused we adopt the anchor (main
/// boss) fate when the anchor recorded them.
///
/// This is deliberately narrow:
/// - Only aux phases are touched. Sequential real-boss/miniboss phases (e.g. a
///   Chancellor Dammah fought before Oryx) keep their own per-phase fate, so a
///   player who completed the miniboss and only later nexused/died at the boss
///   still reads Present for the miniboss.
/// - Only a raw `Nexused` is ever rewritten; a genuine Present/Died is never
///   downgraded. Participants absent from the anchor keep their raw status.
fn unify_phase_end_statuses(
    phases: &mut [FightRecord],
    anchor_statuses: &std::collections::HashMap<(bool, i32, String), ParticipantEndStatus>,
) {
    for phase in phases.iter_mut() {
        if crate::assets::aux_target_for_type(phase.boss_object_type).is_none() {
            continue;
        }
        for p in phase.participants.iter_mut() {
            if !matches!(p.end_status, ParticipantEndStatus::Nexused) {
                continue;
            }
            if let Some(status) = anchor_statuses.get(&participant_key(p)).copied() {
                p.end_status = status;
            }
        }
    }
}

/// Propagate a participant's death from a concurrent **aux / add-on** phase up
/// into the encounter's other phases (notably the main boss). A player who dies
/// while an add-on is up -- e.g. during a Nest boss's invulnerable Blue Killer
/// Bee Nest / Red Beehemoth add phase -- leaves the main boss's range
/// with no gravestone correlated to the boss fight, so the boss phase records
/// them as `Nexused`. Their death IS captured on the concurrent aux phase, so
/// once a `Died` is seen on any aux phase we mark that player `Died` on every
/// phase they appear in (death is terminal within a run).
///
/// Scope is deliberately aux-*sourced* only: a death at a sequential real boss
/// (e.g. Oryx fought after a completed Chancellor Dammah miniboss) must never
/// bleed backward into the earlier boss the player actually cleared alive.
fn propagate_aux_deaths(phases: &mut [FightRecord]) {
    use std::collections::HashMap;
    let mut deaths: HashMap<(bool, i32, String), ParticipantEndStatus> = HashMap::new();
    for phase in phases.iter() {
        if crate::assets::aux_target_for_type(phase.boss_object_type).is_none() {
            continue;
        }
        for p in &phase.participants {
            if matches!(p.end_status, ParticipantEndStatus::Died { .. }) {
                deaths
                    .entry(participant_key(p))
                    .and_modify(|s| *s = combine_end_status(*s, p.end_status))
                    .or_insert(p.end_status);
            }
        }
    }
    if deaths.is_empty() {
        return;
    }
    for phase in phases.iter_mut() {
        for p in phase.participants.iter_mut() {
            if matches!(p.end_status, ParticipantEndStatus::Died { .. }) {
                continue;
            }
            if let Some(status) = deaths.get(&participant_key(p)).copied() {
                p.end_status = status;
            }
        }
    }
}

/// Convenience: build participant records straight from engine participants.
impl From<&FightParticipant> for ParticipantRecord {
    fn from(p: &FightParticipant) -> Self {
        ParticipantRecord {
            object_id: p.object_id,
            object_type: p.object_type,
            skin_id: p.skin_id,
            tex1: p.tex1,
            tex2: p.tex2,
            name: p.name.clone(),
            equipment: p.equipment,
            equipment_enchants: p.equipment_enchants.clone(),
            damage: p.damage,
            hits: p.hits as i64,
            is_local: p.is_local,
            provenance: p.provenance,
            end_status: p.end_status,
            pet: p.pet.clone(),
            damage_taken: p.damage_taken,
            damage_taken_provenance: p.damage_taken_provenance,
            damage_blocked: p.damage_blocked,
            guarded_damage: p.guarded_damage,
            guarded_hits: p.guarded_hits.map(|h| h as i64),
        }
    }
}

/// A minimal phase view used by the encounter anchor/headline helpers:
/// `(object_type, max_hp, start_hp, killed, ended_at)`.
type PhaseStat = (i32, i32, i32, bool, i64);

/// Resolve whether a grouped run counts as killed. Encounters listed in
/// `encounter_completion_all` are killed only when EVERY listed member was killed
/// (e.g. the Tomb of the Ancients' three main bosses); those in
/// `encounter_completion_any` are killed when ANY of their listed members was
/// killed (e.g. a Killer Bee Nest Beehemoth, a Pentaract Tower); everything else
/// defers to the anchor phase's own killed flag.
fn run_killed(enc_id: Option<&str>, phases: &[PhaseStat], anchor_killed: bool) -> bool {
    if let Some(types) = enc_id.and_then(crate::assets::encounter_completion_all) {
        return types
            .iter()
            .all(|t| phases.iter().any(|p| p.0 == *t && p.3));
    }
    match enc_id.and_then(crate::assets::encounter_completion_any) {
        Some(types) => phases.iter().any(|p| p.3 && types.contains(&p.0)),
        None => anchor_killed,
    }
}

/// Pick the anchor phase for a grouped run: the encounter's declared
/// `anchor_type` when present, else the latest-ended real (non-aux) boss (tie:
/// higher max HP), else any aux row. Returns `(object_type, max_hp, start_hp,
/// killed)` with `killed` resolved via [`run_killed`].
fn pick_anchor(phases: &[PhaseStat]) -> Option<(i32, i32, i32, bool)> {
    if phases.is_empty() {
        return None;
    }
    let enc = phases
        .iter()
        .find_map(|p| crate::assets::encounter_for_boss_type(p.0));
    let enc_id = enc.map(|e| e.id);
    // Only honor the declared anchor when every *real* boss phase belongs to the
    // same encounter. Aux damage-summary rows and treasure crates don't count as
    // separate bosses. A dungeon instance that also killed a standalone boss
    // outside the encounter (e.g. Forax's curated "Waste" miniboss alongside the
    // final boss Acidus) must instead fall through to the latest-ended boss so
    // the card headlines on the real final boss rather than the miniboss.
    let single_encounter = enc_id.is_some_and(|id| {
        phases.iter().all(|p| {
            if crate::assets::aux_target_for_type(p.0).is_some()
                || crate::assets::is_treasure_crate_type(p.0)
                || crate::assets::is_post_boss_bonus_type(p.0)
                || crate::assets::is_optional_secondary_boss_type(p.0)
            {
                return true;
            }
            crate::assets::encounter_for_boss_type(p.0).is_some_and(|e| e.id == id)
        })
    });
    if let Some(enc) = enc.filter(|_| single_encounter) {
        if let Some(p) = phases
            .iter()
            .filter(|p| p.0 == enc.anchor_type)
            .max_by(|a, b| a.4.cmp(&b.4))
        {
            return Some((p.0, p.1, p.2, run_killed(enc_id, phases, p.3)));
        }
    }
    // Anchor priority tiers: a real boss (3) outranks a Prismimic / optional
    // secondary boss (2) -- which follows or accompanies the real boss, so it
    // must not headline the run -- which outranks a treasure crate (1), which
    // outranks an aux damage-summary add (0). This keeps the dungeon card
    // headlined by its actual boss even when a loot crate was opened or a
    // Prismimic / side boss appeared (and thus "ended") after the boss died.
    // Within the top tier, pick the latest-ended row (tie: higher max HP).
    fn anchor_tier(otype: i32) -> u8 {
        if crate::assets::aux_target_for_type(otype).is_some() {
            0
        } else if crate::assets::is_treasure_crate_type(otype) {
            1
        } else if crate::assets::is_post_boss_bonus_type(otype)
            || crate::assets::is_optional_secondary_boss_type(otype)
        {
            2
        } else {
            3
        }
    }
    let top_tier = phases.iter().map(|p| anchor_tier(p.0)).max().unwrap_or(0);
    let best = phases
        .iter()
        .filter(|p| anchor_tier(p.0) == top_tier)
        .max_by(|a, b| a.4.cmp(&b.4).then(a.1.cmp(&b.1)))?;
    Some((best.0, best.1, best.2, run_killed(enc_id, phases, best.3)))
}

/// Display name for a boss phase: restores the Prismimic Attacker/Defender
/// distinction (both share the game DisplayId "Prismimic") so the two mirror
/// rows read clearly; every other boss keeps its stored name.
fn display_boss_name(object_type: i32, stored: &str) -> String {
    crate::assets::prismimic_display_name(object_type)
        .map(str::to_string)
        .unwrap_or_else(|| stored.to_string())
}

/// Strip the Marble Colossus survival-phase suffix so the summary list card and
/// the boss search suggestion/chip show a single unified "Marble Colossus". The
/// suffix is retained in the per-phase DPS breakdown (player roster view).
pub(crate) fn strip_boss_phase_suffix(name: &str) -> &str {
    name.strip_suffix(" (Pre-survival)")
        .or_else(|| name.strip_suffix(" (Post-survival)"))
        .unwrap_or(name)
}

/// Canonical headline for a realm-grouped encounter that occurs outside a
/// groupable dungeon (i.e. in the open Realm). Returns the encounter's display
/// name so the card is titled by the boss instead of the map name ("Realm").
/// Dungeon runs return `None` and keep their dungeon headline.
fn realm_headline(dungeon: &str, object_types: &[i32]) -> Option<&'static str> {
    if super::tracker::is_groupable_dungeon(dungeon) {
        return None;
    }
    let enc = object_types
        .iter()
        .find_map(|&t| crate::assets::encounter_for_boss_type(t))?;
    crate::assets::encounter_realm_grouped(enc.id).then_some(enc.display_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::types::{
        CompletedFight, DamageProvenance, FightParticipant, ParticipantEndStatus,
    };

    #[test]
    fn combine_end_status_is_died_over_present_over_nexused() {
        use ParticipantEndStatus::*;
        // Present at any phase end beats a nexused flag from another phase
        // (order-independent).
        assert_eq!(combine_end_status(Present, Nexused), Present);
        assert_eq!(combine_end_status(Nexused, Present), Present);
        // A correlated grave (Died) always wins over present/nexused.
        assert_eq!(
            combine_end_status(Present, Died { grave_type: 1830 }),
            Died { grave_type: 1830 }
        );
        assert_eq!(
            combine_end_status(Died { grave_type: 1830 }, Present),
            Died { grave_type: 1830 }
        );
        assert_eq!(
            combine_end_status(Nexused, Died { grave_type: 1837 }),
            Died { grave_type: 1837 }
        );
        // Never present anywhere -> stays nexused.
        assert_eq!(combine_end_status(Nexused, Nexused), Nexused);
    }

    fn sample_fight() -> CompletedFight {
        CompletedFight {
            started_at: 1000,
            ended_at: 61_000,
            dungeon: "Mad Lab".to_string(),
            dungeon_entered_at: None,
            map_seed: 42,
            boss_object_type: 0x10E4,
            boss_name: "Dr Terrible".to_string(),
            boss_max_hp: 300_000,
            boss_start_hp: 300_000,
            local_object_id: 1000,
            local_char_id: 777,
            killed: true,
            reached_zero: true,
            joined_late: false,
            encounter_id: None,
            encounter_run_id: None,
            local_close_calls: 0,
            aux_member_count: None,
            participants: vec![
                FightParticipant {
                    object_id: 600,
                    object_type: 0x0321,
                    skin_id: 5678,
                    tex1: 0x0100_00ff,
                    tex2: 0x0100_ff00,
                    name: "Alice".to_string(),
                    equipment: [1, 2, 3, 4],
                    equipment_enchants: [vec![10], vec![], vec![20, 21], vec![]],
                    damage: 200_000,
                    hits: 300,
                    is_local: false,
                    provenance: DamageProvenance::Observed,
                    end_status: ParticipantEndStatus::Died { grave_type: 1830 },
                    pet: Some(PetInfo {
                        pet_type: 1234,
                        abilities: vec![404, 407],
                    }),
                    damage_taken: Some(5_000),
                    damage_taken_provenance: DamageTakenProvenance::Observed,
                    damage_blocked: None,
                    guarded_damage: None,
                    guarded_hits: None,
                },
                FightParticipant {
                    object_id: 1000,
                    object_type: 0x0321,
                    skin_id: 0,
                    tex1: 0,
                    tex2: 0,
                    name: "You".to_string(),
                    equipment: [5, 6, 7, 8],
                    equipment_enchants: Default::default(),
                    damage: 0,
                    hits: 120,
                    is_local: true,
                    provenance: DamageProvenance::SelfPending,
                    end_status: ParticipantEndStatus::Nexused,
                    pet: None,
                    damage_taken: Some(1_200),
                    damage_taken_provenance: DamageTakenProvenance::Estimated,
                    damage_blocked: None,
                    guarded_damage: None,
                    guarded_hits: None,
                },
            ],
        }
    }

    fn flawless_part(
        object_id: i32,
        name: &str,
        damage: i64,
        damage_taken: Option<i64>,
        end_status: ParticipantEndStatus,
    ) -> FightParticipant {
        FightParticipant {
            object_id,
            object_type: 0x0321,
            skin_id: 0,
            tex1: 0,
            tex2: 0,
            name: name.to_string(),
            equipment: [-1; 4],
            equipment_enchants: Default::default(),
            damage,
            hits: 100,
            is_local: false,
            provenance: DamageProvenance::Observed,
            end_status,
            pet: None,
            damage_taken,
            damage_taken_provenance: DamageTakenProvenance::Observed,
            damage_blocked: None,
            guarded_damage: None,
            guarded_hits: None,
        }
    }

    fn flawless_fight(
        dungeon: &str,
        boss_type: i32,
        participants: Vec<FightParticipant>,
    ) -> CompletedFight {
        CompletedFight {
            started_at: 1000,
            ended_at: 61_000,
            dungeon: dungeon.to_string(),
            dungeon_entered_at: None,
            map_seed: 1,
            boss_object_type: boss_type,
            boss_name: String::new(),
            boss_max_hp: 100_000,
            boss_start_hp: 100_000,
            local_object_id: 10,
            local_char_id: 1,
            killed: true,
            reached_zero: true,
            joined_late: false,
            encounter_id: None,
            encounter_run_id: None,
            local_close_calls: 0,
            aux_member_count: None,
            participants,
        }
    }

    #[test]
    fn writer_enables_wal_busy_timeout_and_user_version() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("combat_history.db");
        let db = CombatDatabase::open_writer(&path).unwrap();

        let journal_mode: String = db
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
        let busy_timeout: i64 = db
            .conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        assert_eq!(busy_timeout, 5000);
        let version: i32 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn database_size_is_explicit_and_path_based() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("combat_history.db");
        assert_eq!(CombatDatabase::database_size_bytes(&path), 0);

        let mut db = CombatDatabase::open_writer(&path).unwrap();
        db.insert_fight(&sample_fight()).unwrap();
        assert!(CombatDatabase::database_size_bytes(&path) > 0);
    }

    #[test]
    fn reader_is_read_only_at_explicit_path() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("combat_history.db");
        // The writer creates the schema before the UI reader opens.
        let _writer = CombatDatabase::open_writer(&path).unwrap();
        let reader = CombatDatabase::open_reader_at(&path).unwrap();
        assert!(reader
            .conn
            .execute("CREATE TABLE should_fail (x INTEGER)", [])
            .is_err());
    }

    #[test]
    fn two_combat_profiles_isolate_identical_fight_identifiers() {
        let temp = tempfile::tempdir().unwrap();
        let a_path = temp.path().join("a").join("combat_history.db");
        let b_path = temp.path().join("b").join("combat_history.db");
        let mut a = CombatDatabase::open_writer(&a_path).unwrap();
        let mut b = CombatDatabase::open_writer(&b_path).unwrap();

        // Identical fight identifiers (map_seed / timestamps), distinct data.
        let mut fight_a = sample_fight();
        fight_a.boss_name = "Profile A Boss".to_string();
        let mut fight_b = sample_fight();
        fight_b.boss_name = "Profile B Boss".to_string();
        let id_a = a.insert_fight(&fight_a).unwrap();
        let id_b = b.insert_fight(&fight_b).unwrap();
        assert_eq!(id_a, id_b, "each profile starts its own fight id space");

        assert_eq!(a.fight_count().unwrap(), 1);
        assert_eq!(b.fight_count().unwrap(), 1);
        let list_a = a.list_fights(&FightQuery::default(), 10).unwrap();
        let list_b = b.list_fights(&FightQuery::default(), 10).unwrap();
        assert_eq!(list_a[0].boss_name, "Profile A Boss");
        assert_eq!(list_b[0].boss_name, "Profile B Boss");
    }

    #[test]
    fn flawless_marker_set_when_finisher_takes_no_damage() {
        // Fungal Cavern (Crystal Worm Mother) is an Exaltation dungeon boss.
        let mut db = CombatDatabase::open_in_memory().unwrap();
        db.insert_fight(&flawless_fight(
            "Fungal Cavern",
            45712,
            vec![
                // Present, 40k damage (>= 5% of 100k), no observed damage taken.
                flawless_part(600, "Alice", 40_000, None, ParticipantEndStatus::Present),
                // Present and high damage, but took damage -> not itself flawless.
                flawless_part(
                    601,
                    "Bob",
                    40_000,
                    Some(3_000),
                    ParticipantEndStatus::Present,
                ),
            ],
        ))
        .unwrap();
        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 1);
        assert!(
            list[0].flawless,
            "Alice qualifies: heavy damage, zero taken"
        );
    }

    #[test]
    fn flawless_marker_exempts_exaltation_mini_bosses() {
        // Agonized Titan (0xb010) and Kogbold Flying Machine (0xc4ad) sit in
        // Exaltation dungeons but must never earn the flawless marker, even for
        // a roster that would otherwise qualify.
        let mut db = CombatDatabase::open_in_memory().unwrap();
        db.insert_fight(&flawless_fight(
            "Lost Halls",
            0xb010,
            vec![
                flawless_part(600, "Alice", 40_000, None, ParticipantEndStatus::Present),
                flawless_part(
                    601,
                    "Bob",
                    40_000,
                    Some(3_000),
                    ParticipantEndStatus::Present,
                ),
            ],
        ))
        .unwrap();
        db.insert_fight(&flawless_fight(
            "Kogbold Steamworks",
            0xc4ad,
            vec![
                flawless_part(700, "Cara", 40_000, None, ParticipantEndStatus::Present),
                flawless_part(
                    701,
                    "Dan",
                    40_000,
                    Some(3_000),
                    ParticipantEndStatus::Present,
                ),
            ],
        ))
        .unwrap();
        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 2);
        assert!(
            list.iter().all(|s| !s.flawless),
            "exempt mini-bosses must never be flawless"
        );
    }

    fn secret_local_part(damage: i64, end_status: ParticipantEndStatus) -> FightParticipant {
        FightParticipant {
            object_id: 10,
            object_type: 0x0321,
            skin_id: 0,
            tex1: 0,
            tex2: 0,
            name: "You".to_string(),
            equipment: [-1; 4],
            equipment_enchants: Default::default(),
            damage,
            hits: 100,
            is_local: true,
            provenance: DamageProvenance::Observed,
            end_status,
            pet: None,
            damage_taken: None,
            damage_taken_provenance: DamageTakenProvenance::Observed,
            damage_blocked: None,
            guarded_damage: None,
            guarded_hits: None,
        }
    }

    #[test]
    fn lone_fighter_set_when_local_is_sole_damage_dealer() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        db.insert_fight(&flawless_fight(
            "Fungal Cavern",
            45712,
            vec![
                secret_local_part(80_000, ParticipantEndStatus::Present),
                // Other player present but dealt zero damage -> still solo.
                flawless_part(600, "Alice", 0, None, ParticipantEndStatus::Present),
            ],
        ))
        .unwrap();
        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 1);
        assert!(
            list[0].lone_fighter,
            "local sole damage dealer -> lone fighter"
        );
        assert!(
            !list[0].last_hero_standing,
            "no dead/nexused other -> not last hero"
        );
    }

    #[test]
    fn lone_fighter_not_set_when_other_player_dealt_damage() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        db.insert_fight(&flawless_fight(
            "Fungal Cavern",
            45712,
            vec![
                secret_local_part(80_000, ParticipantEndStatus::Present),
                flawless_part(
                    600,
                    "Alice",
                    5_000,
                    None,
                    ParticipantEndStatus::Died { grave_type: 1830 },
                ),
            ],
        ))
        .unwrap();
        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 1);
        assert!(
            !list[0].lone_fighter,
            "another damage dealer disqualifies solo"
        );
    }

    #[test]
    fn unattributed_damage_does_not_disqualify_solo() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // Legacy card with anonymous ("Unknown"/unresolved) leftover damage but no
        // detected other player: still counts as a solo finish.
        let mut unknown = flawless_part(0, "Unknown", 12_000, None, ParticipantEndStatus::Present);
        unknown.provenance = DamageProvenance::Unresolved;
        db.insert_fight(&flawless_fight(
            "Fungal Cavern",
            45712,
            vec![
                secret_local_part(80_000, ParticipantEndStatus::Present),
                unknown,
            ],
        ))
        .unwrap();
        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 1);
        assert!(
            list[0].lone_fighter,
            "unattributed damage must not break solo"
        );
        assert!(
            !list[0].last_hero_standing,
            "no detected dead/nexused other"
        );
    }

    #[test]
    fn last_hero_set_when_all_others_died_or_nexused() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        db.insert_fight(&flawless_fight(
            "Fungal Cavern",
            45712,
            vec![
                secret_local_part(50_000, ParticipantEndStatus::Present),
                flawless_part(
                    600,
                    "Alice",
                    30_000,
                    None,
                    ParticipantEndStatus::Died { grave_type: 1830 },
                ),
                flawless_part(601, "Bob", 20_000, None, ParticipantEndStatus::Nexused),
            ],
        ))
        .unwrap();
        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].last_hero_standing, "sole survivor -> last hero");
        assert!(!list[0].lone_fighter, "others dealt damage -> not lone");
    }

    #[test]
    fn last_hero_not_set_when_another_survived() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        db.insert_fight(&flawless_fight(
            "Fungal Cavern",
            45712,
            vec![
                secret_local_part(50_000, ParticipantEndStatus::Present),
                flawless_part(600, "Alice", 30_000, None, ParticipantEndStatus::Present),
            ],
        ))
        .unwrap();
        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 1);
        assert!(
            !list[0].last_hero_standing,
            "another survivor -> not last hero"
        );
    }

    #[test]
    fn secret_stats_only_for_exaltation_group() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // Non-exaltation dungeon: even a perfect solo does not qualify.
        db.insert_fight(&flawless_fight(
            "Snake Pit",
            9,
            vec![secret_local_part(80_000, ParticipantEndStatus::Present)],
        ))
        .unwrap();
        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 1);
        assert!(!list[0].lone_fighter);
        assert!(!list[0].last_hero_standing);
        assert!(!list[0].most_damage_taken);
    }

    #[test]
    fn most_damage_taken_set_when_local_leads_team() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let mut me = secret_local_part(50_000, ParticipantEndStatus::Present);
        me.damage_taken = Some(9_000);
        db.insert_fight(&flawless_fight(
            "Fungal Cavern",
            45712,
            vec![
                me,
                flawless_part(
                    600,
                    "Alice",
                    30_000,
                    Some(4_000),
                    ParticipantEndStatus::Present,
                ),
                flawless_part(
                    601,
                    "Bob",
                    20_000,
                    Some(1_000),
                    ParticipantEndStatus::Present,
                ),
            ],
        ))
        .unwrap();
        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 1);
        assert!(
            list[0].most_damage_taken,
            "local took strictly the most damage"
        );
    }

    #[test]
    fn most_damage_taken_not_set_when_teammate_took_more() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let mut me = secret_local_part(50_000, ParticipantEndStatus::Present);
        me.damage_taken = Some(3_000);
        db.insert_fight(&flawless_fight(
            "Fungal Cavern",
            45712,
            vec![
                me,
                flawless_part(
                    600,
                    "Alice",
                    30_000,
                    Some(8_000),
                    ParticipantEndStatus::Present,
                ),
            ],
        ))
        .unwrap();
        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 1);
        assert!(
            !list[0].most_damage_taken,
            "a teammate took more -> no award"
        );
    }

    #[test]
    fn most_damage_taken_not_set_for_solo_card() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let mut me = secret_local_part(80_000, ParticipantEndStatus::Present);
        me.damage_taken = Some(9_000);
        db.insert_fight(&flawless_fight("Fungal Cavern", 45712, vec![me]))
            .unwrap();
        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 1);
        assert!(
            !list[0].most_damage_taken,
            "no team -> no most-damage-taken award"
        );
    }

    #[test]
    fn reconcile_stat_awards_is_idempotent() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        db.insert_fight(&flawless_fight(
            "Fungal Cavern",
            45712,
            vec![
                secret_local_part(80_000, ParticipantEndStatus::Present),
                flawless_part(600, "Alice", 0, None, ParticipantEndStatus::Present),
            ],
        ))
        .unwrap();
        let first = db.reconcile_stat_awards().unwrap();
        assert_eq!(
            first.get(&1).copied(),
            Some((1, 0, 0)),
            "one lone-fighter award for char 1"
        );
        // Totals are authoritative and self-healing: re-running yields the same
        // absolute totals (no double count).
        let second = db.reconcile_stat_awards().unwrap();
        assert_eq!(
            second.get(&1).copied(),
            Some((1, 0, 0)),
            "totals stay stable across runs"
        );
    }

    #[test]
    fn targeted_stat_awards_reconcile_only_selected_card() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let selected_id = db
            .insert_fight(&flawless_fight(
                "Fungal Cavern",
                45712,
                vec![
                    secret_local_part(80_000, ParticipantEndStatus::Present),
                    flawless_part(600, "Alice", 0, None, ParticipantEndStatus::Present),
                ],
            ))
            .unwrap();
        let unrelated_id = db
            .insert_fight(&flawless_fight(
                "Fungal Cavern",
                45712,
                vec![secret_local_part(80_000, ParticipantEndStatus::Present)],
            ))
            .unwrap();

        let totals = db
            .reconcile_stat_awards_for_selections(&[FightSelection::Single(selected_id)])
            .unwrap();

        assert_eq!(totals.get(&1).copied(), Some((1, 0, 0)));
        let ledger_count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM stat_awards", [], |row| row.get(0))
            .unwrap();
        assert_eq!(ledger_count, 1);
        let unrelated_key = format!("S{unrelated_id}");
        let unrelated_count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM stat_awards WHERE card_key = ?1",
                params![unrelated_key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unrelated_count, 0);
    }

    #[test]
    fn targeted_stat_awards_correct_late_group_disqualifier() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let run_id = "late-disqualifier".to_string();
        let mut boss = flawless_fight(
            "Fungal Cavern",
            45712,
            vec![
                secret_local_part(80_000, ParticipantEndStatus::Present),
                flawless_part(600, "Alice", 0, None, ParticipantEndStatus::Present),
            ],
        );
        boss.encounter_id = Some("test-run".to_string());
        boss.encounter_run_id = Some(run_id.clone());
        db.insert_fight(&boss).unwrap();

        let first = db
            .reconcile_stat_awards_for_selections(&[FightSelection::Encounter(run_id.clone())])
            .unwrap();
        assert_eq!(first.get(&1).copied(), Some((1, 0, 0)));

        let mut late = flawless_fight(
            "Fungal Cavern",
            45712,
            vec![flawless_part(
                601,
                "Bob",
                5_000,
                None,
                ParticipantEndStatus::Present,
            )],
        );
        late.encounter_id = Some("test-run".to_string());
        late.encounter_run_id = Some(run_id.clone());
        late.started_at += 1;
        late.ended_at += 1;
        db.insert_fight(&late).unwrap();

        let corrected = db
            .reconcile_stat_awards_for_selections(&[FightSelection::Encounter(run_id.clone())])
            .unwrap();
        assert_eq!(corrected.get(&1).copied(), Some((0, 0, 0)));
        let flags: (i64, i64, i64) = db
            .conn
            .query_row(
                "SELECT lone, last, most FROM stat_awards WHERE card_key = ?1",
                params![format!("E{run_id}")],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(flags, (0, 0, 0));
    }

    #[test]
    fn targeted_stat_awards_preserve_excluded_ledger_rows() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let id = db
            .insert_fight(&flawless_fight(
                "Fungal Cavern",
                45712,
                vec![secret_local_part(80_000, ParticipantEndStatus::Present)],
            ))
            .unwrap();
        let key = format!("S{id}");
        db.conn
            .execute(
                "INSERT INTO stat_awards (card_key, char_id, lone, last, most)
                 VALUES (?1, 1, 1, 1, 1)",
                params![&key],
            )
            .unwrap();
        db.conn
            .execute(
                "UPDATE fights SET boss_group = NULL WHERE id = ?1",
                params![id],
            )
            .unwrap();

        let totals = db
            .reconcile_stat_awards_for_selections(&[
                FightSelection::Single(id),
                FightSelection::Single(i64::MAX),
            ])
            .unwrap();

        assert!(totals.is_empty());
        let flags: (i64, i64, i64) = db
            .conn
            .query_row(
                "SELECT lone, last, most FROM stat_awards WHERE card_key = ?1",
                params![key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(flags, (1, 1, 1));
    }

    #[test]
    fn targeted_stat_awards_preserve_non_exaltation_ledger_row() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let id = db
            .insert_fight(&flawless_fight(
                "Snake Pit",
                9,
                vec![secret_local_part(80_000, ParticipantEndStatus::Present)],
            ))
            .unwrap();
        let key = format!("S{id}");
        db.conn
            .execute(
                "INSERT INTO stat_awards (card_key, char_id, lone, last, most)
                 VALUES (?1, 1, 1, 1, 1)",
                params![&key],
            )
            .unwrap();

        let totals = db
            .reconcile_stat_awards_for_selections(&[FightSelection::Single(id)])
            .unwrap();

        assert!(totals.is_empty());
        let flags: (i64, i64, i64) = db
            .conn
            .query_row(
                "SELECT lone, last, most FROM stat_awards WHERE card_key = ?1",
                params![key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(flags, (1, 1, 1));
    }

    #[test]
    fn targeted_stat_awards_preserve_zero_owner_ledger_row() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let id = db
            .insert_fight(&flawless_fight(
                "Fungal Cavern",
                45712,
                vec![secret_local_part(80_000, ParticipantEndStatus::Present)],
            ))
            .unwrap();
        let key = format!("S{id}");
        db.conn
            .execute(
                "INSERT INTO stat_awards (card_key, char_id, lone, last, most)
                 VALUES (?1, 1, 1, 1, 1)",
                params![&key],
            )
            .unwrap();
        db.conn
            .execute(
                "UPDATE fights SET local_char_id = 0 WHERE id = ?1",
                params![id],
            )
            .unwrap();

        let totals = db
            .reconcile_stat_awards_for_selections(&[FightSelection::Single(id)])
            .unwrap();

        assert!(totals.is_empty());
        let flags: (i64, i64, i64) = db
            .conn
            .query_row(
                "SELECT lone, last, most FROM stat_awards WHERE card_key = ?1",
                params![key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(flags, (1, 1, 1));
    }

    #[test]
    fn targeted_stat_awards_return_old_and_new_owner_totals() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let id = db
            .insert_fight(&flawless_fight(
                "Fungal Cavern",
                45712,
                vec![secret_local_part(80_000, ParticipantEndStatus::Present)],
            ))
            .unwrap();
        db.reconcile_stat_awards_for_selections(&[FightSelection::Single(id)])
            .unwrap();
        db.conn
            .execute(
                "UPDATE fights SET local_char_id = 2 WHERE id = ?1",
                params![id],
            )
            .unwrap();

        let totals = db
            .reconcile_stat_awards_for_selections(&[FightSelection::Single(id)])
            .unwrap();

        assert_eq!(totals.get(&1).copied(), Some((0, 0, 0)));
        assert_eq!(totals.get(&2).copied(), Some((1, 0, 0)));
    }

    #[test]
    fn full_stat_awards_return_zero_for_displaced_owner() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let id = db
            .insert_fight(&flawless_fight(
                "Fungal Cavern",
                45712,
                vec![secret_local_part(80_000, ParticipantEndStatus::Present)],
            ))
            .unwrap();
        let initial = db.reconcile_stat_awards().unwrap();
        assert_eq!(initial.get(&1).copied(), Some((1, 0, 0)));
        db.conn
            .execute(
                "UPDATE fights SET local_char_id = 2 WHERE id = ?1",
                params![id],
            )
            .unwrap();

        let corrected = db.reconcile_stat_awards().unwrap();

        assert_eq!(corrected.get(&1).copied(), Some((0, 0, 0)));
        assert_eq!(corrected.get(&2).copied(), Some((1, 0, 0)));
    }

    #[test]
    fn targeted_stat_awards_keep_secret_stat_group_guard() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let id = db
            .insert_fight(&flawless_fight(
                "Snake Pit",
                9,
                vec![secret_local_part(80_000, ParticipantEndStatus::Present)],
            ))
            .unwrap();
        db.conn
            .execute(
                "UPDATE fights SET boss_group = ?1 WHERE id = ?2",
                params![crate::assets::BossGroup::Exaltation.as_str(), id],
            )
            .unwrap();

        let totals = db
            .reconcile_stat_awards_for_selections(&[FightSelection::Single(id)])
            .unwrap();

        assert_eq!(totals.get(&1).copied(), Some((0, 0, 0)));
    }

    #[test]
    fn targeted_and_full_stat_awards_are_equivalent() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let first_id = db
            .insert_fight(&flawless_fight(
                "Fungal Cavern",
                45712,
                vec![
                    secret_local_part(80_000, ParticipantEndStatus::Present),
                    flawless_part(600, "Alice", 0, None, ParticipantEndStatus::Present),
                ],
            ))
            .unwrap();
        let mut second = flawless_fight(
            "Fungal Cavern",
            45712,
            vec![secret_local_part(80_000, ParticipantEndStatus::Present)],
        );
        second.local_char_id = 2;
        let second_id = db.insert_fight(&second).unwrap();

        let run_id = "equivalent-group".to_string();
        let mut grouped = flawless_fight(
            "Fungal Cavern",
            45712,
            vec![secret_local_part(80_000, ParticipantEndStatus::Present)],
        );
        grouped.encounter_id = Some("equivalent-group".to_string());
        grouped.encounter_run_id = Some(run_id.clone());
        db.insert_fight(&grouped).unwrap();

        let targeted = db
            .reconcile_stat_awards_for_selections(&[
                FightSelection::Single(first_id),
                FightSelection::Single(second_id),
                FightSelection::Encounter(run_id),
            ])
            .unwrap();
        let full = db.reconcile_stat_awards().unwrap();

        assert_eq!(targeted, full);
    }

    #[test]
    fn cross_card_completion_mutators_remain_non_exaltation() {
        for (dragon_type, _) in crate::assets::lod_dragon_chest_pairs() {
            assert_ne!(
                crate::assets::boss_group("Legacy Lair of Draconis", *dragon_type, ""),
                Some(crate::assets::BossGroup::Exaltation)
            );
        }
        assert_ne!(
            crate::assets::boss_group(
                "Legacy Lair of Draconis",
                crate::assets::LEGACY_LOD_IVORY_BOSS,
                ""
            ),
            Some(crate::assets::BossGroup::Exaltation)
        );
        assert_ne!(
            crate::assets::boss_group("Realm", 47909, "Towering Perfection"),
            Some(crate::assets::BossGroup::Exaltation)
        );
    }

    #[test]
    fn reconcile_close_calls_sums_per_character_from_cards() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let mut f1 = sample_fight();
        f1.local_char_id = 777;
        f1.local_close_calls = 2;
        f1.encounter_run_id = None;
        db.insert_fight(&f1).unwrap();

        let mut f2 = sample_fight();
        f2.local_char_id = 777;
        f2.local_close_calls = 3;
        db.insert_fight(&f2).unwrap();

        let mut f3 = sample_fight();
        f3.local_char_id = 888;
        f3.local_close_calls = 1;
        db.insert_fight(&f3).unwrap();

        // A row with no close calls contributes nothing and no phantom char id.
        let mut f4 = sample_fight();
        f4.local_char_id = 999;
        f4.local_close_calls = 0;
        db.insert_fight(&f4).unwrap();

        let totals = db.reconcile_close_calls().unwrap();
        assert_eq!(
            totals.get(&777).copied(),
            Some(5),
            "summed across the char's cards"
        );
        assert_eq!(totals.get(&888).copied(), Some(1));
        assert_eq!(totals.get(&999), None, "zero-close-call char is omitted");

        // Self-healing: re-running yields identical absolute totals.
        let again = db.reconcile_close_calls().unwrap();
        assert_eq!(again.get(&777).copied(), Some(5));
    }

    #[test]
    fn find_killed_fight_matches_within_window() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let id = db
            .insert_fight(&flawless_fight(
                "Fungal Cavern",
                45712,
                vec![flawless_part(
                    600,
                    "Alice",
                    40_000,
                    None,
                    ParticipantEndStatus::Present,
                )],
            ))
            .unwrap();

        // Drop right at the kill time correlates to the single fight card.
        let hit = db.find_killed_fight(1, 45712, 61_000).unwrap();
        assert_eq!(hit, Some(FightSelection::Single(id)));

        // Drop long after the kill still matches inside the trailing window.
        let late = db
            .find_killed_fight(1, 45712, 61_000 + LOOT_LINK_POST_MS)
            .unwrap();
        assert_eq!(late, Some(FightSelection::Single(id)));

        // Unknown instance, wrong mob, and out-of-window all miss.
        assert_eq!(db.find_killed_fight(0, 45712, 61_000).unwrap(), None);
        assert_eq!(db.find_killed_fight(1, 1, 61_000).unwrap(), None);
        assert_eq!(
            db.find_killed_fight(1, 45712, 61_000 + LOOT_LINK_POST_MS + 1)
                .unwrap(),
            None
        );
    }

    #[test]
    fn find_killed_fight_ignores_escaped_runs() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let mut escaped = flawless_fight(
            "Fungal Cavern",
            45712,
            vec![flawless_part(
                600,
                "Alice",
                40_000,
                None,
                ParticipantEndStatus::Present,
            )],
        );
        escaped.killed = false;
        db.insert_fight(&escaped).unwrap();
        assert_eq!(db.find_killed_fight(1, 45712, 61_000).unwrap(), None);
    }

    #[test]
    fn find_killed_fight_translates_lod_chest_to_dragon() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let id = db
            .insert_fight(&flawless_fight(
                "Legacy Lair of Draconis",
                30019, // Pyyr the Wicked
                vec![flawless_part(
                    600,
                    "Alice",
                    40_000,
                    None,
                    ParticipantEndStatus::Present,
                )],
            ))
            .unwrap();
        // A bag emitted by Pyyr's loot balloon chest (30034) resolves back to the
        // dragon's killed fight, not a (nonexistent) chest fight.
        let hit = db.find_killed_fight(1, 30034, 61_000).unwrap();
        assert_eq!(hit, Some(FightSelection::Single(id)));
    }

    #[test]
    fn lod_chest_loot_completes_matching_dragon_on_open() {
        let temp = tempfile::tempdir().unwrap();
        let combat_path = temp.path().join("combat_history.db");
        let loot_path = temp.path().join("loot_history.db");

        // A loot DB whose only drop is Pyyr's chest (30034) in instance seed 7.
        {
            let conn = Connection::open(&loot_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE loot_drops (map_seed INTEGER, mob_type INTEGER, timestamp INTEGER);
                 INSERT INTO loot_drops (map_seed, mob_type, timestamp) VALUES (7, 30034, 65000);",
            )
            .unwrap();
        }

        // A combat DB with an escaped Pyyr in the same instance/window, plus a
        // same-type escaped dragon in a different instance that must stay escaped.
        {
            let mut db = CombatDatabase::open_writer(&combat_path).unwrap();
            let mut hit = flawless_fight("Legacy Lair of Draconis", 30019, vec![]);
            hit.map_seed = 7;
            hit.killed = false;
            hit.reached_zero = false;
            db.insert_fight(&hit).unwrap();
            let mut other = flawless_fight("Legacy Lair of Draconis", 30019, vec![]);
            other.map_seed = 99;
            other.killed = false;
            other.reached_zero = false;
            db.insert_fight(&other).unwrap();
        }

        // Re-open paired with the loot DB: the backfill completes only the dragon
        // whose chest bag was recorded in the same instance.
        let db = CombatDatabase::open_writer_with_loot(&combat_path, Some(&loot_path)).unwrap();
        let killed_seed7: i64 = db
            .conn
            .query_row("SELECT killed FROM fights WHERE map_seed = 7", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(killed_seed7, 1, "chest loot completes the matching dragon");
        let killed_seed99: i64 = db
            .conn
            .query_row("SELECT killed FROM fights WHERE map_seed = 99", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(killed_seed99, 0, "a different instance is untouched");
    }

    #[test]
    fn lod_chest_loot_completes_only_the_nearest_dragon_fragment() {
        let temp = tempfile::tempdir().unwrap();
        let combat_path = temp.path().join("combat_history.db");
        let loot_path = temp.path().join("loot_history.db");

        // Pyyr's chest (30034) drops once, close to the second fragment's end.
        {
            let conn = Connection::open(&loot_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE loot_drops (map_seed INTEGER, mob_type INTEGER, timestamp INTEGER);
                 INSERT INTO loot_drops (map_seed, mob_type, timestamp) VALUES (7, 30034, 400000);",
            )
            .unwrap();
        }

        // Two escaped Pyyr fragments in the same instance (a reconnect artifact);
        // only the one whose end is nearest the chest drop should complete.
        let far_id;
        let near_id;
        {
            let mut db = CombatDatabase::open_writer(&combat_path).unwrap();
            let mut far = flawless_fight("Legacy Lair of Draconis", 30019, vec![]);
            far.map_seed = 7;
            far.killed = false;
            far.reached_zero = false;
            far.started_at = 1000;
            far.ended_at = 61_000;
            far_id = db.insert_fight(&far).unwrap();
            let mut near = flawless_fight("Legacy Lair of Draconis", 30019, vec![]);
            near.map_seed = 7;
            near.killed = false;
            near.reached_zero = false;
            near.started_at = 350_000;
            near.ended_at = 395_000;
            near_id = db.insert_fight(&near).unwrap();
        }

        let db = CombatDatabase::open_writer_with_loot(&combat_path, Some(&loot_path)).unwrap();
        let killed_far: i64 = db
            .conn
            .query_row("SELECT killed FROM fights WHERE id = ?1", [far_id], |r| {
                r.get(0)
            })
            .unwrap();
        let killed_near: i64 = db
            .conn
            .query_row("SELECT killed FROM fights WHERE id = ?1", [near_id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            killed_near, 1,
            "the fragment nearest the chest drop completes"
        );
        assert_eq!(killed_far, 0, "one chest drop completes only one fragment");
    }

    #[test]
    fn ivory_wyvern_completes_preceding_grouped_lair_run() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // Two escaped dragons sharing one grouped Lair run.
        let mut d1 = flawless_fight("Legacy Lair of Draconis", 30019, vec![]);
        d1.map_seed = 7;
        d1.local_char_id = 3;
        d1.killed = false;
        d1.reached_zero = false;
        d1.started_at = 1000;
        d1.ended_at = 60_000;
        d1.encounter_id = Some("dungeon_run".into());
        d1.encounter_run_id = Some("run-lair".into());
        let id1 = db.insert_fight(&d1).unwrap();
        let mut d2 = flawless_fight("Legacy Lair of Draconis", 29978, vec![]);
        d2.map_seed = 7;
        d2.local_char_id = 3;
        d2.killed = false;
        d2.reached_zero = false;
        d2.started_at = 60_000;
        d2.ended_at = 120_000;
        d2.encounter_id = Some("dungeon_run".into());
        d2.encounter_run_id = Some("run-lair".into());
        let id2 = db.insert_fight(&d2).unwrap();

        // The Ivory fight that follows proves all four dragons were defeated.
        let mut ivory = flawless_fight("The Ivory Wyvern", 30026, vec![]);
        ivory.map_seed = 8;
        ivory.local_char_id = 3;
        ivory.started_at = 150_000;
        ivory.ended_at = 200_000;
        db.insert_fight(&ivory).unwrap();

        for id in [id1, id2] {
            let killed: i64 = db
                .conn
                .query_row("SELECT killed FROM fights WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(killed, 1, "every recorded dragon in the run completes");
        }
    }

    #[test]
    fn ivory_backfill_completes_on_open_and_isolates_by_character() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("combat_history.db");
        {
            let mut db = CombatDatabase::open_writer(&path).unwrap();
            // Ivory recorded before its Lair dragon finalized: the live hook finds
            // nothing yet, so completion must come from the on-open backfill.
            let mut ivory = flawless_fight("The Ivory Wyvern", 30026, vec![]);
            ivory.map_seed = 8;
            ivory.local_char_id = 3;
            ivory.started_at = 150_000;
            ivory.ended_at = 200_000;
            db.insert_fight(&ivory).unwrap();
            let mut dragon = flawless_fight("Legacy Lair of Draconis", 30019, vec![]);
            dragon.map_seed = 7;
            dragon.local_char_id = 3;
            dragon.killed = false;
            dragon.reached_zero = false;
            dragon.started_at = 1000;
            dragon.ended_at = 60_000;
            db.insert_fight(&dragon).unwrap();
            // A different character's escaped Lair dragon with no Ivory follow-up.
            let mut other = flawless_fight("Legacy Lair of Draconis", 30019, vec![]);
            other.map_seed = 9;
            other.local_char_id = 4;
            other.killed = false;
            other.reached_zero = false;
            other.started_at = 1000;
            other.ended_at = 60_000;
            db.insert_fight(&other).unwrap();

            let live: i64 = db
                .conn
                .query_row("SELECT killed FROM fights WHERE map_seed = 7", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(
                live, 0,
                "live hook cannot complete a dragon inserted after its Ivory"
            );
        }
        let db = CombatDatabase::open_writer(&path).unwrap();
        let cleared: i64 = db
            .conn
            .query_row("SELECT killed FROM fights WHERE map_seed = 7", [], |r| {
                r.get(0)
            })
            .unwrap();
        let untouched: i64 = db
            .conn
            .query_row("SELECT killed FROM fights WHERE map_seed = 9", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            cleared, 1,
            "on-open backfill completes the character's Lair run"
        );
        assert_eq!(
            untouched, 0,
            "another character's Lair run is not completed"
        );
    }

    #[test]
    fn ivory_ignores_lair_run_outside_followup_window() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let mut stale = flawless_fight("Legacy Lair of Draconis", 30019, vec![]);
        stale.map_seed = 7;
        stale.local_char_id = 3;
        stale.killed = false;
        stale.reached_zero = false;
        stale.started_at = 1000;
        stale.ended_at = 100_000;
        db.insert_fight(&stale).unwrap();
        // An Ivory fight 25 minutes later is beyond the follow-up window.
        let mut ivory = flawless_fight("The Ivory Wyvern", 30026, vec![]);
        ivory.local_char_id = 3;
        ivory.started_at = 100_000 + 25 * 60 * 1000;
        ivory.ended_at = ivory.started_at + 40_000;
        db.insert_fight(&ivory).unwrap();
        let killed: i64 = db
            .conn
            .query_row("SELECT killed FROM fights WHERE map_seed = 7", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            killed, 0,
            "a distant Ivory fight does not complete an earlier Lair run"
        );
    }

    #[test]
    fn ivory_seed_fallback_ignores_recycled_seed_far_in_time() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // An old ungrouped instance reusing seed 7, long before the Ivory fight.
        let mut old = flawless_fight("Legacy Lair of Draconis", 30019, vec![]);
        old.map_seed = 7;
        old.local_char_id = 3;
        old.killed = false;
        old.reached_zero = false;
        old.started_at = 0;
        old.ended_at = 50_000;
        let old_id = db.insert_fight(&old).unwrap();
        // A recent ungrouped instance reusing seed 7, right before the Ivory fight.
        let mut recent = flawless_fight("Legacy Lair of Draconis", 29978, vec![]);
        recent.map_seed = 7;
        recent.local_char_id = 3;
        recent.killed = false;
        recent.reached_zero = false;
        recent.started_at = 10_000_000;
        recent.ended_at = 10_060_000;
        let recent_id = db.insert_fight(&recent).unwrap();
        let mut ivory = flawless_fight("The Ivory Wyvern", 30026, vec![]);
        ivory.local_char_id = 3;
        ivory.started_at = 10_100_000;
        ivory.ended_at = 10_140_000;
        db.insert_fight(&ivory).unwrap();

        let k_recent: i64 = db
            .conn
            .query_row(
                "SELECT killed FROM fights WHERE id = ?1",
                [recent_id],
                |r| r.get(0),
            )
            .unwrap();
        let k_old: i64 = db
            .conn
            .query_row("SELECT killed FROM fights WHERE id = ?1", [old_id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(k_recent, 1, "the instance the Ivory followed completes");
        assert_eq!(
            k_old, 0,
            "a far-earlier instance reusing the seed is untouched"
        );
    }

    #[test]
    fn ivory_skips_inference_for_unknown_character() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let mut dragon = flawless_fight("Legacy Lair of Draconis", 30019, vec![]);
        dragon.map_seed = 7;
        dragon.local_char_id = 0;
        dragon.killed = false;
        dragon.reached_zero = false;
        dragon.started_at = 1000;
        dragon.ended_at = 60_000;
        let id = db.insert_fight(&dragon).unwrap();
        let mut ivory = flawless_fight("The Ivory Wyvern", 30026, vec![]);
        ivory.local_char_id = 0;
        ivory.started_at = 150_000;
        ivory.ended_at = 200_000;
        db.insert_fight(&ivory).unwrap();
        let killed: i64 = db
            .conn
            .query_row("SELECT killed FROM fights WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            killed, 0,
            "unknown character identity blocks cross-player inference"
        );
    }

    #[test]
    fn lod_completion_noops_without_paired_loot_db() {
        let temp = tempfile::tempdir().unwrap();
        let combat_path = temp.path().join("combat_history.db");
        {
            let mut db = CombatDatabase::open_writer(&combat_path).unwrap();
            let mut dragon = flawless_fight("Legacy Lair of Draconis", 30019, vec![]);
            dragon.map_seed = 7;
            dragon.killed = false;
            dragon.reached_zero = false;
            db.insert_fight(&dragon).unwrap();
        }
        // A maintenance writer with no loot path leaves the escaped dragon as-is
        // so a later paired open can still repair it.
        let db = CombatDatabase::open_writer(&combat_path).unwrap();
        let killed: i64 = db
            .conn
            .query_row("SELECT killed FROM fights WHERE map_seed = 7", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            killed, 0,
            "loot-dependent completion is a no-op without a loot DB"
        );
    }

    #[test]
    fn loot_bag_latches_escaped_towering_perfection_run_completed() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // Two escaped segment fights (Lower + Upper Imperfection) grouped in one
        // Towering Perfection realm run; the core (47909) was never seen dying.
        for otype in [47916, 47917] {
            let mut seg = flawless_fight(
                "Realm",
                otype,
                vec![flawless_part(
                    600,
                    "Alice",
                    40_000,
                    None,
                    ParticipantEndStatus::Present,
                )],
            );
            seg.killed = false;
            seg.encounter_id = Some("towering_perfection".to_string());
            seg.encounter_run_id = Some("tp-run".to_string());
            db.insert_fight(&seg).unwrap();
        }
        let run_killed = |db: &CombatDatabase| -> i64 {
            db.conn
                .query_row(
                    "SELECT killed FROM encounter_runs WHERE run_id = 'tp-run'",
                    [],
                    |r| r.get(0),
                )
                .unwrap()
        };
        // Card view reflects the run status, not just the raw column.
        let card_killed = |db: &CombatDatabase| -> bool {
            let cards = db.list_fights(&FightQuery::default(), 50).unwrap();
            assert_eq!(cards.len(), 1, "the two segments fold into one card");
            cards[0].killed
        };
        assert_eq!(run_killed(&db), 0, "run starts escaped");
        assert!(!card_killed(&db), "card starts Escaped");

        // A bag from the core (same instance seed) latches the card Completed.
        db.mark_encounter_killed_by_loot(1, "towering_perfection")
            .unwrap();
        assert_eq!(run_killed(&db), 1, "core bag completes the run");
        assert!(card_killed(&db), "card now shows Completed");
        assert!(
            db.encounter_detail("tp-run").unwrap().unwrap().killed,
            "detail view shows Completed too",
        );

        // A bag from a different instance seed never touches this run.
        let mut db2 = CombatDatabase::open_in_memory().unwrap();
        let mut seg = flawless_fight("Realm", 47916, vec![]);
        seg.killed = false;
        seg.map_seed = 5;
        seg.encounter_id = Some("towering_perfection".to_string());
        seg.encounter_run_id = Some("tp-run-2".to_string());
        db2.insert_fight(&seg).unwrap();
        db2.mark_encounter_killed_by_loot(1, "towering_perfection")
            .unwrap();
        let killed: i64 = db2
            .conn
            .query_row(
                "SELECT killed FROM encounter_runs WHERE run_id = 'tp-run-2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(killed, 0, "wrong instance seed leaves the run escaped");
    }

    #[test]
    fn flawless_marker_clear_when_every_finisher_took_damage_or_left() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        db.insert_fight(&flawless_fight(
            "Fungal Cavern",
            45712,
            vec![
                // No damage taken but died -> did not finish.
                flawless_part(
                    600,
                    "Alice",
                    40_000,
                    None,
                    ParticipantEndStatus::Died { grave_type: 0 },
                ),
                // Finished with no damage taken but only trivial damage (< 5%).
                flawless_part(601, "Bob", 1_000, None, ParticipantEndStatus::Present),
                // Finished and heavy damage but took damage.
                flawless_part(
                    602,
                    "Carol",
                    40_000,
                    Some(10),
                    ParticipantEndStatus::Present,
                ),
            ],
        ))
        .unwrap();
        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 1);
        assert!(!list[0].flawless);
    }

    #[test]
    fn flawless_marker_skipped_for_legacy_rows_without_damage_taken() {
        // A qualifying Exaltation roster whose damage-taken is entirely NULL
        // (legacy, pre-feature) must not be read as a factual zero taken.
        let mut db = CombatDatabase::open_in_memory().unwrap();
        db.insert_fight(&flawless_fight(
            "Fungal Cavern",
            45712,
            vec![
                flawless_part(600, "Alice", 40_000, None, ParticipantEndStatus::Present),
                flawless_part(601, "Bob", 40_000, None, ParticipantEndStatus::Present),
            ],
        ))
        .unwrap();
        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 1);
        assert!(
            !list[0].flawless,
            "legacy all-NULL taken must not flag flawless"
        );
    }

    #[test]
    fn flawless_marker_only_for_exaltation_group() {
        // Same qualifying roster in a non-Exaltation dungeon stays unmarked.
        let mut db = CombatDatabase::open_in_memory().unwrap();
        db.insert_fight(&flawless_fight(
            "Nowhere Test Map",
            999_999,
            vec![flawless_part(
                600,
                "Alice",
                40_000,
                None,
                ParticipantEndStatus::Present,
            )],
        ))
        .unwrap();
        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 1);
        assert!(
            !list[0].flawless,
            "non-Exaltation cards never get the marker"
        );
    }

    #[test]
    fn insert_and_read_back() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let id = db.insert_fight(&sample_fight()).unwrap();
        assert!(id > 0);
        assert_eq!(db.fight_count().unwrap(), 1);

        let fights = db.recent_fights(10).unwrap();
        assert_eq!(fights.len(), 1);
        let f = &fights[0];
        assert_eq!(f.dungeon, "Mad Lab");
        assert_eq!(f.boss_name, "Dr Terrible");
        assert!(f.killed);
        assert_eq!(f.duration_ms(), 60_000);
        assert_eq!(f.participants.len(), 2);
        // Sorted by damage descending.
        assert_eq!(f.participants[0].name, "Alice");
        assert_eq!(f.participants[0].damage, 200_000);
        // Per-slot equipment enchants round-trip.
        assert_eq!(f.participants[0].equipment_enchants[0], vec![10]);
        assert_eq!(f.participants[0].equipment_enchants[1], Vec::<u16>::new());
        assert_eq!(f.participants[0].equipment_enchants[2], vec![20, 21]);
        // Skin id and dye textures round-trip.
        assert_eq!(f.participants[0].skin_id, 5678);
        assert_eq!(f.participants[0].tex1, 0x0100_00ff);
        assert_eq!(f.participants[0].tex2, 0x0100_ff00);
        // Pet round-trips; the local player has none.
        let pet = f.participants[0].pet.as_ref().expect("Alice pet persisted");
        assert_eq!(pet.pet_type, 1234);
        assert_eq!(pet.abilities, vec![404, 407]);
        assert!(f.participants[1].pet.is_none());
        assert_eq!(f.participants[1].skin_id, 0);
        assert_eq!(f.participants[1].tex1, 0);
        assert_eq!(
            f.participants[1].equipment_enchants,
            <[Vec<u16>; 4]>::default()
        );
        assert_eq!(f.participants[1].provenance, DamageProvenance::SelfPending);
        assert!(f.participants[1].is_local);
        // End status (death / nexus) round-trips, including the grave type.
        assert_eq!(
            f.participants[0].end_status,
            ParticipantEndStatus::Died { grave_type: 1830 }
        );
        assert_eq!(f.participants[1].end_status, ParticipantEndStatus::Nexused);
    }

    #[test]
    fn close_calls_round_trip_and_sum_across_phases() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let mut cf = sample_fight();
        cf.local_close_calls = 3;
        db.insert_fight(&cf).unwrap();
        let f = &db.recent_fights(10).unwrap()[0];
        assert_eq!(f.local_close_calls, 3);
        // The standalone summary carries the same value.
        let s = &db.list_fights(&FightQuery::default(), 10).unwrap()[0];
        assert_eq!(s.local_close_calls, 3);

        // A grouped run sums close calls across its phases.
        let mut p1 = phase_fight(45217, "Argus", "run-cc", 1_000, true);
        p1.local_close_calls = 2;
        let mut p2 = phase_fight(45231, "Malus", "run-cc", 80_000, true);
        p2.local_close_calls = 4;
        db.insert_fight(&p1).unwrap();
        db.insert_fight(&p2).unwrap();
        let enc = db.encounter_detail("run-cc").unwrap().expect("encounter");
        assert_eq!(enc.total_close_calls(), 6);
        let group = db
            .list_fights(&FightQuery::default(), 10)
            .unwrap()
            .into_iter()
            .find(|s| s.encounter_run_id.as_deref() == Some("run-cc"))
            .expect("grouped summary");
        assert_eq!(group.local_close_calls, 6);
    }

    #[test]
    fn guarded_damage_round_trips() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let mut fight = sample_fight();
        fight.participants[0].guarded_damage = Some(12_345);
        fight.participants[0].guarded_hits = Some(7);
        // Second participant left as None to model a row with no guarded data.
        db.insert_fight(&fight).unwrap();

        let fights = db.recent_fights(10).unwrap();
        let p = &fights[0].participants;
        assert_eq!(p[0].guarded_damage, Some(12_345));
        assert_eq!(p[0].guarded_hits, Some(7));
        assert_eq!(p[1].guarded_damage, None);
        assert_eq!(p[1].guarded_hits, None);
    }

    #[test]
    fn damage_blocked_round_trips() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let mut fight = sample_fight();
        fight.participants[0].damage_blocked = Some(9_876);
        // Second participant left as None to model a remote / pre-feature row.
        db.insert_fight(&fight).unwrap();

        let fights = db.recent_fights(10).unwrap();
        let p = &fights[0].participants;
        assert_eq!(p[0].damage_blocked, Some(9_876));
        assert_eq!(p[1].damage_blocked, None);
    }

    #[test]
    fn v1_to_v2_migration_backfills_char_id() {
        // Build a v1-schema database with one fight row (no local_char_id column).
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE fights (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                started_at INTEGER NOT NULL,
                ended_at INTEGER NOT NULL,
                dungeon TEXT NOT NULL,
                map_seed INTEGER NOT NULL,
                boss_object_type INTEGER NOT NULL,
                boss_name TEXT NOT NULL,
                boss_max_hp INTEGER NOT NULL,
                boss_start_hp INTEGER NOT NULL,
                local_object_id INTEGER NOT NULL,
                killed INTEGER NOT NULL
            );
            CREATE TABLE fight_participants (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                fight_id INTEGER NOT NULL,
                object_id INTEGER NOT NULL,
                object_type INTEGER NOT NULL,
                name TEXT NOT NULL,
                equip0 INTEGER NOT NULL, equip1 INTEGER NOT NULL,
                equip2 INTEGER NOT NULL, equip3 INTEGER NOT NULL,
                damage INTEGER NOT NULL, hits INTEGER NOT NULL,
                is_local INTEGER NOT NULL, provenance TEXT NOT NULL
            );
            INSERT INTO fights
              (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
               boss_max_hp, boss_start_hp, local_object_id, killed)
              VALUES (1, 2, 'Old Lab', 7, 100, 'Legacy Boss', 500, 500, 9, 1);
            PRAGMA user_version = 1;
            "#,
        )
        .unwrap();

        let mut db = CombatDatabase { conn };
        db.initialize(None).unwrap();

        let version: i32 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        let fights = db.recent_fights(10).unwrap();
        assert_eq!(fights.len(), 1);
        assert_eq!(fights[0].local_char_id, 0);
        assert_eq!(fights[0].boss_name, "Legacy Boss");
    }

    #[test]
    fn regroup_realm_encounters_folds_historical_pentaract() {
        // Five Pentaract Tower fights recorded standalone (pre-realm-grouping)
        // in one realm visit must fold into a single encounter card, and a card
        // completes because a tower was killed.
        let mut db = CombatDatabase::open_in_memory().unwrap();
        db.conn
            .execute_batch(
                r#"
                INSERT INTO fights
                  (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
                   boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
                   encounter_id, encounter_run_id, boss_group)
                  VALUES
                  (100, 108, 'Realm', 555, 22018, 'Pentaract Tower', 10, 10, 9, 42, 1, NULL, NULL, 'adept_encounter'),
                  (101, 109, 'Realm', 555, 22018, 'Pentaract Tower', 10, 10, 9, 42, 1, NULL, NULL, 'adept_encounter'),
                  (102, 110, 'Realm', 555, 22018, 'Pentaract Tower', 10, 10, 9, 42, 1, NULL, NULL, 'adept_encounter'),
                  (103, 111, 'Realm', 555, 22018, 'Pentaract Tower', 10, 10, 9, 42, 1, NULL, NULL, 'adept_encounter'),
                  (104, 112, 'Realm', 555, 22018, 'Pentaract Tower', 10, 10, 9, 42, 1, NULL, NULL, 'adept_encounter');
                "#,
            )
            .unwrap();

        db.regroup_realm_encounters().unwrap();

        // All five share one encounter run.
        let distinct_runs: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(DISTINCT encounter_run_id) FROM fights WHERE boss_object_type = 22018",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(distinct_runs, 1, "all towers must share one run");

        let nulls: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fights WHERE encounter_run_id IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(nulls, 0, "no tower left standalone");

        // One grouped card, titled "Pentaract" and marked killed.
        let summaries = db
            .encounter_summaries(&FightQuery::default(), 10, None)
            .unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].boss_name, "Pentaract");
        assert_eq!(summaries[0].phase_count, 5);
        assert!(
            summaries[0].killed,
            "card completes when a tower was killed"
        );
    }

    #[test]
    fn towering_perfection_card_always_headlines_on_core() {
        // A Towering Perfection run where only the escaping segments (Lower /
        // Upper Imperfection + a Toppled Cube) took damage and the core (47909)
        // never appeared as a phase must still show the core's name and picture.
        let mut db = CombatDatabase::open_in_memory().unwrap();
        db.conn
            .execute_batch(
                r#"
                INSERT INTO fights
                  (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
                   boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
                   encounter_id, encounter_run_id, boss_group)
                  VALUES
                  (100, 108, 'Realm', 777, 47916, 'Lower Imperfection', 10, 10, 9, 42, 0, NULL, NULL, 'adept_encounter'),
                  (101, 109, 'Realm', 777, 47917, 'Upper Imperfection', 10, 10, 9, 42, 0, NULL, NULL, 'adept_encounter'),
                  (102, 110, 'Realm', 777, 47914, 'Toppled Green Cube', 10, 10, 9, 42, 0, NULL, NULL, 'adept_encounter');
                "#,
            )
            .unwrap();

        db.regroup_realm_encounters().unwrap();

        let summaries = db
            .encounter_summaries(&FightQuery::default(), 10, None)
            .unwrap();
        assert_eq!(summaries.len(), 1, "segments fold into one card");
        let card = &summaries[0];
        assert_eq!(
            card.boss_name, "Towering Perfection",
            "name is always the core"
        );
        assert_eq!(card.boss_object_type, 47909, "picture is always the core");
        assert_eq!(
            card.killed_bosses,
            vec![(47909, "Towering Perfection".to_string())],
            "subtitle is only the headline boss, never the segment names",
        );
    }

    #[test]
    fn regroup_folds_toppled_cubes_into_existing_towering_run() {
        // Toppled Cubes were curated into the Towering Perfection encounter after
        // some runs were already grouped (v48). Re-running the regroup must fold
        // their standalone cards into the EXISTING run, not spawn a duplicate,
        // and must preserve the run's loot-latched Completed flag.
        let mut db = CombatDatabase::open_in_memory().unwrap();
        db.conn
            .execute_batch(
                r#"
                INSERT INTO fights
                  (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
                   boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
                   encounter_id, encounter_run_id, boss_group)
                  VALUES
                  (100, 108, 'Realm', 777, 47909, 'Towering Perfection', 10, 10, 9, 42, 0, 'towering_perfection', 'tp-run', 'adept_encounter'),
                  (101, 109, 'Realm', 777, 47916, 'Lower Imperfection', 10, 10, 9, 42, 0, 'towering_perfection', 'tp-run', 'adept_encounter'),
                  (130, 135, 'Realm', 777, 47914, 'Toppled Green Cube', 10, 10, 9, 42, 0, NULL, NULL, 'adept_encounter');
                INSERT INTO encounter_runs (run_id, encounter_id, dungeon, started_at, ended_at, killed)
                  VALUES ('tp-run', 'towering_perfection', 'Realm', 100, 109, 1);
                "#,
            )
            .unwrap();

        db.regroup_realm_encounters().unwrap();

        let distinct_runs: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(DISTINCT encounter_run_id) FROM fights
                 WHERE boss_object_type IN (47909, 47914, 47916)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            distinct_runs, 1,
            "cube must join the existing run, not duplicate it"
        );

        let cube_run: String = db
            .conn
            .query_row(
                "SELECT encounter_run_id FROM fights WHERE boss_object_type = 47914",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            cube_run, "tp-run",
            "cube attaches to the pre-existing run id"
        );

        let killed: i64 = db
            .conn
            .query_row(
                "SELECT killed FROM encounter_runs WHERE run_id = 'tp-run'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(killed, 1, "loot-latched Completed flag is preserved");

        let summaries = db
            .encounter_summaries(&FightQuery::default(), 10, None)
            .unwrap();
        assert_eq!(
            summaries.len(),
            1,
            "still a single Towering Perfection card"
        );
        assert_eq!(summaries[0].boss_name, "Towering Perfection");
    }

    #[test]
    fn regroup_realm_encounters_splits_on_seed_and_time() {
        // A different realm visit (new seed) and a time-gapped repeat must not
        // merge into the same card.
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let gap = 40 * 60 * 1000; // > RUN_GAP_MS
        db.conn
            .execute(
                "INSERT INTO fights
                  (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
                   boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
                   encounter_id, encounter_run_id, boss_group)
                  VALUES (100, 108, 'Realm', 555, 22018, 'Pentaract Tower', 10, 10, 9, 42, 1, NULL, NULL, 'adept_encounter')",
                [],
            )
            .unwrap();
        // Same seed but a long time later -> separate run.
        db.conn
            .execute(
                "INSERT INTO fights
                  (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
                   boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
                   encounter_id, encounter_run_id, boss_group)
                  VALUES (?1, ?2, 'Realm', 555, 22018, 'Pentaract Tower', 10, 10, 9, 42, 1, NULL, NULL, 'adept_encounter')",
                params![100 + gap, 108 + gap],
            )
            .unwrap();
        // Different seed -> separate run.
        db.conn
            .execute(
                "INSERT INTO fights
                  (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
                   boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
                   encounter_id, encounter_run_id, boss_group)
                  VALUES (105, 113, 'Realm', 999, 22018, 'Pentaract Tower', 10, 10, 9, 42, 1, NULL, NULL, 'adept_encounter')",
                [],
            )
            .unwrap();

        db.regroup_realm_encounters().unwrap();

        let distinct_runs: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(DISTINCT encounter_run_id) FROM fights",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(distinct_runs, 3, "seed and time gaps split runs");
    }

    #[test]
    fn regroup_realm_encounters_folds_angry_hornets_into_nest() {
        // Historical Hornet's Nest (34603) + three standalone Angry Hornet
        // (34604) fights from one realm visit must fold into a single Hornet's
        // Nest encounter card, and the hornet rows must be preserved (the
        // migration folds instead of deleting).
        let mut db = CombatDatabase::open_in_memory().unwrap();
        db.conn
            .execute_batch(
                r#"
                INSERT INTO fights
                  (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
                   boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
                   encounter_id, encounter_run_id, boss_group)
                  VALUES
                  (100, 120, 'Realm', 777, 34603, 'Hornet''s Nest', 10, 10, 9, 42, 1, NULL, NULL, NULL),
                  (101, 105, 'Realm', 777, 34604, 'Angry Hornet',  10, 10, 9, 42, 1, NULL, NULL, NULL),
                  (106, 110, 'Realm', 777, 34604, 'Angry Hornet',  10, 10, 9, 42, 1, NULL, NULL, NULL),
                  (111, 115, 'Realm', 777, 34604, 'Angry Hornet',  10, 10, 9, 42, 1, NULL, NULL, NULL);
                "#,
            )
            .unwrap();

        db.regroup_realm_encounters().unwrap();

        // No hornet was deleted.
        let hornets: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fights WHERE boss_object_type = 34604",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hornets, 3, "angry hornets must be folded, not purged");

        // Nest and all hornets share one encounter run; nothing left standalone.
        let distinct_runs: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(DISTINCT encounter_run_id) FROM fights",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(distinct_runs, 1, "nest and hornets share one run");
        let nulls: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fights WHERE encounter_run_id IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(nulls, 0, "no fight left standalone");

        // One grouped Hornet's Nest card holding all four phases.
        let summaries = db
            .encounter_summaries(&FightQuery::default(), 10, None)
            .unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].boss_name, "Hornet's Nest");
        assert_eq!(summaries[0].phase_count, 4);
    }

    #[test]
    fn regroup_realm_encounters_folds_legion_orbs_into_missionary() {
        // Historical Legion Missionary (53013) + Holy Orb (53017) + Chaos Orb
        // (53018) fights from one realm visit must fold into a single Legion
        // Missionary encounter card, preserving the orb rows (the migration
        // folds instead of deleting).
        let mut db = CombatDatabase::open_in_memory().unwrap();
        db.conn
            .execute_batch(
                r#"
                INSERT INTO fights
                  (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
                   boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
                   encounter_id, encounter_run_id, boss_group)
                  VALUES
                  (100, 120, 'Realm', 888, 53013, 'Legion Missionary',           10, 10, 9, 42, 1, NULL, NULL, NULL),
                  (101, 105, 'Realm', 888, 53017, 'Legion Missionary Holy Orb',  10, 10, 9, 42, 1, NULL, NULL, NULL),
                  (106, 110, 'Realm', 888, 53018, 'Legion Missionary Chaos Orb', 10, 10, 9, 42, 1, NULL, NULL, NULL);
                "#,
            )
            .unwrap();

        db.regroup_realm_encounters().unwrap();

        // No orb was deleted.
        let orbs: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fights WHERE boss_object_type IN (53017, 53018)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orbs, 2, "orbs must be folded, not purged");

        // Missionary and both orbs share one encounter run; nothing standalone.
        let nulls: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fights WHERE encounter_run_id IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(nulls, 0, "no fight left standalone");

        // One grouped Legion Missionary card holding all three phases.
        let summaries = db
            .encounter_summaries(&FightQuery::default(), 10, None)
            .unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].boss_name, "Legion Missionary");
        assert_eq!(summaries[0].phase_count, 3);
    }

    #[test]
    fn regroup_dungeon_instances_folds_crate_into_boss_card() {
        // A Sunken Treasure crate (28065) recorded standalone in a Davy Jones'
        // Locker instance must join the boss run for the same seed, so the crate
        // and boss show as one dungeon card instead of two.
        let mut db = CombatDatabase::open_in_memory().unwrap();
        db.insert_fight(&grouped_fight(
            "Davy Jones' Locker",
            3634,
            "Davy Jones",
            "run-dj",
            5_000,
            true,
            300_000,
        ))
        .unwrap();
        // The crate row: same char/dungeon/seed, but NULL run id (legacy build).
        db.conn
            .execute(
                "INSERT INTO fights
                  (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
                   boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
                   encounter_id, encounter_run_id, boss_group)
                  VALUES (1000, 1500, 'Davy Jones'' Locker', 42, 28065, 'Sunken Treasure',
                          5000, 5000, 1000, 777, 1, NULL, NULL, 'treasure_crate')",
                [],
            )
            .unwrap();

        db.regroup_dungeon_instances().unwrap();

        let nulls: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fights WHERE encounter_run_id IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(nulls, 0, "crate must not stay standalone");

        // One grouped card, headlined by the dungeon with the boss as anchor.
        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 1, "crate folds into the boss card");
        let g = &list[0];
        assert!(g.is_encounter());
        assert_eq!(g.boss_name, "Davy Jones' Locker");
        assert_eq!(
            g.boss_object_type, 3634,
            "boss anchors the card, not the crate"
        );
        assert_eq!(g.phase_count, 2);
        assert!(g.killed);
    }

    #[test]
    fn crate_never_hijacks_dungeon_anchor() {
        // A crate opened (and thus "ended") after the boss died must not become
        // the card's anchor icon/subtitle: the real boss always headlines.
        let mut db = CombatDatabase::open_in_memory().unwrap();
        db.insert_fight(&grouped_fight(
            "Davy Jones' Locker",
            3634,
            "Davy Jones",
            "run-dj",
            1_000,
            true,
            300_000,
        ))
        .unwrap();
        db.insert_fight(&grouped_fight(
            "Davy Jones' Locker",
            28065,
            "Sunken Treasure",
            "run-dj",
            90_000, // opens well after the boss dies
            true,
            5_000,
        ))
        .unwrap();

        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 1);
        let g = &list[0];
        assert_eq!(
            g.boss_object_type, 3634,
            "boss anchors over the later crate"
        );
        assert_eq!(g.boss_max_hp, 300_000);
    }

    #[test]
    fn forax_run_anchors_on_acidus_not_waste() {
        // A full Forax run: the curated "Waste" miniboss encounter (45917) plus
        // the standalone bosses Heartbat and Acidus (45827, the real final boss)
        // and a Satellite Core crate opened last. The card must headline on
        // Acidus, not the Waste miniboss whose encounter declares its own anchor.
        let mut db = CombatDatabase::open_in_memory().unwrap();
        db.insert_fight(&grouped_fight(
            "Forax",
            45916,
            "Heartbat",
            "run-forax",
            1_000,
            true,
            200_000,
        ))
        .unwrap();
        db.insert_fight(&grouped_fight(
            "Forax",
            45917,
            "Waste",
            "run-forax",
            2_000,
            true,
            250_000,
        ))
        .unwrap();
        db.insert_fight(&grouped_fight(
            "Forax",
            45827,
            "Acidus",
            "run-forax",
            3_000,
            true,
            400_000,
        ))
        .unwrap();
        db.insert_fight(&grouped_fight(
            "Forax",
            45909,
            "Satellite Core",
            "run-forax",
            9_000,
            true,
            5_000,
        ))
        .unwrap();

        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 1, "the whole Forax run is one card");
        let g = &list[0];
        assert_eq!(
            g.boss_object_type, 45827,
            "Acidus anchors the card, not the Waste miniboss or the crate"
        );
    }

    #[test]
    fn search_and_list_filters() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let mut a = sample_fight();
        a.local_char_id = 777;
        a.dungeon = "Mad Lab".to_string();
        a.boss_object_type = 100;
        a.boss_name = "Dr Terrible".to_string();
        let mut b = sample_fight();
        b.local_char_id = 0; // legacy/unknown identity
        b.dungeon = "Sprite World".to_string();
        b.boss_object_type = 200;
        b.boss_name = "Limon".to_string();
        db.insert_fight(&a).unwrap();
        db.insert_fight(&b).unwrap();

        // Text filter on boss name.
        let q = FightQuery {
            text: Some("limon".into()),
            ..Default::default()
        };
        assert_eq!(db.search_fights(&q, 10).unwrap().len(), 1);
        assert_eq!(db.list_fights(&q, 10).unwrap().len(), 1);

        // Dungeon filter.
        let q = FightQuery {
            dungeon: Some("Mad Lab".into()),
            ..Default::default()
        };
        let got = db.search_fights(&q, 10).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].boss_name, "Dr Terrible");

        // Boss object type filter.
        let q = FightQuery {
            boss_object_type: Some(200),
            ..Default::default()
        };
        assert_eq!(db.search_fights(&q, 10).unwrap().len(), 1);

        // Character filter excludes unknown-identity (0) fights.
        let q = FightQuery {
            char_id: Some(777),
            ..Default::default()
        };
        let got = db.search_fights(&q, 10).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].local_char_id, 777);

        // Empty query returns everything, newest first.
        let all = db.list_fights(&FightQuery::default(), 10).unwrap();
        assert_eq!(all.len(), 2);
        // Summary carries participant count + local object type.
        let summary = all.iter().find(|s| s.local_char_id == 777).unwrap();
        assert_eq!(summary.participant_count, 2);
        assert_eq!(summary.local_object_type, Some(0x0321));
    }

    #[test]
    fn fight_detail_round_trip() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let id = db.insert_fight(&sample_fight()).unwrap();
        let detail = db.fight_detail(id).unwrap().unwrap();
        assert_eq!(detail.participants.len(), 2);
        assert_eq!(detail.local_char_id, 777);
        assert!(db.fight_detail(999_999).unwrap().is_none());
    }

    #[test]
    fn summary_surfaces_local_death_and_stored_skin() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // Baseline: local player nexused (survived), default skin -> not "died".
        let alive = sample_fight();
        db.insert_fight(&alive).unwrap();

        // A separate fight where the local player died on a skinned character.
        let mut died = sample_fight();
        died.started_at = 500_000;
        died.ended_at = 560_000;
        died.local_char_id = 888;
        died.killed = false;
        let local = died.participants.iter_mut().find(|p| p.is_local).unwrap();
        local.end_status = ParticipantEndStatus::Died { grave_type: 1830 };
        local.skin_id = 4242;
        local.tex1 = 0x0100_00ff;
        local.tex2 = 0x0100_ff00;
        db.insert_fight(&died).unwrap();

        let all = db.list_fights(&FightQuery::default(), 10).unwrap();

        let survived = all.iter().find(|s| s.local_char_id == 777).unwrap();
        assert!(!survived.local_died);
        assert_eq!(survived.local_skin_id, 0);

        let dead = all.iter().find(|s| s.local_char_id == 888).unwrap();
        assert!(dead.local_died);
        assert_eq!(dead.local_skin_id, 4242);
        assert_eq!(dead.local_tex1, 0x0100_00ff);
        assert_eq!(dead.local_tex2, 0x0100_ff00);
    }

    /// Build a standalone fight with an explicit set of participant IGNs.
    fn fight_with_players(dungeon: &str, started: i64, names: &[&str]) -> CompletedFight {
        let mut f = sample_fight();
        f.dungeon = dungeon.to_string();
        f.started_at = started;
        f.ended_at = started + 30_000;
        f.encounter_id = None;
        f.encounter_run_id = None;
        f.participants = names
            .iter()
            .enumerate()
            .map(|(i, name)| FightParticipant {
                object_id: 500 + i as i32,
                object_type: 0x0321,
                skin_id: 0,
                tex1: 0,
                tex2: 0,
                name: name.to_string(),
                equipment: [-1; 4],
                equipment_enchants: Default::default(),
                damage: 1_000,
                hits: 10,
                is_local: false,
                provenance: DamageProvenance::Observed,
                end_status: ParticipantEndStatus::Present,
                pet: None,
                damage_taken: None,
                damage_taken_provenance: DamageTakenProvenance::Observed,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
            })
            .collect();
        f
    }

    #[test]
    fn player_name_filter_matches_standalone_grouped_and_all_query_paths() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // Two standalone fights with different rosters.
        db.insert_fight(&fight_with_players("Mad Lab", 1_000, &["Alice", "Bob"]))
            .unwrap();
        db.insert_fight(&fight_with_players("Sprite World", 2_000, &["Carol"]))
            .unwrap();
        // A grouped encounter (two phases) where only Dave took part.
        let mut p1 = fight_with_players("The Nest", 3_000, &["Dave"]);
        p1.encounter_id = Some("dungeon_run".to_string());
        p1.encounter_run_id = Some("run-p".to_string());
        let mut p2 = fight_with_players("The Nest", 40_000, &["Dave"]);
        p2.encounter_id = Some("dungeon_run".to_string());
        p2.encounter_run_id = Some("run-p".to_string());
        db.insert_fight(&p1).unwrap();
        db.insert_fight(&p2).unwrap();

        // Standalone match across list_fights, search_fights and selections.
        let q = FightQuery {
            player_name: Some("Alice".into()),
            ..Default::default()
        };
        assert_eq!(db.list_fights(&q, 50).unwrap().len(), 1);
        let got = db.search_fights(&q, 50).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].dungeon, "Mad Lab");
        assert_eq!(db.selections_matching(&q).unwrap().len(), 1);

        // A player in a different fight only matches that fight.
        let q = FightQuery {
            player_name: Some("Carol".into()),
            ..Default::default()
        };
        let got = db.search_fights(&q, 50).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].dungeon, "Sprite World");

        // Grouped encounter: matching any phase returns the one grouped row.
        let q = FightQuery {
            player_name: Some("Dave".into()),
            ..Default::default()
        };
        let list = db.list_fights(&q, 50).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].is_encounter());
        let sels = db.selections_matching(&q).unwrap();
        assert_eq!(sels.len(), 1);

        // An unknown IGN matches nothing.
        let q = FightQuery {
            player_name: Some("Nobody".into()),
            ..Default::default()
        };
        assert_eq!(db.list_fights(&q, 50).unwrap().len(), 0);

        // Suggestions list every real IGN and excludes synthetic placeholders.
        let players = db.distinct_players().unwrap();
        let names: Vec<&str> = players.iter().map(|(n, ..)| n.as_str()).collect();
        assert!(names.contains(&"Alice"));
        assert!(names.contains(&"Bob"));
        assert!(names.contains(&"Carol"));
        assert!(names.contains(&"Dave"));
        assert!(!names.contains(&"You"));
        assert!(!names.contains(&"Unknown"));
        // Each suggestion carries a sprite id (the most recent class/skin seen).
        assert!(players.iter().all(|(_, icon, ..)| *icon != 0));
    }

    #[test]
    fn participant_count_filter_matches_standalone_and_grouped_cards() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // Standalone: 2-roster and 1-roster fights.
        db.insert_fight(&fight_with_players("Mad Lab", 1_000, &["Alice", "Bob"]))
            .unwrap();
        db.insert_fight(&fight_with_players("Sprite World", 2_000, &["Carol"]))
            .unwrap();
        // Grouped encounter whose two phases share one roster of two: the card's
        // distinct-participant count is 2, not 4.
        let mut p1 = fight_with_players("The Nest", 3_000, &["Dave", "Erin"]);
        p1.encounter_id = Some("dungeon_run".to_string());
        p1.encounter_run_id = Some("run-p".to_string());
        let mut p2 = fight_with_players("The Nest", 40_000, &["Dave", "Erin"]);
        p2.encounter_id = Some("dungeon_run".to_string());
        p2.encounter_run_id = Some("run-p".to_string());
        db.insert_fight(&p1).unwrap();
        db.insert_fight(&p2).unwrap();

        // Two-participant cards: the Mad Lab standalone plus the grouped run.
        let q = FightQuery {
            participant_count: Some(2),
            ..Default::default()
        };
        let list = db.list_fights(&q, 50).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(db.selections_matching(&q).unwrap().len(), 2);

        // One-participant cards: only the Sprite World standalone.
        let q = FightQuery {
            participant_count: Some(1),
            ..Default::default()
        };
        let list = db.list_fights(&q, 50).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].dungeon, "Sprite World");

        // A count that never occurs matches nothing.
        let q = FightQuery {
            participant_count: Some(4),
            ..Default::default()
        };
        assert_eq!(db.list_fights(&q, 50).unwrap().len(), 0);

        // Distinct-count suggestions expose exactly the counts that exist.
        let counts = db.distinct_participant_counts().unwrap();
        assert_eq!(counts, vec![1, 2]);
    }

    /// A Cultist Hideout phase fight with a given boss type, run id and killed.
    fn phase_fight(
        boss_type: i32,
        name: &str,
        run: &str,
        started: i64,
        killed: bool,
    ) -> CompletedFight {
        let mut f = sample_fight();
        f.dungeon = "Cultist Hideout".to_string();
        f.boss_object_type = boss_type;
        f.boss_name = name.to_string();
        f.started_at = started;
        f.ended_at = started + 30_000;
        f.killed = killed;
        f.encounter_id = Some("cultist_hideout".to_string());
        f.encounter_run_id = Some(run.to_string());
        f
    }

    fn o3_participant(
        object_id: i32,
        name: &str,
        is_local: bool,
        end_status: ParticipantEndStatus,
    ) -> FightParticipant {
        FightParticipant {
            object_id,
            object_type: 0x0321,
            skin_id: 0,
            tex1: 0,
            tex2: 0,
            name: name.to_string(),
            equipment: [0, 0, 0, 0],
            equipment_enchants: Default::default(),
            damage: 1_000,
            hits: 10,
            is_local,
            provenance: DamageProvenance::Observed,
            end_status,
            pet: None,
            damage_taken: None,
            damage_taken_provenance: DamageTakenProvenance::Observed,
            damage_blocked: None,
            guarded_damage: None,
            guarded_hits: None,
        }
    }

    fn o3_fight(
        boss_type: i32,
        name: &str,
        started: i64,
        killed: bool,
        participants: Vec<FightParticipant>,
    ) -> CompletedFight {
        let mut f = sample_fight();
        f.dungeon = "Oryx's Sanctuary".to_string();
        f.boss_object_type = boss_type;
        f.boss_name = name.to_string();
        f.started_at = started;
        f.ended_at = started + 30_000;
        f.killed = killed;
        f.encounter_id = Some("o3_oryx".to_string());
        f.encounter_run_id = Some("run-o3".to_string());
        f.participants = participants;
        f
    }

    #[test]
    fn phase_participants_inherit_main_boss_status_245() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // The Messengers aux phase finalizes ~15s after Oryx dies, so its roster
        // wrongly flags still-present players as Nexused.
        db.insert_fight(&o3_fight(
            45365,
            "Messengers",
            1_000,
            false,
            vec![
                o3_participant(700, "Bob", false, ParticipantEndStatus::Nexused),
                o3_participant(701, "Carol", false, ParticipantEndStatus::Nexused),
                o3_participant(702, "Dave", false, ParticipantEndStatus::Nexused),
            ],
        ))
        .unwrap();
        // Anchor (Oryx the Mad God 3): the authoritative fates at the boss kill.
        db.insert_fight(&o3_fight(
            45363,
            "Oryx the Mad God",
            2_000,
            true,
            vec![
                o3_participant(700, "Bob", false, ParticipantEndStatus::Present),
                o3_participant(
                    701,
                    "Carol",
                    false,
                    ParticipantEndStatus::Died { grave_type: 1830 },
                ),
            ],
        ))
        .unwrap();

        let enc = db.encounter_detail("run-o3").unwrap().unwrap();
        let messengers = enc
            .phases
            .iter()
            .find(|p| p.boss_object_type == 45365)
            .expect("messengers phase");

        // Bob was present at the boss kill -> the Messengers phase must not show
        // him as Nexused.
        let bob = messengers
            .participants
            .iter()
            .find(|p| p.name == "Bob")
            .unwrap();
        assert!(
            matches!(bob.end_status, ParticipantEndStatus::Present),
            "Bob should inherit the main boss Present status, got {:?}",
            bob.end_status,
        );
        // Carol died to the boss -> Died wins everywhere.
        let carol = messengers
            .participants
            .iter()
            .find(|p| p.name == "Carol")
            .unwrap();
        assert!(
            matches!(carol.end_status, ParticipantEndStatus::Died { .. }),
            "Carol should inherit the main boss Died status, got {:?}",
            carol.end_status,
        );
        // Dave never reached the boss (absent from the anchor) -> keeps his
        // raw aux status (Nexused) since there is no boss-phase evidence.
        let dave = messengers
            .participants
            .iter()
            .find(|p| p.name == "Dave")
            .unwrap();
        assert!(
            matches!(dave.end_status, ParticipantEndStatus::Nexused),
            "Dave has no anchor evidence and should stay Nexused, got {:?}",
            dave.end_status,
        );
    }

    #[test]
    fn miniboss_phase_keeps_present_when_player_dies_at_later_boss_245() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // Chancellor Dammah is a real miniboss fought BEFORE Oryx in the same run.
        // Players complete it (Present), then some nexus/die at Oryx later. The
        // miniboss phase must keep its own Present fate -- the later anchor status
        // must NOT bleed backward into a sequential earlier phase.
        db.insert_fight(&o3_fight(
            9635,
            "Chancellor Dammah",
            1_000,
            true,
            vec![
                o3_participant(700, "Bob", false, ParticipantEndStatus::Present),
                o3_participant(701, "Carol", false, ParticipantEndStatus::Present),
            ],
        ))
        .unwrap();
        db.insert_fight(&o3_fight(
            45363,
            "Oryx the Mad God",
            2_000,
            true,
            vec![
                o3_participant(700, "Bob", false, ParticipantEndStatus::Nexused),
                o3_participant(
                    701,
                    "Carol",
                    false,
                    ParticipantEndStatus::Died { grave_type: 1830 },
                ),
            ],
        ))
        .unwrap();

        let enc = db.encounter_detail("run-o3").unwrap().unwrap();
        let dammah = enc
            .phases
            .iter()
            .find(|p| p.boss_object_type == 9635)
            .expect("dammah phase");
        for name in ["Bob", "Carol"] {
            let p = dammah.participants.iter().find(|p| p.name == name).unwrap();
            assert!(
                matches!(p.end_status, ParticipantEndStatus::Present),
                "{name} completed the miniboss and must stay Present there, got {:?}",
                p.end_status,
            );
        }
    }

    #[test]
    fn all_aux_encounter_keeps_present_status_245() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // An all-aux run (no real boss recorded): two Messenger sub-fights, one of
        // which finalizes late and wrongly flags the player Nexused. The picked
        // anchor is an aux row, so we must fall back to the combined roster and
        // keep the player Present -- never downgrade from the late aux phase.
        db.insert_fight(&o3_fight(
            45365,
            "Messengers",
            1_000,
            false,
            vec![o3_participant(
                700,
                "Bob",
                false,
                ParticipantEndStatus::Present,
            )],
        ))
        .unwrap();
        db.insert_fight(&o3_fight(
            45431,
            "Messengers",
            2_000,
            false,
            vec![o3_participant(
                700,
                "Bob",
                false,
                ParticipantEndStatus::Nexused,
            )],
        ))
        .unwrap();

        let enc = db.encounter_detail("run-o3").unwrap().unwrap();
        let phase = enc
            .phases
            .iter()
            .find(|p| crate::assets::aux_target_for_type(p.boss_object_type).is_some())
            .expect("aux phase");
        let bob = phase.participants.iter().find(|p| p.name == "Bob").unwrap();
        assert!(
            matches!(bob.end_status, ParticipantEndStatus::Present),
            "all-aux encounter must not downgrade a present player to Nexused, got {:?}",
            bob.end_status,
        );
    }

    #[test]
    fn boss_phase_inherits_death_from_concurrent_addon_245() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // A player dies during the boss's invulnerable add-on phase: the death is
        // captured on the concurrent aux (Messengers) phase, but the boss phase
        // only saw them leave range -> Nexused. The boss card must still show
        // them Died, inheriting the aux death.
        db.insert_fight(&o3_fight(
            45365,
            "Messengers",
            1_000,
            false,
            vec![
                o3_participant(700, "Bob", false, ParticipantEndStatus::Present),
                o3_participant(
                    701,
                    "Carol",
                    false,
                    ParticipantEndStatus::Died { grave_type: 1830 },
                ),
            ],
        ))
        .unwrap();
        db.insert_fight(&o3_fight(
            45363,
            "Oryx the Mad God",
            2_000,
            true,
            vec![
                o3_participant(700, "Bob", false, ParticipantEndStatus::Present),
                o3_participant(701, "Carol", false, ParticipantEndStatus::Nexused),
            ],
        ))
        .unwrap();

        let enc = db.encounter_detail("run-o3").unwrap().unwrap();
        let boss = enc
            .phases
            .iter()
            .find(|p| p.boss_object_type == 45363)
            .expect("boss phase");
        let carol = boss
            .participants
            .iter()
            .find(|p| p.name == "Carol")
            .unwrap();
        assert!(
            matches!(
                carol.end_status,
                ParticipantEndStatus::Died { grave_type: 1830 }
            ),
            "boss phase must inherit Carol's death from the concurrent add-on, got {:?}",
            carol.end_status,
        );
        // Bob never died anywhere -> unaffected.
        let bob = boss.participants.iter().find(|p| p.name == "Bob").unwrap();
        assert!(matches!(bob.end_status, ParticipantEndStatus::Present));
        // The aggregated roster reflects the death too.
        let carol_r = enc.roster.iter().find(|p| p.name == "Carol").unwrap();
        assert!(matches!(
            carol_r.end_status,
            ParticipantEndStatus::Died { .. }
        ));
    }

    #[test]
    fn encounter_phases_group_into_one_summary() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // Two cultist phases + the anchor, one standalone unrelated fight.
        db.insert_fight(&phase_fight(45217, "Argus", "run-1", 1_000, true))
            .unwrap();
        db.insert_fight(&phase_fight(45219, "Gaius", "run-1", 40_000, true))
            .unwrap();
        db.insert_fight(&phase_fight(45231, "Malus", "run-1", 80_000, true))
            .unwrap();
        db.insert_fight(&sample_fight()).unwrap();

        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        // One grouped card + one standalone = 2 rows (not 4).
        assert_eq!(list.len(), 2);
        let group = list.iter().find(|s| s.is_encounter()).unwrap();
        assert_eq!(group.boss_name, "Cultist Hideout"); // dungeon headline
        assert_eq!(group.phase_count, 3);
        assert!(group.killed);
        assert_eq!(
            group.selection(),
            FightSelection::Encounter("run-1".to_string())
        );
        // Group span covers all phases.
        assert_eq!(group.started_at, 1_000);
        assert_eq!(group.ended_at, 80_000 + 30_000);
    }

    #[test]
    fn filtering_by_one_phase_returns_whole_group() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        db.insert_fight(&phase_fight(45217, "Argus", "run-1", 1_000, true))
            .unwrap();
        db.insert_fight(&phase_fight(45231, "Malus", "run-1", 80_000, true))
            .unwrap();

        // Searching a non-anchor phase name still yields the whole encounter.
        let q = FightQuery {
            text: Some("argus".into()),
            ..Default::default()
        };
        let list = db.list_fights(&q, 50).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].phase_count, 2);
        // Searching the canonical name also works.
        let q = FightQuery {
            text: Some("malus".into()),
            ..Default::default()
        };
        assert_eq!(db.list_fights(&q, 50).unwrap().len(), 1);
    }

    #[test]
    fn grouped_list_filters_before_applying_limit() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let mut older = phase_fight(45231, "Malus", "run-old", 1_000, true);
        older.local_char_id = 1;
        let mut newer = phase_fight(45231, "Malus", "run-new", 10_000, true);
        newer.local_char_id = 2;
        db.insert_fight(&older).unwrap();
        db.insert_fight(&newer).unwrap();

        // The newest run does not belong to character 1. The filter must run
        // before the list limit so the older matching run is still returned.
        let q = FightQuery {
            char_id: Some(1),
            ..Default::default()
        };
        let list = db.list_fights(&q, 1).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].encounter_run_id.as_deref(), Some("run-old"));
    }

    #[test]
    fn backfill_leaves_ambiguous_legacy_fights_standalone() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let mut first = sample_fight();
        first.dungeon = "Cultist Hideout".to_string();
        first.boss_object_type = 45217;
        first.boss_name = "Argus".to_string();
        first.local_char_id = 0;
        first.map_seed = 0;
        let mut second = first.clone();
        second.boss_object_type = 45231;
        second.boss_name = "Malus".to_string();
        second.started_at = 2_000;
        second.ended_at = 3_000;
        db.insert_fight(&first).unwrap();
        db.insert_fight(&second).unwrap();

        db.backfill_encounter_runs().unwrap();

        assert_eq!(
            db.conn
                .query_row("SELECT COUNT(*) FROM encounter_runs", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(db.list_fights(&FightQuery::default(), 10).unwrap().len(), 2);
    }

    #[test]
    fn v5_to_v6_repairs_unsafe_backfill_merges() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let mut first = phase_fight(45217, "Argus", "bf-cultist_hideout-3e8", 1_000, true);
        first.local_char_id = 1;
        first.map_seed = 42;
        let mut second = phase_fight(45231, "Malus", "bf-cultist_hideout-3e8", 2_000, true);
        second.local_char_id = 2;
        second.map_seed = 42;
        db.insert_fight(&first).unwrap();
        db.insert_fight(&second).unwrap();
        db.conn.execute_batch("PRAGMA user_version = 5").unwrap();

        db.initialize(None).unwrap();

        assert_eq!(
            db.conn
                .query_row("SELECT COUNT(*) FROM encounter_runs", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            db.conn
                .query_row(
                    "SELECT COUNT(*) FROM fights WHERE encounter_run_id LIKE 'bf-%'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn encounter_detail_aggregates_roster_and_deletes() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        db.insert_fight(&phase_fight(45217, "Argus", "run-1", 1_000, true))
            .unwrap();
        db.insert_fight(&phase_fight(45231, "Malus", "run-1", 80_000, true))
            .unwrap();

        let enc = db.encounter_detail("run-1").unwrap().unwrap();
        assert_eq!(enc.display_name, "Cultist Hideout");
        assert_eq!(enc.phases.len(), 2);
        // Each sample phase has Alice (200k) + local You; summed across 2 phases.
        let alice = enc.roster.iter().find(|p| p.name == "Alice").unwrap();
        assert_eq!(alice.damage, 400_000);
        let me = enc.roster.iter().find(|p| p.is_local).unwrap();
        // Both phases SelfPending -> reduced to SelfPending.
        assert_eq!(me.provenance, DamageProvenance::SelfPending);
        // Active duration excludes the gap between phases.
        assert_eq!(enc.active_duration_ms(), 60_000);

        // Deleting the run cascades to all phases + participants.
        db.delete_selection(&FightSelection::Encounter("run-1".to_string()))
            .unwrap();
        assert_eq!(db.fight_count().unwrap(), 0);
        assert!(db.encounter_detail("run-1").unwrap().is_none());
    }

    #[test]
    fn total_dungeon_time_from_entry_and_active_excludes_addons() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // Main boss: entered the dungeon at t=1000, fought from 10s to 40s.
        let mut boss = grouped_fight(
            "The Nest",
            4000,
            "Killer Bee Queen",
            "run-z",
            10_000,
            true,
            100_000,
        );
        boss.dungeon_entered_at = Some(1_000);
        // Aggregated add-on row (aux) overlapping the boss fight: must be excluded
        // from active combat time and never push active past total dungeon time.
        let mut addon = grouped_fight(
            "The Nest",
            53018,
            "Orb of Chaos",
            "run-z",
            12_000,
            true,
            20_000,
        );
        addon.dungeon_entered_at = Some(1_000);
        db.insert_fight(&boss).unwrap();
        db.insert_fight(&addon).unwrap();

        let enc = db.encounter_detail("run-z").unwrap().unwrap();
        // Active counts only the big boss interval (30s); the add-on is excluded.
        assert_eq!(enc.active_duration_ms(), 30_000);
        // Total spans from dungeon entry to the final boss death.
        assert_eq!(enc.total_dungeon_ms(), enc.ended_at - 1_000);
        assert!(enc.active_duration_ms() <= enc.total_dungeon_ms());
    }

    #[test]
    fn total_dungeon_time_falls_back_to_phase_start_for_legacy_rows() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // Legacy row with no recorded dungeon entry: total falls back to the
        // earliest phase start (old behavior).
        db.insert_fight(&grouped_fight(
            "The Nest",
            4000,
            "Killer Bee Queen",
            "run-l",
            10_000,
            true,
            100_000,
        ))
        .unwrap();
        let enc = db.encounter_detail("run-l").unwrap().unwrap();
        assert_eq!(enc.total_dungeon_ms(), enc.ended_at - enc.started_at);
    }

    #[test]
    fn royal_crabs_collapse_into_one_phase() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // Crab Sovereign anchor + several Royal Crab (51073) adds recorded as
        // separate historical rows must fold into a single Royal Crab phase with
        // summed HP, while the anchor stays its own phase.
        db.insert_fight(&grouped_fight(
            "Realm",
            51072,
            "Crab Sovereign",
            "run-c",
            1_000,
            true,
            149_250,
        ))
        .unwrap();
        db.insert_fight(&grouped_fight(
            "Realm",
            51073,
            "Royal Crab",
            "run-c",
            2_000,
            true,
            16_152,
        ))
        .unwrap();
        db.insert_fight(&grouped_fight(
            "Realm",
            51073,
            "Royal Crab",
            "run-c",
            3_000,
            true,
            15_080,
        ))
        .unwrap();
        db.insert_fight(&grouped_fight(
            "Realm",
            51073,
            "Royal Crab",
            "run-c",
            4_000,
            true,
            18_297,
        ))
        .unwrap();

        let enc = db.encounter_detail("run-c").unwrap().unwrap();
        // Sovereign + one merged Royal Crab row = 2 phases (not 4).
        assert_eq!(enc.phases.len(), 2);
        let crab = enc
            .phases
            .iter()
            .find(|p| p.boss_object_type == 51073)
            .expect("merged royal crab phase");
        assert_eq!(crab.boss_name, "Royal Crab");
        // Summed max HP across the three crabs.
        assert_eq!(crab.boss_max_hp, 16_152 + 15_080 + 18_297);
        assert!(crab.killed);
        // Roster totals are unchanged by the collapse.
        assert_eq!(enc.display_name, "Crab Sovereign");
    }

    fn grouped_fight(
        dungeon: &str,
        boss_type: i32,
        name: &str,
        run: &str,
        started: i64,
        killed: bool,
        max_hp: i32,
    ) -> CompletedFight {
        let mut f = sample_fight();
        f.dungeon = dungeon.to_string();
        f.boss_object_type = boss_type;
        f.boss_name = name.to_string();
        f.started_at = started;
        f.ended_at = started + 30_000;
        f.killed = killed;
        f.boss_max_hp = max_hp;
        f.boss_start_hp = max_hp;
        f.encounter_id = Some("dungeon_run".to_string());
        f.encounter_run_id = Some(run.to_string());
        f
    }

    #[test]
    fn legacy_megamoth_completes_only_when_final_form_killed() {
        // The three self-destructing forms share one dungeon-instance run. The
        // earlier forms (Larva 52662, Mammoth 52663) are killed=false; only the
        // final Murderous Megamoth (52665) decides completion.
        let mut db = CombatDatabase::open_in_memory().unwrap();
        db.insert_fight(&grouped_fight(
            "Legacy Woodland Labyrinth",
            52662,
            "Retro Megamoth Larva",
            "wl-a",
            1_000,
            false,
            124_920,
        ))
        .unwrap();
        db.insert_fight(&grouped_fight(
            "Legacy Woodland Labyrinth",
            52663,
            "Retro Mammoth Megamoth",
            "wl-a",
            30_000,
            false,
            124_920,
        ))
        .unwrap();
        // No final form yet (player nexused mid-fight) -> the card is Escaped.
        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 1);
        assert!(!list[0].killed, "no Murderous Megamoth kill -> Escaped");

        // The final form dies -> the whole grouped card is Completed and anchors
        // on the Murderous Megamoth.
        db.insert_fight(&grouped_fight(
            "Legacy Woodland Labyrinth",
            52665,
            "Retro Murderous Megamoth",
            "wl-a",
            60_000,
            true,
            83_280,
        ))
        .unwrap();
        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].boss_object_type, 52665);
        assert!(list[0].killed, "Murderous Megamoth killed -> Completed");
    }

    #[test]
    fn legacy_megamoth_escaped_even_if_intermediate_form_scored_killed() {
        // Guard: an intermediate form stored as killed=true (e.g. a rare
        // transition scored as a kill) must NOT complete the card when the final
        // Murderous Megamoth never died.
        let mut db = CombatDatabase::open_in_memory().unwrap();
        db.insert_fight(&grouped_fight(
            "Legacy Woodland Labyrinth",
            52662,
            "Retro Megamoth Larva",
            "wl-b",
            1_000,
            true,
            124_920,
        ))
        .unwrap();
        db.insert_fight(&grouped_fight(
            "Legacy Woodland Labyrinth",
            52663,
            "Retro Mammoth Megamoth",
            "wl-b",
            30_000,
            true,
            124_920,
        ))
        .unwrap();
        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 1);
        assert!(
            !list[0].killed,
            "final form absent -> Escaped despite killed intermediates"
        );
    }

    #[test]
    fn dungeon_instance_groups_all_bosses_under_dungeon_name() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // Two unrelated bosses of one instance share the run id (as the tracker
        // tags them live). They fold into one card headlined by the dungeon.
        db.insert_fight(&grouped_fight(
            "The Nest",
            4000,
            "Killer Bee Queen",
            "run-x",
            1_000,
            true,
            112_500,
        ))
        .unwrap();
        db.insert_fight(&grouped_fight(
            "The Nest",
            4001,
            "Some Miniboss",
            "run-x",
            40_000,
            true,
            50_000,
        ))
        .unwrap();

        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 1);
        let g = &list[0];
        assert!(g.is_encounter());
        assert_eq!(g.boss_name, "The Nest");
        assert_eq!(g.phase_count, 2);
        assert!(g.killed);
    }

    #[test]
    fn aux_summary_never_hijacks_the_anchor() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // A Killer Bee Nest (aux repr 4280) is a 400k damage-summary with
        // killed=false; the Queen is the real boss. Header/icon + completion must
        // come from the Queen, not the bigger-HP aux row.
        db.insert_fight(&grouped_fight(
            "The Nest",
            4280,
            "Yellow Killer Bee Nest",
            "run-y",
            1_000,
            false,
            400_000,
        ))
        .unwrap();
        db.insert_fight(&grouped_fight(
            "The Nest",
            4000,
            "Killer Bee Queen",
            "run-y",
            30_000,
            true,
            112_500,
        ))
        .unwrap();

        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 1);
        let g = &list[0];
        assert_eq!(g.boss_object_type, 4000); // the Queen, not the 400k nest
        assert_eq!(g.boss_max_hp, 112_500);
        assert!(g.killed); // Queen killed -> run counts as complete
    }

    #[test]
    fn realm_encounter_headlined_by_encounter_name() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // World's Oyster (34538) fought in the open Realm with its Pearl summary
        // (aux 34546). The card is titled by the encounter, not "Realm".
        db.insert_fight(&grouped_fight(
            "Realm",
            34546,
            "World's Pearl",
            "run-o",
            1_000,
            false,
            90_000,
        ))
        .unwrap();
        db.insert_fight(&grouped_fight(
            "Realm",
            34538,
            "World's Oyster",
            "run-o",
            5_000,
            true,
            250_000,
        ))
        .unwrap();

        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 1);
        let g = &list[0];
        assert_eq!(g.boss_name, "World's Oyster");
        assert_eq!(g.boss_object_type, 34538); // anchor is the Oyster, not the Pearl
        assert!(g.killed);
    }

    #[test]
    fn syndicate_mercenary_never_hijacks_the_anchor() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // The Hive's Queen Bee (4000 stand-in name) is the real boss; a Syndicate
        // Takeover mercenary (Pirate Queen Ramm, 49708) spawns and dies *after*
        // the Queen. The portrait/anchor must stay the Queen, though the boss
        // line still lists both.
        db.insert_fight(&grouped_fight(
            "The Hive",
            4000,
            "Queen Bee",
            "run-hive",
            1_000,
            true,
            90_000,
        ))
        .unwrap();
        db.insert_fight(&grouped_fight(
            "The Hive",
            49708,
            "Pirate Queen Ramm",
            "run-hive",
            30_000,
            true,
            200_000,
        ))
        .unwrap();

        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 1);
        let g = &list[0];
        assert_eq!(
            g.boss_object_type, 4000,
            "Queen anchors the card, not the mercenary"
        );
        assert_eq!(g.boss_name, "The Hive");
        let names: Vec<&str> = g.killed_bosses.iter().map(|(_, n)| n.as_str()).collect();
        assert!(names.contains(&"Queen Bee"));
        assert!(names.contains(&"Pirate Queen Ramm"));
    }

    #[test]
    fn dimitus_never_hijacks_the_anchor() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // Dimitus (8813) spawns after the dungeon boss dies; it must not headline.
        db.insert_fight(&grouped_fight(
            "Davy Jones' Locker",
            1900,
            "Davy Jones",
            "run-dim",
            1_000,
            true,
            100_000,
        ))
        .unwrap();
        db.insert_fight(&grouped_fight(
            "Davy Jones' Locker",
            8813,
            "Dimitus",
            "run-dim",
            40_000,
            true,
            300_000,
        ))
        .unwrap();

        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        let g = &list[0];
        assert_eq!(
            g.boss_object_type, 1900,
            "the dungeon boss anchors over Dimitus"
        );
    }

    #[test]
    fn killer_bee_nest_killed_if_any_beehemoth() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // The event hive (4312) despawns un-killed but a Beehemoth (4324) dies;
        // the card counts as killed and keeps the hive as its icon anchor.
        db.insert_fight(&grouped_fight(
            "Realm",
            4312,
            "Killer Bee Nest",
            "run-b",
            1_000,
            false,
            200_000,
        ))
        .unwrap();
        db.insert_fight(&grouped_fight(
            "Realm",
            4324,
            "Yellow Beehemoth",
            "run-b",
            5_000,
            true,
            60_000,
        ))
        .unwrap();

        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        let g = &list[0];
        assert_eq!(g.boss_name, "Killer Bee Nest");
        assert_eq!(g.boss_object_type, 4312); // hive is the declared anchor icon
        assert!(g.killed, "any Beehemoth kill completes the nest");
    }

    #[test]
    fn pentaract_towers_group_and_complete_on_any_tower() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // Two towers as separate rows in one Pentaract card; one killed -> done.
        db.insert_fight(&grouped_fight(
            "Realm",
            22018,
            "Pentaract Tower",
            "run-p",
            1_000,
            false,
            40_000,
        ))
        .unwrap();
        db.insert_fight(&grouped_fight(
            "Realm",
            22018,
            "Pentaract Tower",
            "run-p",
            2_000,
            true,
            40_000,
        ))
        .unwrap();

        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        let g = &list[0];
        assert_eq!(g.boss_name, "Pentaract");
        assert_eq!(g.phase_count, 2);
        assert!(g.killed);
    }

    #[test]
    fn assembled_giant_completes_on_any_part() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // Head killed but Body escaped -> the card still completes, because the
        // encounter is killed when any of its parts was killed.
        db.insert_fight(&grouped_fight(
            "Realm",
            34480,
            "Assembled Giant",
            "run-g",
            1_000,
            true,
            50_000,
        ))
        .unwrap();
        db.insert_fight(&grouped_fight(
            "Realm",
            34485,
            "Assembled Giant",
            "run-g",
            5_000,
            false,
            120_000,
        ))
        .unwrap();

        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        let g = &list[0];
        assert_eq!(g.boss_name, "Assembled Giant");
        assert_eq!(g.boss_object_type, 34485); // Body anchor drives the icon
        assert!(g.killed, "card completes when any part was killed");
    }

    #[test]
    fn flying_behemoth_egg_never_hijacks_anchor() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // The Egg (20799) ends later than the Behemoth (20744) but must not become
        // the anchor icon/completion: the declared anchor is the Behemoth.
        db.insert_fight(&grouped_fight(
            "Realm",
            20744,
            "Flying Behemoth",
            "run-f",
            1_000,
            true,
            300_000,
        ))
        .unwrap();
        db.insert_fight(&grouped_fight(
            "Realm",
            20799,
            "Behemoth's Egg",
            "run-f",
            40_000,
            true,
            80_000,
        ))
        .unwrap();

        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        let g = &list[0];
        assert_eq!(g.boss_name, "Flying Behemoth");
        assert_eq!(g.boss_object_type, 20744); // Behemoth, not the later-ending Egg
        assert!(g.killed);
    }

    #[test]
    fn v4_to_v5_regroups_history_by_dungeon_instance() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE fights (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                started_at INTEGER NOT NULL, ended_at INTEGER NOT NULL,
                dungeon TEXT NOT NULL, map_seed INTEGER NOT NULL,
                boss_object_type INTEGER NOT NULL, boss_name TEXT NOT NULL,
                boss_max_hp INTEGER NOT NULL, boss_start_hp INTEGER NOT NULL,
                local_object_id INTEGER NOT NULL,
                local_char_id INTEGER NOT NULL DEFAULT 0, killed INTEGER NOT NULL,
                encounter_id TEXT, encounter_run_id TEXT
            );
            CREATE TABLE encounter_runs (
                run_id TEXT PRIMARY KEY, encounter_id TEXT NOT NULL, dungeon TEXT NOT NULL,
                started_at INTEGER NOT NULL, ended_at INTEGER NOT NULL, killed INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE fight_participants (
                id INTEGER PRIMARY KEY AUTOINCREMENT, fight_id INTEGER NOT NULL,
                object_id INTEGER NOT NULL, object_type INTEGER NOT NULL, name TEXT NOT NULL,
                equip0 INTEGER NOT NULL, equip1 INTEGER NOT NULL,
                equip2 INTEGER NOT NULL, equip3 INTEGER NOT NULL, enchants TEXT,
                damage INTEGER NOT NULL, hits INTEGER NOT NULL,
                is_local INTEGER NOT NULL, provenance TEXT NOT NULL
            );
            -- Two curated encounters from ONE instance (same char+seed, contiguous).
            INSERT INTO fights (started_at, ended_at, dungeon, map_seed, boss_object_type,
              boss_name, boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
              encounter_id, encounter_run_id)
              VALUES (1000, 2000, 'Oryx''s Sanctuary', 5, 6622, 'Leucoryx', 100, 100, 1, 9, 1,
                      'o3_leucoryx', 'bf-o3_leucoryx-3e8');
            INSERT INTO fights (started_at, ended_at, dungeon, map_seed, boss_object_type,
              boss_name, boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
              encounter_id, encounter_run_id)
              VALUES (3000, 4000, 'Oryx''s Sanctuary', 5, 45363, 'Oryx 3', 200, 200, 1, 9, 1,
                      'o3_oryx', 'bf-o3_oryx-bb8');
            INSERT INTO encounter_runs VALUES ('bf-o3_leucoryx-3e8','o3_leucoryx','Oryx''s Sanctuary',1000,2000,1);
            INSERT INTO encounter_runs VALUES ('bf-o3_oryx-bb8','o3_oryx','Oryx''s Sanctuary',3000,4000,1);
            -- Weak-evidence fight (char 0) in a groupable dungeon stays standalone.
            INSERT INTO fights (started_at, ended_at, dungeon, map_seed, boss_object_type,
              boss_name, boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
              encounter_id, encounter_run_id)
              VALUES (9000, 9500, 'Mad Lab', 7, 500, 'Legacy', 50, 50, 1, 0, 1, NULL, NULL);
            PRAGMA user_version = 4;
            "#,
        )
        .unwrap();

        let mut db = CombatDatabase { conn };
        db.initialize(None).unwrap();
        let version: i32 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        // The two curated encounters merged into ONE dungeon-instance run.
        let runs: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM encounter_runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(runs, 1);

        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        let group = list.iter().find(|s| s.is_encounter()).unwrap();
        assert_eq!(group.boss_name, "Oryx's Sanctuary");
        assert_eq!(group.phase_count, 2);
        // Anchor = last real boss of the run = Oryx 3.
        assert_eq!(group.boss_object_type, 45363);
        // The weak-evidence Mad Lab fight remains standalone.
        let standalone = list.iter().find(|s| !s.is_encounter()).unwrap();
        assert_eq!(standalone.boss_name, "Legacy");
    }

    #[test]
    fn v4_to_v5_never_merges_distinct_live_runs() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE fights (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                started_at INTEGER NOT NULL, ended_at INTEGER NOT NULL,
                dungeon TEXT NOT NULL, map_seed INTEGER NOT NULL,
                boss_object_type INTEGER NOT NULL, boss_name TEXT NOT NULL,
                boss_max_hp INTEGER NOT NULL, boss_start_hp INTEGER NOT NULL,
                local_object_id INTEGER NOT NULL,
                local_char_id INTEGER NOT NULL DEFAULT 0, killed INTEGER NOT NULL,
                encounter_id TEXT, encounter_run_id TEXT
            );
            CREATE TABLE encounter_runs (
                run_id TEXT PRIMARY KEY, encounter_id TEXT NOT NULL, dungeon TEXT NOT NULL,
                started_at INTEGER NOT NULL, ended_at INTEGER NOT NULL, killed INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE fight_participants (
                id INTEGER PRIMARY KEY AUTOINCREMENT, fight_id INTEGER NOT NULL,
                object_id INTEGER NOT NULL, object_type INTEGER NOT NULL, name TEXT NOT NULL,
                equip0 INTEGER NOT NULL, equip1 INTEGER NOT NULL,
                equip2 INTEGER NOT NULL, equip3 INTEGER NOT NULL, enchants TEXT,
                damage INTEGER NOT NULL, hits INTEGER NOT NULL,
                is_local INTEGER NOT NULL, provenance TEXT NOT NULL
            );
            -- Two DISTINCT live runs (same char+seed, recycled within 30 min).
            INSERT INTO fights (started_at, ended_at, dungeon, map_seed, boss_object_type,
              boss_name, boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
              encounter_id, encounter_run_id)
              VALUES (1000, 2000, 'The Nest', 5, 100, 'Queen A', 100, 100, 1, 9, 1,
                      'dungeon_run', '0000000000000001-1');
            INSERT INTO fights (started_at, ended_at, dungeon, map_seed, boss_object_type,
              boss_name, boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
              encounter_id, encounter_run_id)
              VALUES (2500, 3500, 'The Nest', 5, 100, 'Queen B', 100, 100, 1, 9, 1,
                      'dungeon_run', '0000000000000001-2');
            INSERT INTO encounter_runs VALUES ('0000000000000001-1','dungeon_run','The Nest',1000,2000,1);
            INSERT INTO encounter_runs VALUES ('0000000000000001-2','dungeon_run','The Nest',2500,3500,1);
            PRAGMA user_version = 4;
            "#,
        )
        .unwrap();

        let mut db = CombatDatabase { conn };
        db.initialize(None).unwrap();

        // The two distinct live instances are NOT merged.
        let runs: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM encounter_runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(runs, 2);
        assert_eq!(db.list_fights(&FightQuery::default(), 50).unwrap().len(), 2);
    }

    #[test]
    fn v2_to_v3_migration_keeps_rows_standalone() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE fights (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                started_at INTEGER NOT NULL, ended_at INTEGER NOT NULL,
                dungeon TEXT NOT NULL, map_seed INTEGER NOT NULL,
                boss_object_type INTEGER NOT NULL, boss_name TEXT NOT NULL,
                boss_max_hp INTEGER NOT NULL, boss_start_hp INTEGER NOT NULL,
                local_object_id INTEGER NOT NULL,
                local_char_id INTEGER NOT NULL DEFAULT 0, killed INTEGER NOT NULL
            );
            CREATE TABLE fight_participants (
                id INTEGER PRIMARY KEY AUTOINCREMENT, fight_id INTEGER NOT NULL,
                object_id INTEGER NOT NULL, object_type INTEGER NOT NULL, name TEXT NOT NULL,
                equip0 INTEGER NOT NULL, equip1 INTEGER NOT NULL,
                equip2 INTEGER NOT NULL, equip3 INTEGER NOT NULL,
                damage INTEGER NOT NULL, hits INTEGER NOT NULL,
                is_local INTEGER NOT NULL, provenance TEXT NOT NULL
            );
            INSERT INTO fights
              (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
               boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed)
              VALUES (1, 2, 'Old Lab', 7, 100, 'Legacy Boss', 500, 500, 9, 55, 1);
            PRAGMA user_version = 2;
            "#,
        )
        .unwrap();

        let mut db = CombatDatabase { conn };
        db.initialize(None).unwrap();
        let version: i32 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        // A lone dungeon boss now forms a single-phase run but still shows its
        // own name (only multi-boss runs adopt the dungeon headline).
        let list = db.list_fights(&FightQuery::default(), 10).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].boss_name, "Legacy Boss");
        assert_eq!(list[0].phase_count, 1);
    }

    #[test]
    fn v3_to_v4_migration_hydrates_legacy_participants_with_empty_enchants() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE fights (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                started_at INTEGER NOT NULL, ended_at INTEGER NOT NULL,
                dungeon TEXT NOT NULL, map_seed INTEGER NOT NULL,
                boss_object_type INTEGER NOT NULL, boss_name TEXT NOT NULL,
                boss_max_hp INTEGER NOT NULL, boss_start_hp INTEGER NOT NULL,
                local_object_id INTEGER NOT NULL,
                local_char_id INTEGER NOT NULL DEFAULT 0, killed INTEGER NOT NULL,
                encounter_id TEXT, encounter_run_id TEXT
            );
            CREATE TABLE encounter_runs (
                run_id TEXT PRIMARY KEY, encounter_id TEXT NOT NULL, dungeon TEXT NOT NULL,
                started_at INTEGER NOT NULL, ended_at INTEGER NOT NULL, killed INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE fight_participants (
                id INTEGER PRIMARY KEY AUTOINCREMENT, fight_id INTEGER NOT NULL,
                object_id INTEGER NOT NULL, object_type INTEGER NOT NULL, name TEXT NOT NULL,
                equip0 INTEGER NOT NULL, equip1 INTEGER NOT NULL,
                equip2 INTEGER NOT NULL, equip3 INTEGER NOT NULL,
                damage INTEGER NOT NULL, hits INTEGER NOT NULL,
                is_local INTEGER NOT NULL, provenance TEXT NOT NULL
            );
            INSERT INTO fights
              (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
               boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed)
              VALUES (1, 2, 'Old Lab', 7, 100, 'Legacy Boss', 500, 500, 9, 55, 1);
            INSERT INTO fight_participants
              (fight_id, object_id, object_type, name, equip0, equip1, equip2, equip3,
               damage, hits, is_local, provenance)
              VALUES (1, 42, 0x0321, 'Bob', 1, 2, 3, 4, 5000, 30, 0, 'observed');
            PRAGMA user_version = 3;
            "#,
        )
        .unwrap();

        let mut db = CombatDatabase { conn };
        db.initialize(None).unwrap();
        let version: i32 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        // Legacy participant hydrates with four empty enchant slots (NULL column).
        let detail = db.fight_detail(1).unwrap().unwrap();
        assert_eq!(detail.participants.len(), 1);
        let p = &detail.participants[0];
        assert_eq!(p.name, "Bob");
        assert_eq!(p.equipment, [1, 2, 3, 4]);
        assert_eq!(p.equipment_enchants, <[Vec<u16>; 4]>::default());

        // A fresh insert on the migrated DB persists and reads back enchants.
        let mut cf = sample_fight();
        cf.participants[0].equipment_enchants = [vec![7], vec![], vec![], vec![9, 8]];
        let id = db.insert_fight(&cf).unwrap();
        let round = db.fight_detail(id).unwrap().unwrap();
        assert_eq!(round.participants[0].equipment_enchants[0], vec![7]);
        assert_eq!(round.participants[0].equipment_enchants[3], vec![9, 8]);
    }

    #[test]
    fn enchant_serialization_round_trips() {
        let cases: [[Vec<u16>; 4]; 3] = [
            [vec![1, 2], vec![], vec![3], vec![]],
            Default::default(),
            [vec![], vec![100], vec![], vec![65535]],
        ];
        for c in &cases {
            let s = serialize_enchants(c);
            assert_eq!(&parse_enchants(Some(&s)), c);
        }
        // Legacy NULL / empty hydrate as four empty slots.
        assert_eq!(parse_enchants(None), <[Vec<u16>; 4]>::default());
        assert_eq!(parse_enchants(Some("")), <[Vec<u16>; 4]>::default());
    }

    #[test]
    fn insert_stores_boss_group_and_filters_by_it() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // Mad Lab (difficulty 4.0) -> Adept band.
        db.insert_fight(&sample_fight()).unwrap();
        let stored: Option<String> = db
            .conn
            .query_row("SELECT boss_group FROM fights LIMIT 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stored.as_deref(), Some("adept"));

        // Filtering by the matching group returns the fight; a different group
        // excludes it.
        let adept = FightQuery {
            groups: Some(vec!["adept".to_string()]),
            ..Default::default()
        };
        assert_eq!(db.list_fights(&adept, 50).unwrap().len(), 1);
        let expert = FightQuery {
            groups: Some(vec!["expert".to_string()]),
            ..Default::default()
        };
        assert_eq!(db.list_fights(&expert, 50).unwrap().len(), 0);
        // Empty group filter is a no-op (returns everything).
        let none = FightQuery {
            groups: Some(vec![]),
            ..Default::default()
        };
        assert_eq!(db.list_fights(&none, 50).unwrap().len(), 1);
    }

    #[test]
    fn v6_to_v7_backfills_boss_group() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE fights (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                started_at INTEGER NOT NULL, ended_at INTEGER NOT NULL,
                dungeon TEXT NOT NULL, map_seed INTEGER NOT NULL,
                boss_object_type INTEGER NOT NULL, boss_name TEXT NOT NULL,
                boss_max_hp INTEGER NOT NULL, boss_start_hp INTEGER NOT NULL,
                local_object_id INTEGER NOT NULL,
                local_char_id INTEGER NOT NULL DEFAULT 0, killed INTEGER NOT NULL,
                encounter_id TEXT, encounter_run_id TEXT
            );
            CREATE TABLE encounter_runs (
                run_id TEXT PRIMARY KEY, encounter_id TEXT NOT NULL, dungeon TEXT NOT NULL,
                started_at INTEGER NOT NULL, ended_at INTEGER NOT NULL, killed INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE fight_participants (
                id INTEGER PRIMARY KEY AUTOINCREMENT, fight_id INTEGER NOT NULL,
                object_id INTEGER NOT NULL, object_type INTEGER NOT NULL, name TEXT NOT NULL,
                equip0 INTEGER NOT NULL, equip1 INTEGER NOT NULL,
                equip2 INTEGER NOT NULL, equip3 INTEGER NOT NULL, enchants TEXT,
                damage INTEGER NOT NULL, hits INTEGER NOT NULL,
                is_local INTEGER NOT NULL, provenance TEXT NOT NULL
            );
            INSERT INTO fights
              (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
               boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed)
              VALUES (1, 2, 'The Shatters', 7, 100, 'The Forgotten King', 500, 500, 9, 55, 1);
            PRAGMA user_version = 6;
            "#,
        )
        .unwrap();

        let mut db = CombatDatabase { conn };
        db.initialize(None).unwrap();
        let version: i32 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        // The Shatters (10.0) backfills to the Exaltation band.
        let group: Option<String> = db
            .conn
            .query_row("SELECT boss_group FROM fights LIMIT 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(group.as_deref(), Some("exaltation"));
    }

    #[test]
    fn v23_to_v24_purges_mv_non_bosses_and_reclassifies_fishing_loot() {
        let mut db = CombatDatabase {
            conn: Connection::open_in_memory().unwrap(),
        };
        db.initialize(None).unwrap();
        // A Moonlight Village run recorded before the fix: a Challenge Gate and
        // MV Easy Dropper (now curated non-bosses), a dancer, and Fishing Loot.
        db.conn
            .execute_batch(
                r#"
            INSERT INTO encounter_runs (run_id, encounter_id, dungeon, started_at, ended_at, killed)
              VALUES ('mv-1', 'dungeon_run', 'Moonlight Village', 1, 100, 0);
            INSERT INTO fights
              (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
               boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
               encounter_run_id, boss_group)
              VALUES
              (1, 100, 'Moonlight Village', 7, 20553, 'Challenge Gate', 500000, 500000, 9, 55, 0, 'mv-1', NULL),
              (2, 10, 'Moonlight Village', 7, 20577, 'MV Easy Dropper', 120000, 120000, 9, 55, 0, 'mv-1', NULL),
              (5, 50, 'Moonlight Village', 7, 20450, 'Sage Genji', 360000, 360000, 9, 55, 0, 'mv-1', NULL),
              (6, 40, 'Moonlight Village', 7, 20789, 'MV Fishing Loot 1', 90000, 90000, 9, 55, 1, 'mv-1', NULL);
            "#,
            )
            .unwrap();
        let gate_id: i64 = db
            .conn
            .query_row(
                "SELECT id FROM fights WHERE boss_object_type = 20553",
                [],
                |r| r.get(0),
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO fight_participants
                 (fight_id, object_id, object_type, name, equip0, equip1, equip2, equip3,
                  damage, hits, is_local, provenance)
                 VALUES (?1, 1, 0, 'Lovens', 0, 0, 0, 0, 100, 5, 1, 'self-computed')",
                params![gate_id],
            )
            .unwrap();

        // Re-run migrations from v23.
        db.conn.execute_batch("PRAGMA user_version = 23").unwrap();
        db.initialize(None).unwrap();
        let version: i32 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        // Gate + Dropper fights and their participants are purged.
        let non_boss: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fights WHERE boss_object_type IN (20553, 20577)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(non_boss, 0, "curated non-boss fights purged");
        let orphan_parts: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fight_participants WHERE fight_id = ?1",
                params![gate_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphan_parts, 0, "purged fight participants removed");

        // The run survives (dancer + fishing remain) and Fishing Loot is a crate.
        let run_exists: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM encounter_runs WHERE run_id = 'mv-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(run_exists, 1, "run with surviving phases kept");
        // The run span is recomputed from surviving phases (the purged Gate had
        // defined both the earliest start and latest end).
        let (start, end): (i64, i64) = db
            .conn
            .query_row(
                "SELECT started_at, ended_at FROM encounter_runs WHERE run_id = 'mv-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (start, end),
            (5, 50),
            "run span recomputed from surviving phases"
        );
        let loot_group: Option<String> = db
            .conn
            .query_row(
                "SELECT boss_group FROM fights WHERE boss_object_type = 20789",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            loot_group.as_deref(),
            Some("treasure_crate"),
            "Fishing Loot reclassified as crate"
        );
    }

    #[test]
    fn v24_to_v25_purges_encounter_minion_spam() {
        let mut db = CombatDatabase {
            conn: Connection::open_in_memory().unwrap(),
        };
        db.initialize(None).unwrap();
        // A Lich King run recorded before the fix: the real boss plus Lich
        // King Grave adds (34587) that were tracked as their own fights, one of
        // which defines the run's earliest start and latest end.
        db.conn
            .execute_batch(
                r#"
            INSERT INTO encounter_runs (run_id, encounter_id, dungeon, started_at, ended_at, killed)
              VALUES ('lk-1', 'dungeon_run', 'Undead Lair', 1, 200, 1);
            INSERT INTO fights
              (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
               boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
               encounter_run_id, boss_group)
              VALUES
              (1, 200, 'Undead Lair', 7, 34587, 'Lich King Grave', 5000, 5000, 9, 55, 0, 'lk-1', NULL),
              (30, 30, 'Undead Lair', 7, 34587, 'Lich King Grave', 5000, 5000, 9, 55, 0, 'lk-1', NULL),
              (50, 150, 'Undead Lair', 7, 34584, 'The Lich King', 300000, 300000, 9, 55, 1, 'lk-1', NULL);
            "#,
            )
            .unwrap();
        let grave_id: i64 = db
            .conn
            .query_row(
                "SELECT id FROM fights WHERE boss_object_type = 34587 LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO fight_participants
                 (fight_id, object_id, object_type, name, equip0, equip1, equip2, equip3,
                  damage, hits, is_local, provenance)
                 VALUES (?1, 1, 0, 'Lovens', 0, 0, 0, 0, 100, 5, 1, 'self-computed')",
                params![grave_id],
            )
            .unwrap();

        // Re-run migrations from v24.
        db.conn.execute_batch("PRAGMA user_version = 24").unwrap();
        db.initialize(None).unwrap();
        let version: i32 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        // The spam-add fights and their participants are purged.
        let spam: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fights WHERE boss_object_type = 34587",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(spam, 0, "curated non-boss add fights purged");
        let orphan_parts: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fight_participants WHERE fight_id = ?1",
                params![grave_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphan_parts, 0, "purged fight participants removed");

        // The run survives (the real boss remains) and its span is recomputed
        // from the surviving phase (the purged add had defined start and end).
        let run_exists: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM encounter_runs WHERE run_id = 'lk-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(run_exists, 1, "run with surviving boss kept");
        let (start, end): (i64, i64) = db
            .conn
            .query_row(
                "SELECT started_at, ended_at FROM encounter_runs WHERE run_id = 'lk-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (start, end),
            (50, 150),
            "run span recomputed from surviving boss"
        );
        // The real boss is untouched.
        let boss: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fights WHERE boss_object_type = 34584",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(boss, 1, "real boss fight preserved");
    }

    #[test]
    fn v28_to_v29_purges_baneserpent_segments() {
        let mut db = CombatDatabase {
            conn: Connection::open_in_memory().unwrap(),
        };
        db.initialize(None).unwrap();
        // A Realm visit recorded before the fix: the real Adult Baneserpent
        // body (34456, killed) plus a spurious head segment (34459) that shares
        // the DisplayId "Adult Baneserpent" but was fought without the body in
        // view and logged as its own Escaped card.
        db.conn
            .execute_batch(
                r#"
            INSERT INTO fights
              (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
               boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
               encounter_run_id, boss_group)
              VALUES
              (10, 140, 'Realm', 0, 34459, 'Adult Baneserpent', 90, 90, 9, 55, 0, NULL, NULL),
              (150, 155, 'Realm', 0, 34456, 'Adult Baneserpent', 130, 130, 9, 55, 1, NULL, NULL);
            "#,
            )
            .unwrap();
        let head_id: i64 = db
            .conn
            .query_row(
                "SELECT id FROM fights WHERE boss_object_type = 34459 LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO fight_participants
                 (fight_id, object_id, object_type, name, equip0, equip1, equip2, equip3,
                  damage, hits, is_local, provenance)
                 VALUES (?1, 1, 0, 'Lovens', 0, 0, 0, 0, 100, 5, 1, 'self-computed')",
                params![head_id],
            )
            .unwrap();

        // Re-run migrations from v28.
        db.conn.execute_batch("PRAGMA user_version = 28").unwrap();
        db.initialize(None).unwrap();
        let version: i32 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        // Every non-body segment row and its participants are purged.
        let segments: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fights WHERE boss_object_type IN (34457, 34458, 34459, 34467, 34468, 34469, 34474)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(segments, 0, "Baneserpent segment fights purged");
        let orphan_parts: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fight_participants WHERE fight_id = ?1",
                params![head_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphan_parts, 0, "purged segment participants removed");
        // The real body card survives untouched.
        let body: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fights WHERE boss_object_type = 34456",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(body, 1, "Adult Baneserpent body fight preserved");
    }

    #[test]
    fn v37_to_v38_purges_specpen_tutorial_objects() {
        let mut db = CombatDatabase {
            conn: Connection::open_in_memory().unwrap(),
        };
        db.initialize(None).unwrap();
        // A Spectral Penitentiary run recorded before the fix: a CellBranch
        // tutorial object (24456) and an Administration Turret (24061) tracked as
        // spurious cards, plus the real Murcian Overseer boss (23508, killed).
        db.conn
            .execute_batch(
                r#"
            INSERT INTO fights
              (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
               boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
               encounter_run_id, boss_group)
              VALUES
              (10, 140, 'Spectral Penitentiary', 0, 24456, 'SpecPen CellBranch Tutorial Object', 11000, 11000, 9, 55, 0, NULL, NULL),
              (12, 130, 'Spectral Penitentiary', 0, 24061, 'Administration Turret', 15080, 15080, 9, 55, 1, NULL, NULL),
              (150, 155, 'Spectral Penitentiary', 0, 23508, 'Murcian Overseer', 5000000, 5000000, 9, 55, 1, NULL, NULL);
            "#,
            )
            .unwrap();
        let tutorial_id: i64 = db
            .conn
            .query_row(
                "SELECT id FROM fights WHERE boss_object_type = 24456 LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO fight_participants
                 (fight_id, object_id, object_type, name, equip0, equip1, equip2, equip3,
                  damage, hits, is_local, provenance)
                 VALUES (?1, 1, 0, 'Lovens', 0, 0, 0, 0, 100, 5, 1, 'self-computed')",
                params![tutorial_id],
            )
            .unwrap();

        // Re-run migrations from v37.
        db.conn.execute_batch("PRAGMA user_version = 37").unwrap();
        db.initialize(None).unwrap();
        let version: i32 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        // Every tutorial-object and turret row and its participants are purged.
        let tutorial: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fights WHERE boss_object_type IN (24061, 24454, 24455, 24456, 24457, 24458, 24459)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tutorial, 0, "SpecPen tutorial and turret fights purged");
        let orphan_parts: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fight_participants WHERE fight_id = ?1",
                params![tutorial_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphan_parts, 0, "purged tutorial participants removed");
        // The real boss card survives untouched.
        let boss: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fights WHERE boss_object_type = 23508",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(boss, 1, "Murcian Overseer fight preserved");
    }

    #[test]
    fn v38_to_v39_adds_aux_member_count_column() {
        let mut db = CombatDatabase {
            conn: Connection::open_in_memory().unwrap(),
        };
        // A pre-v39 fights table lacks the aux_member_count column.
        db.conn
            .execute_batch(
                r#"
            CREATE TABLE fights (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                started_at INTEGER NOT NULL, ended_at INTEGER NOT NULL,
                dungeon TEXT NOT NULL, map_seed INTEGER NOT NULL,
                boss_object_type INTEGER NOT NULL, boss_name TEXT NOT NULL,
                boss_max_hp INTEGER NOT NULL, boss_start_hp INTEGER NOT NULL,
                local_object_id INTEGER NOT NULL, local_char_id INTEGER NOT NULL DEFAULT 0,
                killed INTEGER NOT NULL, encounter_id TEXT, encounter_run_id TEXT,
                boss_group TEXT, local_close_calls INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE encounter_runs (
                run_id TEXT PRIMARY KEY, encounter_id TEXT, dungeon TEXT,
                started_at INTEGER, ended_at INTEGER, killed INTEGER
            );
            CREATE TABLE fight_participants (fight_id INTEGER, object_id INTEGER);
            PRAGMA user_version = 38;
            "#,
            )
            .unwrap();
        assert!(!db.column_exists("fights", "aux_member_count").unwrap());

        db.initialize(None).unwrap();

        let version: i32 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        assert!(
            db.column_exists("fights", "aux_member_count").unwrap(),
            "aux_member_count column added by migration"
        );
    }

    #[test]
    fn v39_to_v40_purges_turret_left_behind_by_v38() {
        let mut db = CombatDatabase {
            conn: Connection::open_in_memory().unwrap(),
        };
        db.initialize(None).unwrap();
        // A database that reached v39 during earlier testing still holds an
        // Administration Turret (24061) row because the v38 purge gained that type
        // only after this database had already advanced past v38.
        db.conn
            .execute_batch(
                r#"
            INSERT INTO fights
              (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
               boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
               encounter_run_id, boss_group)
              VALUES
              (12, 130, 'Spectral Penitentiary', 0, 24061, 'Administration Turret', 15080, 15080, 9, 55, 1, NULL, NULL),
              (150, 155, 'Spectral Penitentiary', 0, 23508, 'Murcian Overseer', 5000000, 5000000, 9, 55, 1, NULL, NULL);
            "#,
            )
            .unwrap();

        // Re-run migrations from v39.
        db.conn.execute_batch("PRAGMA user_version = 39").unwrap();
        db.initialize(None).unwrap();
        let version: i32 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        let turret: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fights WHERE boss_object_type = 24061",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(turret, 0, "Administration Turret purged by v40");
        let boss: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fights WHERE boss_object_type = 23508",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(boss, 1, "Murcian Overseer fight preserved");
    }

    #[test]
    fn v40_to_v41_purges_specpen_switches_and_gravestones() {
        let mut db = CombatDatabase {
            conn: Connection::open_in_memory().unwrap(),
        };
        db.initialize(None).unwrap();
        // A Spectral Penitentiary run recorded before the fix: a branch Door
        // Switch (23840), a colored Torture floor switch (44357) and a Gretch
        // gravestone (23708) all logged as spurious cards via the HP fallback,
        // plus the real Griefkeeper Zole boss (23659, killed). A Sentipede body
        // segment (23934) is also present and must NOT be purged.
        db.conn
            .execute_batch(
                r#"
            INSERT INTO fights
              (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
               boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
               encounter_run_id, boss_group)
              VALUES
              (10, 30, 'Spectral Penitentiary', 0, 23840, 'Doctor Lobotomik', 10000, 10000, 9, 55, 0, NULL, NULL),
              (12, 32, 'Spectral Penitentiary', 0, 44357, 'Doctor Lobotomik', 10000, 10000, 9, 55, 0, NULL, NULL),
              (14, 34, 'Spectral Penitentiary', 0, 23708, 'Griefkeeper Zole', 10000, 10000, 9, 55, 0, NULL, NULL),
              (40, 60, 'Spectral Penitentiary', 0, 23934, 'Doctor Lobotomik', 43875, 43875, 9, 55, 0, NULL, NULL),
              (150, 155, 'Spectral Penitentiary', 0, 23659, 'Griefkeeper Zole', 1721250, 1721250, 9, 55, 1, NULL, NULL);
            "#,
            )
            .unwrap();
        let switch_id: i64 = db
            .conn
            .query_row(
                "SELECT id FROM fights WHERE boss_object_type = 23840 LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO fight_participants
                 (fight_id, object_id, object_type, name, equip0, equip1, equip2, equip3,
                  damage, hits, is_local, provenance)
                 VALUES (?1, 1, 0, 'Lovens', 0, 0, 0, 0, 100, 5, 1, 'self-computed')",
                params![switch_id],
            )
            .unwrap();

        // Re-run migrations from v40.
        db.conn.execute_batch("PRAGMA user_version = 40").unwrap();
        db.initialize(None).unwrap();
        let version: i32 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        let props: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fights WHERE boss_object_type IN
                 (23708, 23747, 23840, 23932, 24412, 44354, 44355, 44356, 44357, 44358, 44359, 44360, 44361)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(props, 0, "SpecPen switch and gravestone fights purged");
        let orphan_parts: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fight_participants WHERE fight_id = ?1",
                params![switch_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphan_parts, 0, "purged switch participants removed");
        // The real boss and the Sentipede segment survive (the segment folds at
        // display time, so its damage must be preserved).
        let boss: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fights WHERE boss_object_type = 23659",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(boss, 1, "Griefkeeper Zole fight preserved");
        let segment: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fights WHERE boss_object_type = 23934",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            segment, 1,
            "Sentipede body segment preserved for display-time fold"
        );
    }

    #[test]
    fn v30_to_v31_purges_city_rat_minions() {
        let mut db = CombatDatabase {
            conn: Connection::open_in_memory().unwrap(),
        };
        db.initialize(None).unwrap();
        // A Rat Extermination visit recorded before the fix: the real Mammoth
        // City Rat (18007) plus event-scaled minion rats (18000, 18047) that were
        // logged as their own spam cards.
        db.conn
            .execute_batch(
                r#"
            INSERT INTO fights
              (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
               boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
               encounter_run_id, boss_group)
              VALUES
              (10, 20, 'Realm', 0, 18000, 'Giant City Rat', 12000, 12000, 9, 55, 1, NULL, NULL),
              (25, 30, 'Realm', 0, 18047, 'Special Small City Rat', 10000, 10000, 9, 55, 1, NULL, NULL),
              (40, 90, 'Realm', 0, 18007, 'Mammoth City Rat', 75000, 75000, 9, 55, 1, NULL, NULL);
            "#,
            )
            .unwrap();
        let minion_id: i64 = db
            .conn
            .query_row(
                "SELECT id FROM fights WHERE boss_object_type = 18000 LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO fight_participants
                 (fight_id, object_id, object_type, name, equip0, equip1, equip2, equip3,
                  damage, hits, is_local, provenance)
                 VALUES (?1, 1, 0, 'Lovens', 0, 0, 0, 0, 100, 5, 1, 'self-computed')",
                params![minion_id],
            )
            .unwrap();

        // Re-run migrations from v30.
        db.conn.execute_batch("PRAGMA user_version = 30").unwrap();
        db.initialize(None).unwrap();
        let version: i32 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        // Every minion rat row and its participants are purged.
        let minions: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fights WHERE boss_object_type IN (18000, 18003, 18047, 18049)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(minions, 0, "City Rat minion fights purged");
        let orphan_parts: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fight_participants WHERE fight_id = ?1",
                params![minion_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphan_parts, 0, "purged minion participants removed");
        // The Mammoth City Rat card survives untouched.
        let mammoth: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fights WHERE boss_object_type = 18007",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(mammoth, 1, "Mammoth City Rat fight preserved");
    }

    #[test]
    fn v31_to_v32_renames_marble_colossus_survival_segments() {
        let mut db = CombatDatabase {
            conn: Connection::open_in_memory().unwrap(),
        };
        db.initialize(None).unwrap();
        // A Lost Halls run recorded before the fix, with the old segment names.
        db.conn
            .execute_batch(
                r#"
            INSERT INTO fights
              (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
               boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
               encounter_run_id, boss_group)
              VALUES
              (10, 20, 'Lost Halls', 0, 45073, 'Marble Colossus (First Coming)', 100, 100, 9, 55, 1, NULL, NULL),
              (25, 40, 'Lost Halls', 0, 45073, 'Marble Colossus (Second Coming)', 100, 100, 9, 55, 1, NULL, NULL);
            "#,
            )
            .unwrap();

        // Re-run migrations from v31.
        db.conn.execute_batch("PRAGMA user_version = 31").unwrap();
        db.initialize(None).unwrap();
        let version: i32 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        let names: Vec<String> = {
            let mut stmt = db
                .conn
                .prepare("SELECT boss_name FROM fights WHERE boss_object_type = 45073 ORDER BY started_at")
                .unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
            rows.map(|r| r.unwrap()).collect()
        };
        assert_eq!(
            names,
            vec![
                "Marble Colossus (Pre-survival)".to_string(),
                "Marble Colossus (Post-survival)".to_string(),
            ],
            "old survival-phase labels renamed"
        );
    }

    #[test]
    fn v45_to_v46_relabels_collapsed_marble_colossus_full_kills() {
        let mut db = CombatDatabase {
            conn: Connection::open_in_memory().unwrap(),
        };
        db.initialize(None).unwrap();
        // Legacy MBC cards recorded before the taunt-driven split:
        //  - run A: a collapsed full-kill whose split never fired, local player
        //    SURVIVED (end_status 'present'), no Post-survival sibling -> relabel.
        //  - run B: a genuine pre-survival segment that DOES have a Post-survival
        //    sibling -> keep both labels.
        //  - run C: an abandoned pre-survival phase (killed = 0) -> keep label.
        //  - run D: collapsed and killed = 1, but the local player DIED in the
        //    fight -> abandoned, not a full kill -> keep the (Pre-survival) label.
        db.conn
            .execute_batch(
                r#"
            INSERT INTO fights
              (id, started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
               boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
               encounter_run_id, boss_group)
              VALUES
              (1, 10, 20, 'Lost Halls', 0, 45073, 'Marble Colossus (Pre-survival)', 100, 100, 9, 55, 1, 'A', NULL),
              (2, 30, 40, 'Lost Halls', 0, 45073, 'Marble Colossus (Pre-survival)', 100, 100, 9, 55, 1, 'B', NULL),
              (3, 45, 55, 'Lost Halls', 0, 45073, 'Marble Colossus (Post-survival)', 100, 100, 9, 55, 1, 'B', NULL),
              (4, 60, 70, 'Lost Halls', 0, 45073, 'Marble Colossus (Pre-survival)', 100, 100, 9, 55, 0, 'C', NULL),
              (5, 80, 90, 'Lost Halls', 0, 45073, 'Marble Colossus (Pre-survival)', 100, 100, 9, 55, 1, 'D', NULL);
            INSERT INTO fight_participants
              (fight_id, object_id, object_type, name, equip0, equip1, equip2, equip3,
               damage, hits, is_local, provenance, end_status)
              VALUES
              (1, 9, 0x0300, 'Me', -1, -1, -1, -1, 100, 5, 1, 'observed', 'present'),
              (2, 9, 0x0300, 'Me', -1, -1, -1, -1, 100, 5, 1, 'observed', 'present'),
              (3, 9, 0x0300, 'Me', -1, -1, -1, -1, 100, 5, 1, 'observed', 'present'),
              (4, 9, 0x0300, 'Me', -1, -1, -1, -1, 100, 5, 1, 'observed', 'present'),
              (5, 9, 0x0300, 'Me', -1, -1, -1, -1, 100, 5, 1, 'observed', 'died');
            "#,
            )
            .unwrap();

        db.conn.execute_batch("PRAGMA user_version = 45").unwrap();
        db.initialize(None).unwrap();
        let version: i32 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        let names: Vec<(String, String)> = {
            let mut stmt = db
                .conn
                .prepare(
                    "SELECT encounter_run_id, boss_name FROM fights \
                     WHERE boss_object_type = 45073 ORDER BY started_at",
                )
                .unwrap();
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .unwrap();
            rows.map(|r| r.unwrap()).collect()
        };
        assert_eq!(
            names,
            vec![
                ("A".to_string(), "Marble Colossus".to_string()),
                (
                    "B".to_string(),
                    "Marble Colossus (Pre-survival)".to_string()
                ),
                (
                    "B".to_string(),
                    "Marble Colossus (Post-survival)".to_string()
                ),
                (
                    "C".to_string(),
                    "Marble Colossus (Pre-survival)".to_string()
                ),
                (
                    "D".to_string(),
                    "Marble Colossus (Pre-survival)".to_string()
                ),
            ],
            "only the collapsed full-kill the local player survived drops its suffix"
        );
    }

    #[test]
    fn v46_to_v47_purges_legacy_shatters_and_woodland_trash() {
        let mut db = CombatDatabase {
            conn: Connection::open_in_memory().unwrap(),
        };
        db.initialize(None).unwrap();
        // A Legacy The Shatters run: three real bosses (Forgotten Sentinel 52556,
        // Twilight Archmage 52557, the King 52560), a kept aux bird (Blizzard
        // 52564) and trash (Royal Guardian 52558). A Legacy Woodland Labyrinth
        // run: the three Megamoth forms (52662/52663/52665) plus a Mecha Squirrel
        // (52658) that must be purged.
        db.conn
            .execute_batch(
                r#"
            INSERT INTO fights
              (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
               boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
               encounter_run_id, boss_group)
              VALUES
              (10, 40, 'Legacy The Shatters', 7, 52556, 'The Forgotten Sentinel', 50000, 50000, 9, 55, 1, 'shtrs', NULL),
              (50, 90, 'Legacy The Shatters', 7, 52557, 'Twilight Archmage', 113460, 113460, 9, 55, 1, 'shtrs', NULL),
              (55, 85, 'Legacy The Shatters', 7, 52564, 'Blizzard', 15000, 15000, 9, 55, 1, 'shtrs', NULL),
              (60, 80, 'Legacy The Shatters', 7, 52558, 'Royal Guardian', 14640, 14640, 9, 55, 0, 'shtrs', NULL),
              (95, 100, 'Legacy The Shatters', 7, 52560, 'The Forgotten King', 146400, 146400, 9, 55, 1, 'shtrs', NULL),
              (200, 230, 'Legacy Woodland Labyrinth', 3, 52662, 'Retro Megamoth Larva', 124920, 124920, 9, 55, 0, 'wlab', NULL),
              (210, 240, 'Legacy Woodland Labyrinth', 3, 52663, 'Retro Mammoth Megamoth', 124920, 124920, 9, 55, 0, 'wlab', NULL),
              (205, 235, 'Legacy Woodland Labyrinth', 3, 52658, 'Mecha Squirrel', 10000, 10000, 9, 55, 0, 'wlab', NULL),
              (245, 250, 'Legacy Woodland Labyrinth', 3, 52665, 'Retro Murderous Megamoth', 83280, 83280, 9, 55, 1, 'wlab', NULL);
            "#,
            )
            .unwrap();
        db.conn
            .execute_batch(
                r#"
            INSERT INTO encounter_runs (run_id, encounter_id, dungeon, started_at, ended_at, killed)
              VALUES ('shtrs', 'dungeon_run', 'Legacy The Shatters', 10, 100, 1),
                     ('wlab', 'dungeon_run', 'Legacy Woodland Labyrinth', 200, 250, 1);
            "#,
            )
            .unwrap();

        // Re-run migrations from v46.
        db.conn.execute_batch("PRAGMA user_version = 46").unwrap();
        db.initialize(None).unwrap();
        let version: i32 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        // Trash purged: Royal Guardian, Mecha Squirrel (the King 52560 survives).
        let trash: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fights WHERE boss_object_type IN (52558, 52658)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(trash, 0, "legacy Shatters/Woodland trash purged");

        // Real bosses (incl. the King 52560) and the kept Blizzard aux row survive.
        let kept: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fights WHERE boss_object_type IN
                 (52556, 52557, 52560, 52564, 52662, 52663, 52665)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept, 7, "real bosses + kept Blizzard aux preserved");

        // Run span keeps the King's ended_at=100 (the King is no longer purged).
        let shtrs_span: (i64, i64) = db
            .conn
            .query_row(
                "SELECT started_at, ended_at FROM encounter_runs WHERE run_id = 'shtrs'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(shtrs_span, (10, 100));
    }

    #[test]
    fn v49_to_v50_purges_galleon_admiral() {
        let mut db = CombatDatabase {
            conn: Connection::open_in_memory().unwrap(),
        };
        db.initialize(None).unwrap();
        // A Bilgewater's Galleon run: the tracked boss (20475) plus a Galleon
        // Admiral add (34465) that must be purged.
        db.conn
            .execute_batch(
                r#"
            INSERT INTO fights
              (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
               boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
               encounter_run_id, boss_group)
              VALUES
              (10, 40, 'Bilgewater''s Galleon', 7, 20475, 'Bilgewater''s Galleon', 70000, 70000, 9, 55, 1, NULL, NULL),
              (20, 35, 'Bilgewater''s Galleon', 7, 34465, 'Galleon Admiral', 30000, 30000, 9, 55, 0, NULL, NULL);
            "#,
            )
            .unwrap();

        db.conn.execute_batch("PRAGMA user_version = 49").unwrap();
        db.initialize(None).unwrap();
        let version: i32 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        let admiral: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fights WHERE boss_object_type = 34465",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(admiral, 0, "Galleon Admiral add purged");
        let boss: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fights WHERE boss_object_type = 20475",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(boss, 1, "Bilgewater's Galleon boss preserved");
    }

    #[test]
    fn v8_to_v9_adds_skin_and_dye_columns() {
        let conn = Connection::open_in_memory().unwrap();
        // A v8 schema: fight_participants lacks skin_id/tex1/tex2.
        conn.execute_batch(
            r#"
            CREATE TABLE fights (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                started_at INTEGER NOT NULL, ended_at INTEGER NOT NULL,
                dungeon TEXT NOT NULL, map_seed INTEGER NOT NULL,
                boss_object_type INTEGER NOT NULL, boss_name TEXT NOT NULL,
                boss_max_hp INTEGER NOT NULL, boss_start_hp INTEGER NOT NULL,
                local_object_id INTEGER NOT NULL,
                local_char_id INTEGER NOT NULL DEFAULT 0, killed INTEGER NOT NULL,
                encounter_id TEXT, encounter_run_id TEXT, boss_group TEXT
            );
            CREATE TABLE encounter_runs (
                run_id TEXT PRIMARY KEY, encounter_id TEXT NOT NULL, dungeon TEXT NOT NULL,
                started_at INTEGER NOT NULL, ended_at INTEGER NOT NULL, killed INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE fight_participants (
                id INTEGER PRIMARY KEY AUTOINCREMENT, fight_id INTEGER NOT NULL,
                object_id INTEGER NOT NULL, object_type INTEGER NOT NULL, name TEXT NOT NULL,
                equip0 INTEGER NOT NULL, equip1 INTEGER NOT NULL,
                equip2 INTEGER NOT NULL, equip3 INTEGER NOT NULL, enchants TEXT,
                damage INTEGER NOT NULL, hits INTEGER NOT NULL,
                is_local INTEGER NOT NULL, provenance TEXT NOT NULL
            );
            INSERT INTO fights
              (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
               boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed)
              VALUES (1, 2, 'Mad Lab', 7, 100, 'Dr Terrible', 500, 500, 9, 55, 1);
            INSERT INTO fight_participants
              (fight_id, object_id, object_type, name, equip0, equip1, equip2, equip3,
               enchants, damage, hits, is_local, provenance)
              VALUES (1, 600, 801, 'Alice', -1, -1, -1, -1, NULL, 100, 5, 0, 'Observed');
            PRAGMA user_version = 8;
            "#,
        )
        .unwrap();

        let mut db = CombatDatabase { conn };
        db.initialize(None).unwrap();
        let version: i32 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        // The new columns exist and old rows default to 0 (class sprite, no dye).
        assert!(db.column_exists("fight_participants", "skin_id").unwrap());
        assert!(db.column_exists("fight_participants", "tex1").unwrap());
        assert!(db.column_exists("fight_participants", "tex2").unwrap());
        let (skin, t1, t2): (i32, i32, i32) = db
            .conn
            .query_row(
                "SELECT skin_id, tex1, tex2 FROM fight_participants LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((skin, t1, t2), (0, 0, 0));

        // New inserts persist skin/dye through the migrated schema.
        db.insert_fight(&sample_fight()).unwrap();
        let f = db.recent_fights(10).unwrap();
        let alice = f
            .iter()
            .flat_map(|r| &r.participants)
            .find(|p| p.name == "Alice")
            .expect("Alice");
        assert_eq!(alice.skin_id, 5678);
        assert_eq!(alice.tex1, 0x0100_00ff);
        assert_eq!(alice.tex2, 0x0100_ff00);
    }

    #[test]
    fn dungeon_time_totals_accumulate_and_read_back() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        assert!(db.dungeon_time_totals().unwrap().is_empty());

        // Two Shatters runs (one completed), one incomplete Nest run.
        db.add_dungeon_time("The Shatters", 600_000, true).unwrap();
        db.add_dungeon_time("The Shatters", 400_000, true).unwrap();
        db.add_dungeon_time("The Nest", 120_000, false).unwrap();

        let mut totals = db.dungeon_time_totals().unwrap();
        totals.sort_by(|a, b| a.dungeon.cmp(&b.dungeon));
        assert_eq!(totals.len(), 2);

        let nest = totals.iter().find(|t| t.dungeon == "The Nest").unwrap();
        assert_eq!(nest.total_time_ms, 120_000);
        assert_eq!(nest.completions, 0);

        let shatters = totals.iter().find(|t| t.dungeon == "The Shatters").unwrap();
        assert_eq!(shatters.total_time_ms, 1_000_000);
        assert_eq!(shatters.completions, 2);
    }

    #[test]
    fn v42_to_v43_adds_dungeon_time_totals_table() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        // Simulate a v42 database missing the new counter table.
        db.conn
            .execute_batch("DROP TABLE dungeon_time_totals; PRAGMA user_version = 42;")
            .unwrap();
        db.migrate(42).unwrap();
        let version: i32 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        db.add_dungeon_time("The Shatters", 5_000, true).unwrap();
        assert_eq!(db.dungeon_time_totals().unwrap().len(), 1);
    }

    #[test]
    fn backfill_seeds_totals_from_legacy_history() {
        let db = CombatDatabase::open_in_memory().unwrap();
        // A grouped Shatters run, a standalone Pirate Cave fight, and a Realm
        // boss that must be excluded from the dungeon counter.
        db.conn
            .execute_batch(
                r#"
                INSERT INTO encounter_runs
                    (run_id, encounter_id, dungeon, started_at, ended_at, killed)
                    VALUES ('r1', 'dungeon_run', 'The Shatters', 1000, 5000, 1);
                INSERT INTO fights
                    (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
                     boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed)
                    VALUES (2000, 3000, 'Pirate Cave', 1, 10, 'Dreadstump', 100, 100, 1, 1, 1);
                INSERT INTO fights
                    (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
                     boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed)
                    VALUES (2000, 8000, 'Realm', 1, 11, 'Ghost King', 100, 100, 1, 1, 1);
                DELETE FROM dungeon_time_totals;
                "#,
            )
            .unwrap();
        db.backfill_dungeon_time_totals().unwrap();

        let totals = db.dungeon_time_totals().unwrap();
        assert_eq!(totals.len(), 2);
        assert!(totals.iter().all(|t| t.dungeon != "Realm"));
        let pc = totals.iter().find(|t| t.dungeon == "Pirate Cave").unwrap();
        assert_eq!(pc.total_time_ms, 1000);
        assert_eq!(pc.completions, 1);
        let sh = totals.iter().find(|t| t.dungeon == "The Shatters").unwrap();
        assert_eq!(sh.total_time_ms, 4000);
        assert_eq!(sh.completions, 1);
    }

    #[test]
    fn batch_delete_and_clear_all() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        let a = db.insert_fight(&sample_fight()).unwrap();
        let mut second = sample_fight();
        second.boss_name = "Second".to_string();
        let b = db.insert_fight(&second).unwrap();
        assert_eq!(db.fight_count().unwrap(), 2);

        let removed = db
            .delete_selections(&[FightSelection::Single(a), FightSelection::Single(b)])
            .unwrap();
        assert_eq!(removed, 2);
        assert_eq!(db.fight_count().unwrap(), 0);
        // Participant rows were removed alongside the fights.
        let orphans: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM fight_participants", [], |r| r.get(0))
            .unwrap();
        assert_eq!(orphans, 0);

        db.insert_fight(&sample_fight()).unwrap();
        db.clear_all().unwrap();
        assert_eq!(db.fight_count().unwrap(), 0);
    }

    #[test]
    fn selections_matching_preserves_list_filter_semantics() {
        let mut db = CombatDatabase::open_in_memory().unwrap();
        db.insert_fight(&sample_fight()).unwrap();
        let mut first_phase = sample_fight();
        first_phase.started_at = 10;
        first_phase.ended_at = 20;
        first_phase.boss_object_type = 100;
        first_phase.boss_name = "First Phase".to_string();
        first_phase.local_char_id = 1000;
        first_phase.encounter_id = Some("test_encounter".to_string());
        first_phase.encounter_run_id = Some("test-run".to_string());
        db.insert_fight(&first_phase).unwrap();

        let mut second_phase = first_phase.clone();
        second_phase.started_at = 30;
        second_phase.ended_at = 40;
        second_phase.boss_object_type = 200;
        second_phase.boss_name = "Second Phase".to_string();
        second_phase.local_char_id = 2000;
        db.insert_fight(&second_phase).unwrap();

        let assert_matches_list = |query: FightQuery| {
            let expected: Vec<FightSelection> = db
                .list_fights(&query, i64::MAX)
                .unwrap()
                .iter()
                .map(FightSummary::selection)
                .collect();
            assert_eq!(db.selections_matching(&query).unwrap(), expected);
        };

        assert_matches_list(FightQuery::default());
        assert_matches_list(FightQuery {
            groups: Some(vec!["adept".to_string()]),
            ..Default::default()
        });
        assert_matches_list(FightQuery {
            text: Some("first phase".to_string()),
            ..Default::default()
        });
        assert_matches_list(FightQuery {
            boss_object_type: Some(200),
            char_id: Some(1000),
            ..Default::default()
        });
        assert_matches_list(FightQuery {
            after: Some(20),
            ..Default::default()
        });
    }

    fn phase(
        boss_type: i32,
        name: &str,
        max_hp: i32,
        start_hp: i32,
        started_at: i64,
    ) -> FightRecord {
        FightRecord {
            id: started_at,
            started_at,
            ended_at: started_at + 1000,
            dungeon: "The Hive".to_string(),
            dungeon_entered_at: None,
            map_seed: 0,
            boss_object_type: boss_type,
            boss_name: name.to_string(),
            boss_max_hp: max_hp,
            boss_start_hp: start_hp,
            local_object_id: 0,
            local_char_id: 0,
            killed: true,
            local_close_calls: 0,
            aux_member_count: None,
            participants: vec![],
        }
    }

    #[test]
    fn collapse_duplicate_boss_phases_folds_respawned_boss() {
        // The Hive: Queen Bee, then the Prismimic Defender recorded twice (full
        // HP + a low-HP tail from a respawned object). The duplicate 19249 rows
        // fold into one; distinct types and the Attacker stay separate.
        let phases = vec![
            phase(297, "Queen Bee", 100_000, 100_000, 1),
            phase(19247, "Prismimic", 61_875, 61_875, 2),
            phase(19249, "Prismimic", 50_625, 50_625, 3),
            phase(19249, "Prismimic", 50_625, 5_062, 4),
        ];
        let out = collapse_duplicate_boss_phases(&phases);
        assert_eq!(out.len(), 3);
        let defender: Vec<_> = out.iter().filter(|p| p.boss_object_type == 19249).collect();
        assert_eq!(defender.len(), 1);
        // Full-HP engagement is kept (max, not a sum of the two rows).
        assert_eq!(defender[0].boss_max_hp, 50_625);
        assert_eq!(defender[0].boss_start_hp, 50_625);
        assert!(defender[0].killed);
    }

    #[test]
    fn collapse_folds_towering_perfection_respawned_segments() {
        // Towering Perfection repeatedly splits, re-spawning the core (47909) and
        // its two segments (Lower 47916 / Upper 47917) as fresh objects. Every
        // duplicate same-(type, name) wave folds to one row per segment, leaving
        // at most three rows regardless of how many times the tower split.
        let phases = vec![
            phase(47917, "Upper Imperfection", 531_500, 531_500, 1),
            phase(47916, "Lower Imperfection", 531_500, 531_500, 2),
            phase(47916, "Lower Imperfection", 406_500, 406_500, 3),
            phase(47917, "Upper Imperfection", 406_500, 406_500, 4),
            phase(47917, "Upper Imperfection", 394_000, 394_000, 5),
            phase(47916, "Lower Imperfection", 394_000, 394_000, 6),
            phase(47909, "Towering Perfection", 888_000, 187_311, 7),
            phase(47909, "Towering Perfection", 513_000, 513_000, 8),
        ];
        let out = collapse_duplicate_boss_phases(&phases);
        assert_eq!(out.len(), 3);
        for otype in [47909, 47916, 47917] {
            assert_eq!(
                out.iter().filter(|p| p.boss_object_type == otype).count(),
                1
            );
        }
        let upper = out.iter().find(|p| p.boss_object_type == 47917).unwrap();
        assert_eq!(upper.boss_max_hp, 531_500);
    }

    #[test]
    fn collapse_duplicate_boss_phases_keeps_named_splits_apart() {
        // Kitsune Umi (20493) is dedup-prone but heals back into a Second Coming;
        // the differing name suffix must keep the two comings as separate rows.
        let phases = vec![
            phase(20493, "Kitsune Umi (First Coming)", 10_000, 10_000, 1),
            phase(20493, "Kitsune Umi (Second Coming)", 10_000, 10_000, 2),
        ];
        let out = collapse_duplicate_boss_phases(&phases);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn collapse_duplicate_boss_phases_keeps_multi_instance_bosses() {
        // Five Pentaract towers of the same type/name are genuinely distinct
        // bosses (not dedup-prone) and must not fold.
        let phases: Vec<FightRecord> = (0..5)
            .map(|i| phase(22019, "Pentaract Tower", 30_000, 30_000, i))
            .collect();
        let out = collapse_duplicate_boss_phases(&phases);
        assert_eq!(out.len(), 5);
    }

    #[test]
    fn pick_anchor_demotes_prismimic_below_real_boss() {
        // A Prismimic spawns after the dungeon boss dies, so it ends later -- but
        // it must not headline the run. The earlier real boss anchors instead.
        let phases: Vec<PhaseStat> = vec![
            (99_999, 100_000, 100_000, true, 100), // real dungeon boss, ends first
            (19_249, 50_625, 50_625, true, 200),   // Prismimic Defender, ends later
        ];
        assert_eq!(pick_anchor(&phases).unwrap().0, 99_999);

        // With no other boss, the Prismimic still anchors its own run.
        let only: Vec<PhaseStat> = vec![(19_247, 61_875, 61_875, true, 50)];
        assert_eq!(pick_anchor(&only).unwrap().0, 19_247);
    }

    #[test]
    fn pick_anchor_demotes_wanderer_below_real_boss() {
        // an event Wanderer spawns on the dungeon boss's death (e.g. in
        // Untaris after Tarul), so it ends later -- but the real dungeon boss must
        // headline the run, not the Wanderer.
        let phases: Vec<PhaseStat> = vec![
            (45_811, 200_000, 200_000, true, 100), // Tarul (Untaris), ends first
            (14_474, 90_000, 90_000, true, 250),   // The Wanderer, ends later
        ];
        assert_eq!(pick_anchor(&phases).unwrap().0, 45_811);

        // In its own dungeon (Hidden Interregnum) the Wanderer is the only boss,
        // so it still anchors the run.
        for wanderer in [14_474, 14_477, 17_391, 18_346] {
            let only: Vec<PhaseStat> = vec![(wanderer, 90_000, 90_000, true, 50)];
            assert_eq!(pick_anchor(&only).unwrap().0, wanderer);
        }
    }

    #[test]
    fn pick_anchor_demotes_optional_secondary_boss_below_real_boss() {
        // A dungeon's optional side boss (e.g. Calamity Crab in Deadwater Docks)
        // is often killed after the real main boss, so it ends later -- but the
        // true main boss must headline the run, not the optional side boss.
        let phases: Vec<PhaseStat> = vec![
            (2352, 200_000, 200_000, true, 100), // Jon Bilgewater (main), ends first
            (47_387, 90_000, 90_000, true, 250), // Calamity Crab (optional), ends later
        ];
        assert_eq!(pick_anchor(&phases).unwrap().0, 2352);

        // Oryx's Castle: Stone Guardians (main) outrank Janus (optional).
        let castle: Vec<PhaseStat> = vec![
            (3448, 150_000, 150_000, true, 100), // Stone Guardian (main)
            (8200, 60_000, 60_000, true, 300),   // Janus the Doorwarden (optional), ends later
        ];
        assert_eq!(pick_anchor(&castle).unwrap().0, 3448);

        // With no real main tracked, the optional boss still anchors over crates.
        let only: Vec<PhaseStat> = vec![
            (24_092, 40_000, 40_000, true, 100), // Horrific Creation (optional)
            (620, 1, 1, true, 200),              // Bilgewater's Booty (crate), ends later
        ];
        assert_eq!(pick_anchor(&only).unwrap().0, 24_092);
    }

    #[test]
    fn tomb_of_the_ancients_completes_only_when_all_three_bosses_killed() {
        // ToA (Nut 3366, Geb 3367, Bes 3368; Active Sarcophagus 3369,
        // Treasure Sarcophagus 3372). The card is Completed only when all three
        // main bosses are killed -- opening a Sarcophagus or clearing one/two
        // bosses must not mark it Completed.

        // Only the Treasure Sarcophagus was opened -> Escaped.
        let sarc_only: Vec<PhaseStat> = vec![(3372, 50_000, 50_000, true, 100)];
        assert!(!pick_anchor(&sarc_only).unwrap().3);

        // Only the Active Sarcophagus killed -> Escaped.
        let active_only: Vec<PhaseStat> = vec![(3369, 80_000, 80_000, true, 100)];
        assert!(!pick_anchor(&active_only).unwrap().3);

        // Two of three bosses killed (Geb missing) -> Escaped.
        let two_of_three: Vec<PhaseStat> = vec![
            (3366, 120_000, 120_000, true, 100), // Nut killed
            (3368, 120_000, 120_000, true, 200), // Bes killed
            (3372, 50_000, 50_000, true, 300),   // crate opened
        ];
        assert!(!pick_anchor(&two_of_three).unwrap().3);

        // Nut engaged but not killed, Geb + Bes killed -> Escaped.
        let one_escaped: Vec<PhaseStat> = vec![
            (3366, 120_000, 40_000, false, 100), // Nut escaped
            (3367, 120_000, 120_000, true, 200), // Geb killed
            (3368, 120_000, 120_000, true, 300), // Bes killed
        ];
        assert!(!pick_anchor(&one_escaped).unwrap().3);

        // All three main bosses killed (plus loot crate) -> Completed.
        let all_three: Vec<PhaseStat> = vec![
            (3366, 120_000, 120_000, true, 100), // Nut
            (3367, 120_000, 120_000, true, 200), // Geb
            (3368, 120_000, 120_000, true, 300), // Bes
            (3372, 50_000, 50_000, true, 400),   // Treasure Sarcophagus opened
        ];
        assert!(pick_anchor(&all_three).unwrap().3);
    }

    #[test]
    fn display_boss_name_disambiguates_prismimic_halves() {
        assert_eq!(display_boss_name(19_247, "Prismimic"), "Prismimic Attacker");
        assert_eq!(display_boss_name(19_249, "Prismimic"), "Prismimic Defender");
        assert_eq!(display_boss_name(297, "Queen Bee"), "Queen Bee");
    }

    #[test]
    fn collapse_folds_moonlight_village_sage_genji_multi_detection() {
        // A summoner MV run re-detects Sage Genji many times. Every
        // "Sage Genji" phase (type 20450) folds to one; the other dancers stay.
        let mut phases = vec![
            phase(20451, "Dancer Miko", 360_000, 360_000, 1),
            phase(20452, "Drummer Kaguya", 360_000, 360_000, 2),
            phase(20450, "Sage Genji", 360_000, 360_000, 3),
        ];
        for i in 0..15 {
            phases.push(phase(20450, "Sage Genji", 180_000, 180_000, 10 + i));
        }
        let out = collapse_duplicate_boss_phases(&phases);
        assert_eq!(out.len(), 3);
        let genji: Vec<_> = out.iter().filter(|p| p.boss_object_type == 20450).collect();
        assert_eq!(genji.len(), 1);
        assert_eq!(genji[0].boss_max_hp, 360_000);
    }

    #[test]
    fn boss_line_lists_bosses_in_facing_order_not_kill_order() {
        // A Moonlight Village run's three dancers are faced Miko -> Kaguya ->
        // Genji, but die within moments of each other in a different order. The
        // card must list them by the order they were faced (started_at), so
        // ordering by kill time (ended_at) would scramble them.
        let db = CombatDatabase::open_in_memory().unwrap();
        db.conn
            .execute_batch(
                r#"
            INSERT INTO encounter_runs (run_id, encounter_id, dungeon, started_at, ended_at, killed)
              VALUES ('mv-order', 'dungeon_run', 'Moonlight Village', 100, 360, 1);
            INSERT INTO fights
              (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
               boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
               encounter_run_id, boss_group)
              VALUES
              (100, 300, 'Moonlight Village', 7, 20451, 'Dancer Miko',   360000, 360000, 9, 55, 1, 'mv-order', NULL),
              (150, 250, 'Moonlight Village', 7, 20452, 'Drummer Kaguya', 360000, 360000, 9, 55, 1, 'mv-order', NULL),
              (200, 360, 'Moonlight Village', 7, 20450, 'Sage Genji',     360000, 360000, 9, 55, 1, 'mv-order', NULL);
            "#,
            )
            .unwrap();

        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 1);
        let names: Vec<&str> = list[0]
            .killed_bosses
            .iter()
            .map(|(_, n)| n.as_str())
            .collect();
        assert_eq!(names, vec!["Dancer Miko", "Drummer Kaguya", "Sage Genji"]);
    }

    #[test]
    fn marble_colossus_survival_phases_unify_in_card_and_search() {
        // The two Marble Colossus survival phases share an object type. On the
        // summary card and in the boss search suggestions they must collapse to
        // a single "Marble Colossus" (no phase suffix); the suffix is kept only
        // in the per-phase DPS breakdown.
        let db = CombatDatabase::open_in_memory().unwrap();
        db.conn
            .execute_batch(
                r#"
            INSERT INTO encounter_runs (run_id, encounter_id, dungeon, started_at, ended_at, killed)
              VALUES ('mc-run', 'dungeon_run', 'Lost Halls', 100, 400, 1);
            INSERT INTO fights
              (started_at, ended_at, dungeon, map_seed, boss_object_type, boss_name,
               boss_max_hp, boss_start_hp, local_object_id, local_char_id, killed,
               encounter_run_id, boss_group)
              VALUES
              (100, 250, 'Lost Halls', 7, 45073, 'Marble Colossus (Pre-survival)',  100000, 100000, 9, 55, 1, 'mc-run', NULL),
              (260, 400, 'Lost Halls', 7, 45073, 'Marble Colossus (Post-survival)', 100000, 100000, 9, 55, 1, 'mc-run', NULL);
            "#,
            )
            .unwrap();

        let list = db.list_fights(&FightQuery::default(), 50).unwrap();
        assert_eq!(list.len(), 1);
        let names: Vec<&str> = list[0]
            .killed_bosses
            .iter()
            .map(|(_, n)| n.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["Marble Colossus"],
            "card shows one unsuffixed boss"
        );

        let bosses = db.distinct_bosses().unwrap();
        let marble: Vec<&(i32, String)> = bosses.iter().filter(|(t, _)| *t == 45073).collect();
        assert_eq!(marble.len(), 1, "search lists Marble Colossus once");
        assert_eq!(marble[0].1, "Marble Colossus");
    }
}

use super::*;
use crate::maintenance::MaintenanceLog;

const BATCH_SIZE: i64 = 64;
const JOBS: &[&str] = &[
    "encounter-grouping-v1",
    "curated-loot-completion-v1",
    "asset-loot-completion-v1",
    "ivory-completion-v1",
    "local-nexus-v1",
    "stat-awards-v1",
    "close-calls-v1",
];

pub(super) const PROGRESS_SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS historical_repairs (
        repair_id TEXT PRIMARY KEY,
        cutoff INTEGER NOT NULL,
        cursor INTEGER NOT NULL DEFAULT 0,
        seed_cursor INTEGER NOT NULL DEFAULT -2147483649,
        time_cursor INTEGER NOT NULL DEFAULT -9223372036854775808,
        complete INTEGER NOT NULL DEFAULT 0,
        text_cursor TEXT NOT NULL DEFAULT ''
    );
";

pub(super) const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS historical_repair_groups (
        encounter TEXT NOT NULL, char_id INTEGER NOT NULL,
        dungeon TEXT NOT NULL, map_seed INTEGER NOT NULL,
        run_id TEXT NOT NULL, last_ended INTEGER NOT NULL,
        PRIMARY KEY(encounter, char_id, dungeon, map_seed)
    );
    CREATE TABLE IF NOT EXISTS pending_stat_cards (
        card_key TEXT PRIMARY KEY
    );
    CREATE TABLE IF NOT EXISTS applied_loot_proofs (
        map_seed INTEGER NOT NULL, boss_type INTEGER NOT NULL, timestamp INTEGER NOT NULL,
        PRIMARY KEY(map_seed, boss_type, timestamp)
    );
    CREATE TABLE IF NOT EXISTS pending_loot_proofs (
        map_seed INTEGER NOT NULL, boss_type INTEGER NOT NULL, timestamp INTEGER NOT NULL,
        PRIMARY KEY(map_seed, boss_type, timestamp)
    );
    CREATE INDEX IF NOT EXISTS idx_pending_loot_proofs_time
        ON pending_loot_proofs(timestamp, map_seed, boss_type);
    CREATE INDEX IF NOT EXISTS idx_death_broadcasts_retry
        ON death_broadcasts(processed, next_attempt_at, occurred_at);
    CREATE TABLE IF NOT EXISTS fight_close_calls (
        fight_id INTEGER PRIMARY KEY,
        char_id INTEGER NOT NULL,
        total INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_fight_close_calls_char ON fight_close_calls(char_id);
    CREATE INDEX IF NOT EXISTS idx_stat_awards_char ON stat_awards(char_id);
    CREATE INDEX IF NOT EXISTS idx_fights_repair_order ON fights(map_seed, started_at, id);
    CREATE TRIGGER IF NOT EXISTS queue_inserted_stat_card AFTER INSERT ON fights BEGIN
        INSERT OR IGNORE INTO pending_stat_cards(card_key)
            VALUES (CASE WHEN NEW.encounter_run_id IS NULL THEN 'S' || NEW.id
                         ELSE 'E' || NEW.encounter_run_id END);
        INSERT INTO fight_close_calls(fight_id, char_id, total)
            VALUES (NEW.id, NEW.local_char_id, NEW.local_close_calls)
            ON CONFLICT(fight_id) DO UPDATE SET char_id=excluded.char_id, total=excluded.total;
    END;
    CREATE TABLE IF NOT EXISTS pending_stat_card_retirements(card_key TEXT PRIMARY KEY);
    DROP TRIGGER IF EXISTS queue_updated_stat_card;
    CREATE TRIGGER queue_updated_stat_card AFTER UPDATE ON fights BEGIN
        INSERT OR IGNORE INTO pending_stat_card_retirements(card_key)
            SELECT CASE WHEN OLD.encounter_run_id IS NULL THEN 'S' || OLD.id
                        ELSE 'E' || OLD.encounter_run_id END
            WHERE OLD.encounter_run_id IS NOT NEW.encounter_run_id
              AND EXISTS(SELECT 1 FROM stat_awards WHERE card_key=
                  CASE WHEN OLD.encounter_run_id IS NULL THEN 'S' || OLD.id
                       ELSE 'E' || OLD.encounter_run_id END);
        INSERT OR IGNORE INTO pending_stat_cards(card_key)
            SELECT 'E' || OLD.encounter_run_id
            WHERE OLD.encounter_run_id IS NOT NEW.encounter_run_id
              AND EXISTS(SELECT 1 FROM fights WHERE encounter_run_id=OLD.encounter_run_id);
        DELETE FROM pending_stat_cards
            WHERE card_key=CASE WHEN OLD.encounter_run_id IS NULL THEN 'S' || OLD.id
                                ELSE 'E' || OLD.encounter_run_id END
              AND OLD.encounter_run_id IS NOT NEW.encounter_run_id
              AND NOT EXISTS(SELECT 1 FROM fights WHERE encounter_run_id=OLD.encounter_run_id);
        INSERT OR IGNORE INTO pending_stat_cards(card_key)
            VALUES (CASE WHEN NEW.encounter_run_id IS NULL THEN 'S' || NEW.id
                         ELSE 'E' || NEW.encounter_run_id END);
        INSERT INTO fight_close_calls(fight_id, char_id, total)
            VALUES (NEW.id, NEW.local_char_id, NEW.local_close_calls)
            ON CONFLICT(fight_id) DO UPDATE SET char_id=excluded.char_id, total=excluded.total;
    END;
    CREATE TRIGGER IF NOT EXISTS queue_updated_stat_participant
    AFTER UPDATE ON fight_participants BEGIN
        INSERT OR IGNORE INTO pending_stat_cards(card_key)
            SELECT CASE WHEN encounter_run_id IS NULL THEN 'S' || id
                        ELSE 'E' || encounter_run_id END
            FROM fights WHERE id=NEW.fight_id;
    END;
    CREATE TABLE IF NOT EXISTS legacy_combat_groups (
        phase TEXT NOT NULL, first_id INTEGER NOT NULL, char_id INTEGER NOT NULL,
        dungeon TEXT NOT NULL, map_seed INTEGER NOT NULL,
        started_at INTEGER NOT NULL, ended_at INTEGER NOT NULL,
        run_id TEXT NOT NULL, live_run TEXT, conflict INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY(phase, first_id)
    );
    CREATE INDEX IF NOT EXISTS idx_legacy_combat_groups
        ON legacy_combat_groups(phase, char_id, dungeon, map_seed, started_at DESC);
    CREATE TABLE IF NOT EXISTS legacy_combat_run_spans (
        phase TEXT NOT NULL, run_id TEXT NOT NULL, encounter TEXT NOT NULL,
        char_id INTEGER NOT NULL, dungeon TEXT NOT NULL, map_seed INTEGER NOT NULL,
        started_at INTEGER NOT NULL, ended_at INTEGER NOT NULL, killed INTEGER NOT NULL,
        PRIMARY KEY(phase, run_id)
    );
    CREATE INDEX IF NOT EXISTS idx_legacy_combat_spans
        ON legacy_combat_run_spans(phase, char_id, map_seed, encounter, dungeon);
";

/// Changes committed by one bounded maintenance batch.
#[derive(Debug, Default)]
pub struct RepairBatch {
    pub history_changed: bool,
    pub pending: bool,
    pub stat_awards: HashMap<i32, (i64, i64, i64)>,
    pub close_calls: HashMap<i32, i64>,
}

impl CombatDatabase {
    pub fn loot_completion_repairs_complete(&self) -> SqlResult<bool> {
        self.conn.query_row(
            "SELECT COUNT(*)=3 FROM historical_repairs
             WHERE repair_id IN ('curated-loot-completion-v1',
                 'asset-loot-completion-v1','ivory-completion-v1') AND complete=1",
            [],
            |r| r.get(0),
        )
    }

    /// Repair one historical batch, with its durable checkpoint in the same
    /// transaction. No paired database is opened during writer initialization.
    pub fn historical_repair_batch(&mut self, loot_path: Option<&Path>) -> SqlResult<RepairBatch> {
        self.historical_repair_batch_logged(loot_path, &mut MaintenanceLog::default())
    }

    /// Reuse `log` across batches to coalesce lifecycle and retry messages.
    pub fn historical_repair_batch_logged(
        &mut self,
        loot_path: Option<&Path>,
        log: &mut MaintenanceLog,
    ) -> SqlResult<RepairBatch> {
        log.begin_batch("combat-maintenance");
        let result = (|| {
            self.conn.busy_timeout(std::time::Duration::ZERO)?;
            self.conn
                .execute_batch("SAVEPOINT historical_repair_batch")?;
            if let Some(batch) = self.legacy_combat_repair_batch(log)? {
                return Ok(batch);
            }
            log.task("combat-maintenance");
            let mut batch = self.historical_repair_batch_inner(loot_path, log)?;
            log.task("death-reconciliation");
            batch.history_changed |= self.repair_remote_deaths()? > 0;
            if crate::assets::get_asset_manager().is_loaded() {
                log.task("retired-stat-cards");
                let retired = self.recover_retired_stat_cards()?;
                batch.stat_awards.extend(retired.stat_awards);
                batch.pending |= retired.pending;
                log.task("live-loot-proofs-v1");
                batch.history_changed |= self.recover_loot_proofs(loot_path)?;
                log.task("pending-stat-cards");
                let recovered = self.recover_stat_cards()?;
                batch.stat_awards.extend(recovered.stat_awards);
                batch.close_calls.extend(recovered.close_calls);
                batch.pending |= recovered.pending;
            }
            Ok(batch)
        })();
        let result = result.and_then(|batch| {
            log.task("combat-maintenance-commit");
            self.conn.execute_batch("RELEASE historical_repair_batch")?;
            Ok(batch)
        });
        if result.is_err() && !self.conn.is_autocommit() {
            if let Err(rollback) = self.conn.execute_batch(
                "ROLLBACK TO historical_repair_batch; RELEASE historical_repair_batch",
            ) {
                tracing::error!("[COMBAT] Historical repair rollback failed: {rollback}");
            }
        }
        log.finish(&result);
        let timeout = self.conn.busy_timeout(std::time::Duration::from_secs(5));
        match (result, timeout) {
            (Ok(batch), Ok(())) => Ok(batch),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }

    fn historical_repair_batch_inner(
        &mut self,
        loot_path: Option<&Path>,
        log: &mut MaintenanceLog,
    ) -> SqlResult<RepairBatch> {
        let cutoff: i64 =
            self.conn
                .query_row("SELECT COALESCE(MAX(id), 0) FROM fights", [], |r| r.get(0))?;
        for job in JOBS {
            if !job.ends_with("loot-completion-v1") {
                self.conn.execute(
                    "INSERT OR IGNORE INTO historical_repairs(repair_id, cutoff)
                     VALUES (?1, ?2)",
                    params![job, cutoff],
                )?;
            }
        }
        // A missing paired database must not consume the repair's completion
        // marker or freeze an empty cutoff before that database is available.
        if !self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM historical_repairs WHERE repair_id='asset-loot-completion-v1')",
            [],
            |r| r.get::<_, bool>(0),
        )? {
            if let Some(path) = loot_path.filter(|p| p.exists()) {
                let loot = Self::repair_loot_reader(path)?;
                let loot_cutoff: i64 =
                    loot.query_row("SELECT COALESCE(MAX(id), 0) FROM loot_drops", [], |r| {
                        r.get(0)
                    })?;
                for job in ["curated-loot-completion-v1", "asset-loot-completion-v1"] {
                    self.conn.execute(
                        "INSERT OR IGNORE INTO historical_repairs(repair_id, cutoff) VALUES (?1, ?2)",
                        params![job, loot_cutoff],
                    )?;
                }
                self.conn.execute(
                    "INSERT OR IGNORE INTO historical_repairs(repair_id, cutoff, cursor)
                     VALUES ('live-loot-proofs-v1', 0, ?1)", params![loot_cutoff],
                )?;
            }
        }
        let mut deferred = false;
        for job in JOBS {
            let progress: Option<(i64, i64, i64, i64, bool)> = self
                .conn
                .query_row(
                    "SELECT cutoff, cursor, seed_cursor, time_cursor, complete
                 FROM historical_repairs WHERE repair_id=?1",
                    params![job],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .optional()?;
            let Some((cutoff, cursor, seed, time, false)) = progress else {
                if progress.is_none() {
                    log.deferred(job, "waiting for the paired Loot database");
                    deferred = true;
                }
                continue;
            };
            let mut batch = RepairBatch {
                pending: true,
                ..Default::default()
            };
            if matches!(*job, "asset-loot-completion-v1" | "stat-awards-v1")
                && !crate::assets::get_asset_manager().is_loaded()
            {
                log.deferred(job, "waiting for game assets");
                deferred = true;
                continue;
            }
            if job.ends_with("loot-completion-v1") && !loot_path.is_some_and(|path| path.exists()) {
                log.deferred(job, "waiting for the paired Loot database");
                deferred = true;
                continue;
            }
            log.start(
                job,
                cursor,
                cutoff,
                cursor != 0 || seed != -2147483649 || time != i64::MIN,
            );
            let (next, complete) = match *job {
                "encounter-grouping-v1" => {
                    let (next, complete, seed, time, changed) =
                        self.repair_encounter_groups(cutoff, cursor, seed, time)?;
                    batch.history_changed = changed;
                    self.conn.execute(
                        "UPDATE historical_repairs SET seed_cursor=?2, time_cursor=?3 WHERE repair_id=?1",
                        params![job, seed, time],
                    )?;
                    (next, complete)
                }
                "curated-loot-completion-v1" | "asset-loot-completion-v1" => {
                    let Some(path) = loot_path.filter(|p| p.exists()) else {
                        log.deferred(job, "waiting for the paired Loot database");
                        deferred = true;
                        continue;
                    };
                    let loot = Self::repair_loot_reader(path)?;
                    let marks = crate::assets::get_asset_manager().boss_mark_item_ids();
                    let mut stmt = loot.prepare(
                        "SELECT d.id, d.map_seed, d.mob_type, d.timestamp,
                                EXISTS(SELECT 1 FROM loot_items i WHERE i.drop_id=d.id
                                       AND i.item_id IN (SELECT value FROM json_each(?3)))
                         FROM loot_drops d WHERE d.id>?1 AND d.id<=?2 ORDER BY d.id LIMIT ?4",
                    )?;
                    let mark_json = serde_json::to_string(&marks)
                        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                    let rows = stmt
                        .query_map(params![cursor, cutoff, mark_json, BATCH_SIZE], |r| {
                            Ok((
                                r.get::<_, i64>(0)?,
                                r.get::<_, i32>(1)?,
                                r.get::<_, i32>(2)?,
                                r.get::<_, i64>(3)?,
                                r.get::<_, bool>(4)?,
                            ))
                        })?
                        .collect::<SqlResult<Vec<_>>>()?;
                    for &(_, seed, mob, timestamp, mark) in &rows {
                        let emitter = crate::assets::boss_for_loot_emitter(mob);
                        if seed == 0 || mob <= 0 {
                            continue;
                        }
                        let curated =
                            emitter.is_some() || crate::assets::bag_proves_boss_killed(mob);
                        if (*job == "curated-loot-completion-v1" && !curated)
                            || (*job == "asset-loot-completion-v1"
                                && (curated
                                    || (!mark
                                        && !crate::assets::get_asset_manager()
                                            .bag_attribution_proves_kill(mob))))
                        {
                            continue;
                        }
                        let fight_cutoff = self.conn.query_row(
                            "SELECT cutoff FROM historical_repairs
                             WHERE repair_id='encounter-grouping-v1'",
                            [],
                            |r| r.get(0),
                        )?;
                        batch.history_changed |= self.apply_stored_loot_proof(
                            seed,
                            emitter.unwrap_or(mob),
                            timestamp,
                            Some(fight_cutoff),
                        )?;
                    }
                    (
                        rows.last().map_or(cursor, |r| r.0),
                        rows.len() < BATCH_SIZE as usize,
                    )
                }
                _ => {
                    let limit = if *job == "stat-awards-v1" {
                        8
                    } else {
                        BATCH_SIZE
                    };
                    let rows: Vec<(i64, i32, i32, i64, Option<String>, i64)> = {
                        let mut stmt = self.conn.prepare(
                            "SELECT id, boss_object_type, local_char_id, started_at,
                                    encounter_run_id, local_close_calls
                             FROM fights WHERE id>?1 AND id<=?2 ORDER BY id LIMIT ?3",
                        )?;
                        let rows = stmt.query_map(params![cursor, cutoff, limit], |r| {
                            Ok((
                                r.get(0)?,
                                r.get(1)?,
                                r.get(2)?,
                                r.get(3)?,
                                r.get(4)?,
                                r.get(5)?,
                            ))
                        })?;
                        rows.collect::<SqlResult<_>>()?
                    };
                    match *job {
                        "ivory-completion-v1" => {
                            for &(_, boss, char_id, started, _, _) in &rows {
                                if boss == crate::assets::LEGACY_LOD_IVORY_BOSS {
                                    self.complete_lod_dragons_before_ivory(started, char_id)?;
                                    batch.history_changed = true;
                                }
                            }
                        }
                        "local-nexus-v1" => {
                            for &(id, _, _, _, _, _) in &rows {
                                batch.history_changed |= self.conn.execute(
                                    "UPDATE fight_participants SET end_status='present'
                                     WHERE fight_id=?1 AND is_local=1 AND end_status='nexused'
                                       AND EXISTS(SELECT 1 FROM fights WHERE id=?1 AND killed=1)",
                                    params![id],
                                )? > 0;
                            }
                        }
                        "stat-awards-v1" => {
                            let selections: Vec<_> = rows
                                .iter()
                                .map(|r| {
                                    r.4.as_ref().map_or(FightSelection::Single(r.0), |id| {
                                        FightSelection::Encounter(id.clone())
                                    })
                                })
                                .collect();
                            batch.stat_awards =
                                self.reconcile_stat_awards_for_selections(&selections)?;
                        }
                        "close-calls-v1" => {
                            let mut chars = HashSet::new();
                            for &(id, _, char_id, _, _, total) in &rows {
                                self.conn.execute(
                                    "INSERT INTO fight_close_calls(fight_id, char_id, total)
                                     VALUES (?1, ?2, ?3) ON CONFLICT(fight_id) DO UPDATE
                                     SET char_id=excluded.char_id, total=excluded.total",
                                    params![id, char_id, total],
                                )?;
                                chars.insert(char_id);
                            }
                            batch.close_calls = self.close_call_ledger_totals(&chars)?;
                        }
                        _ => unreachable!("known repair id"),
                    }
                    (
                        rows.last().map_or(cursor, |r| r.0),
                        rows.len() < limit as usize,
                    )
                }
            };
            self.conn.execute(
                "UPDATE historical_repairs SET cursor=?2, complete=?3 WHERE repair_id=?1",
                params![job, next, complete],
            )?;
            if complete {
                log.complete(job, next);
            }
            return Ok(batch);
        }
        log.task("encounter-grouping-cleanup");
        let deleted = self.conn.execute(
            "DELETE FROM historical_repair_groups WHERE rowid IN
             (SELECT rowid FROM historical_repair_groups LIMIT ?1)
             AND EXISTS(SELECT 1 FROM historical_repairs
                        WHERE repair_id='encounter-grouping-v1' AND complete=1)",
            params![BATCH_SIZE],
        )?;
        if deleted > 0 {
            log.start("encounter-grouping-cleanup", 0, 0, false);
        } else {
            log.complete("encounter-grouping-cleanup", 0);
        }
        Ok(RepairBatch {
            pending: deferred,
            ..Default::default()
        })
    }

    fn repair_loot_reader(path: &Path) -> SqlResult<Connection> {
        let loot = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        // Maintenance yields rather than blocking the packet worker on a lock.
        loot.busy_timeout(std::time::Duration::ZERO)?;
        Ok(loot)
    }

    fn apply_stored_loot_proof(
        &mut self,
        seed: i32,
        boss: i32,
        timestamp: i64,
        cutoff: Option<i64>,
    ) -> SqlResult<bool> {
        let changed = self
            .mark_boss_killed_by_loot_before(seed, boss, timestamp, cutoff)?
            .is_some();
        if !changed
            && chrono::Utc::now()
                .timestamp_millis()
                .saturating_sub(timestamp)
                < 15 * 60 * 1000
        {
            self.conn.execute(
                "INSERT OR IGNORE INTO pending_loot_proofs(map_seed, boss_type, timestamp)
                 SELECT ?1, ?2, ?3 WHERE NOT EXISTS(
                    SELECT 1 FROM applied_loot_proofs WHERE map_seed=?1 AND boss_type=?2 AND timestamp=?3)",
                params![seed, boss, timestamp],
            )?;
        }
        Ok(changed)
    }

    fn recover_loot_proofs(&mut self, path: Option<&Path>) -> SqlResult<bool> {
        let mut changed = false;
        let cursor: Option<i64> = self
            .conn
            .query_row(
                "SELECT cursor FROM historical_repairs WHERE repair_id='live-loot-proofs-v1'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if let (Some(cursor), Some(path)) = (cursor, path.filter(|p| p.exists())) {
            let loot = Self::repair_loot_reader(path)?;
            let marks = crate::assets::get_asset_manager().boss_mark_item_ids();
            let marks = serde_json::to_string(&marks)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            let mut stmt = loot.prepare(
                "SELECT d.id, d.map_seed, d.mob_type, d.timestamp,
                        EXISTS(SELECT 1 FROM loot_items i WHERE i.drop_id=d.id
                               AND i.item_id IN (SELECT value FROM json_each(?2)))
                 FROM loot_drops d WHERE d.id>?1 ORDER BY d.id LIMIT ?3",
            )?;
            let rows = stmt
                .query_map(params![cursor, marks, BATCH_SIZE], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i32>(1)?,
                        r.get::<_, i32>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, bool>(4)?,
                    ))
                })?
                .collect::<SqlResult<Vec<_>>>()?;
            for &(_, seed, mob, time, mark) in &rows {
                if seed == 0 || mob <= 0 {
                    continue;
                }
                let emitter = crate::assets::boss_for_loot_emitter(mob);
                if emitter.is_some()
                    || mark
                    || crate::assets::bag_proves_boss_killed(mob)
                    || crate::assets::get_asset_manager().bag_attribution_proves_kill(mob)
                {
                    changed |=
                        self.apply_stored_loot_proof(seed, emitter.unwrap_or(mob), time, None)?;
                }
            }
            if let Some(last) = rows.last() {
                self.conn.execute(
                    "UPDATE historical_repairs SET cursor=?1 WHERE repair_id='live-loot-proofs-v1'",
                    params![last.0],
                )?;
            }
        }
        let proofs: Vec<(i32, i32, i64)> = {
            let mut stmt = self.conn.prepare(
                "SELECT map_seed, boss_type, timestamp FROM pending_loot_proofs
                 ORDER BY timestamp, map_seed, boss_type LIMIT 8",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect::<SqlResult<_>>()?
        };
        for (seed, boss, time) in proofs {
            let applied = self
                .mark_boss_killed_by_loot_before(seed, boss, time, None)?
                .is_some();
            changed |= applied;
            if applied
                || chrono::Utc::now().timestamp_millis().saturating_sub(time) >= 15 * 60 * 1000
            {
                self.conn.execute(
                    "DELETE FROM pending_loot_proofs WHERE map_seed=?1 AND boss_type=?2 AND timestamp=?3",
                    params![seed, boss, time],
                )?;
            }
        }
        Ok(changed)
    }

    fn repair_encounter_groups(
        &mut self,
        cutoff: i64,
        cursor: i64,
        seed: i64,
        time: i64,
    ) -> SqlResult<(i64, bool, i64, i64, bool)> {
        let rows: Vec<(i64, i32, String, i32, i32, i64, i64, bool)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, boss_object_type, dungeon, map_seed, local_char_id, started_at, ended_at, killed
                 FROM fights INDEXED BY idx_fights_repair_order
                 WHERE (map_seed, started_at, id)>(?1, ?2, ?3) AND id<=?4
                 ORDER BY map_seed, started_at, id LIMIT ?5",
            )?;
            let rows = stmt.query_map(params![seed, time, cursor, cutoff, BATCH_SIZE], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ))
            })?;
            rows.collect::<SqlResult<_>>()?
        };
        let mut changed = false;
        for (id, boss, dungeon, seed, char_id, started, ended, killed) in &rows {
            if *seed == 0 || *char_id == 0 || !super::super::tracker::is_groupable_dungeon(dungeon)
            {
                continue;
            }
            if self.conn.query_row(
                "SELECT encounter_run_id IS NOT NULL FROM fights WHERE id=?1",
                params![id],
                |r| r.get::<_, bool>(0),
            )? {
                continue;
            }
            let encounter =
                crate::assets::encounter_for_boss_type(*boss).map_or("dungeon_run", |e| e.id);
            let previous: Option<(String, i64)> = self
                .conn
                .query_row(
                    "SELECT run_id, last_ended FROM historical_repair_groups
                 WHERE encounter=?1 AND char_id=?2 AND dungeon=?3 AND map_seed=?4",
                    params![encounter, char_id, dungeon, seed],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let (run_id, last_ended) = previous
                .filter(|(_, end)| started.saturating_sub(*end) <= 30 * 60 * 1000)
                .map_or_else(
                    || {
                        (
                            format!("bf-{encounter}-{seed}-{char_id}-{started:x}"),
                            *ended,
                        )
                    },
                    |(run_id, end)| (run_id, end.max(*ended)),
                );
            self.conn.execute(
                "INSERT INTO encounter_runs(run_id, encounter_id, dungeon, started_at, ended_at, killed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(run_id) DO UPDATE SET
                 started_at=MIN(started_at, excluded.started_at),
                 ended_at=MAX(ended_at, excluded.ended_at), killed=MAX(killed, excluded.killed)",
                params![run_id, encounter, dungeon, started, ended, killed],
            )?;
            self.conn.execute(
                "UPDATE fights SET encounter_id=?2, encounter_run_id=?3 WHERE id=?1",
                params![id, encounter, run_id],
            )?;
            self.conn.execute(
                "INSERT INTO historical_repair_groups(encounter, char_id, dungeon, map_seed, run_id, last_ended)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(encounter, char_id, dungeon, map_seed) DO UPDATE
                 SET run_id=excluded.run_id, last_ended=excluded.last_ended",
                params![encounter, char_id, dungeon, seed, run_id, last_ended],
            )?;
            changed = true;
        }
        let complete = rows.len() < BATCH_SIZE as usize;
        let next = rows
            .last()
            .map_or((cursor, seed, time), |r| (r.0, r.3 as i64, r.5));
        Ok((next.0, complete, next.1, next.2, changed))
    }

    fn close_call_ledger_totals(&self, chars: &HashSet<i32>) -> SqlResult<HashMap<i32, i64>> {
        let mut totals = HashMap::new();
        for &char_id in chars {
            if char_id != 0 {
                let total = self.conn.query_row(
                    "SELECT COALESCE(SUM(total), 0) FROM fight_close_calls WHERE char_id=?1",
                    params![char_id],
                    |r| r.get(0),
                )?;
                totals.insert(char_id, total);
            }
        }
        Ok(totals)
    }

    /// Recover a bounded set of interrupted live updates. The queue is written
    /// by the same transactions as the fight/participant changes.
    pub fn recover_stat_cards(&mut self) -> SqlResult<RepairBatch> {
        let keys: Vec<String> = {
            let mut stmt = self
                .conn
                .prepare("SELECT card_key FROM pending_stat_cards ORDER BY card_key LIMIT ?1")?;
            let rows = stmt.query_map(params![8], |r| r.get(0))?;
            rows.collect::<SqlResult<_>>()?
        };
        let mut selections = Vec::new();
        for key in &keys {
            if let Some(run) = key.strip_prefix('E') {
                selections.push(FightSelection::Encounter(run.to_string()));
            } else {
                let id = key
                    .strip_prefix('S')
                    .ok_or(rusqlite::Error::InvalidQuery)?
                    .parse()
                    .map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?;
                selections.push(FightSelection::Single(id));
            }
        }
        let stat_awards = self.reconcile_stat_awards_for_selections(&selections)?;
        let mut chars = HashSet::new();
        for selection in &selections {
            let (sql, bind): (&str, &dyn rusqlite::ToSql) = match selection {
                FightSelection::Single(id) => ("SELECT local_char_id FROM fights WHERE id=?1", id),
                FightSelection::Encounter(run) => (
                    "SELECT DISTINCT local_char_id FROM fights WHERE encounter_run_id=?1",
                    run,
                ),
            };
            let mut stmt = self.conn.prepare(sql)?;
            let rows = stmt.query_map([bind], |r| r.get::<_, i32>(0))?;
            chars.extend(rows.collect::<SqlResult<Vec<_>>>()?);
        }
        for key in &keys {
            self.conn.execute(
                "DELETE FROM pending_stat_cards WHERE card_key=?1",
                params![key],
            )?;
        }
        Ok(RepairBatch {
            stat_awards,
            close_calls: self.close_call_ledger_totals(&chars)?,
            pending: !keys.is_empty(),
            ..Default::default()
        })
    }

    /// Restore derived counters after a crash between a SQLite commit and the
    /// account-cache save. Reapplying absolute totals is idempotent.
    pub fn persisted_combat_totals(&self, chars: &[i32]) -> SqlResult<RepairBatch> {
        let mut batch = RepairBatch::default();
        for &char_id in chars {
            let totals = self.conn.query_row(
                "SELECT COALESCE(SUM(lone),0), COALESCE(SUM(last),0), COALESCE(SUM(most),0)
                 FROM stat_awards WHERE char_id=?1",
                params![char_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            batch.stat_awards.insert(char_id, totals);
        }
        batch.close_calls = self.close_call_ledger_totals(&chars.iter().copied().collect())?;
        Ok(batch)
    }
}

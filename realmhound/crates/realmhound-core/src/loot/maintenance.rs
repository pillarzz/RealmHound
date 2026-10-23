use super::*;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const BATCH: i64 = 64;
const FINAL_COMBAT_REPAIR: &str = "loot-023-combat-final";

pub(super) const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS legacy_loot_repairs (
        repair_id TEXT PRIMARY KEY, ordinal INTEGER NOT NULL,
        cutoff INTEGER NOT NULL, combat_cutoff INTEGER,
        cursor INTEGER NOT NULL DEFAULT 0,
        seed_cursor INTEGER NOT NULL DEFAULT -2147483649,
        boss_cursor INTEGER NOT NULL DEFAULT -2147483649,
        time_cursor INTEGER NOT NULL DEFAULT -9223372036854775808,
        complete INTEGER NOT NULL DEFAULT 0
    );
";

const STAGING_SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS legacy_loot_runs (
        family TEXT NOT NULL, map_seed INTEGER NOT NULL, state TEXT NOT NULL,
        PRIMARY KEY(family,map_seed)
    );
    CREATE TABLE IF NOT EXISTS legacy_loot_candidates (
        drop_id INTEGER PRIMARY KEY, map_seed INTEGER NOT NULL,
        boss INTEGER NOT NULL, timestamp INTEGER NOT NULL, name TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_legacy_loot_candidates
        ON legacy_loot_candidates(map_seed,boss,timestamp,drop_id);
    CREATE TABLE IF NOT EXISTS legacy_loot_votes (
        dungeon TEXT NOT NULL, mark INTEGER NOT NULL, boss INTEGER NOT NULL,
        name TEXT NOT NULL, votes INTEGER NOT NULL,
        PRIMARY KEY(dungeon,mark,boss,name)
    );
    CREATE INDEX IF NOT EXISTS idx_legacy_loot_votes
        ON legacy_loot_votes(dungeon,mark,votes DESC);
";

enum Kind {
    Simple(i32),
    SpecScan,
    SpecAssign,
    SpecMedian,
    SpecApply,
    TombScan,
    TombPreferred,
    TombOther,
    IceScan,
    IceApply,
    VoteScan,
    VoteApply,
    Combat,
    Cleanup(&'static str, Option<&'static str>),
}

const JOBS: &[(i32, &str, Kind)] = &[
    (4, "loot-004-janus", Kind::Simple(4)),
    (5, "loot-005-draconis", Kind::Simple(5)),
    (6, "loot-006-scan", Kind::SpecScan),
    (6, "loot-006-assign", Kind::SpecAssign),
    (6, "loot-006-median", Kind::SpecMedian),
    (6, "loot-006-apply", Kind::SpecApply),
    (
        6,
        "loot-006-candidates",
        Kind::Cleanup("legacy_loot_candidates", None),
    ),
    (
        6,
        "loot-006-runs",
        Kind::Cleanup("legacy_loot_runs", Some("spec")),
    ),
    (7, "loot-007-unique", Kind::Simple(7)),
    (13, "loot-013-combat", Kind::Combat),
    (14, "loot-014-scan", Kind::TombScan),
    (14, "loot-014-preferred", Kind::TombPreferred),
    (14, "loot-014-other", Kind::TombOther),
    (
        14,
        "loot-014-runs",
        Kind::Cleanup("legacy_loot_runs", Some("tomb")),
    ),
    (15, "loot-015-scan", Kind::IceScan),
    (15, "loot-015-apply", Kind::IceApply),
    (
        15,
        "loot-015-runs",
        Kind::Cleanup("legacy_loot_runs", Some("ice")),
    ),
    (16, "loot-016-souls", Kind::Simple(16)),
    (17, "loot-017-invalid", Kind::Simple(17)),
    (19, "loot-019-beisa", Kind::Simple(19)),
    (20, "loot-020-barkeep", Kind::Simple(20)),
    (21, "loot-021-bees", Kind::Simple(21)),
    (22, "loot-022-source", Kind::Simple(22)),
    (22, "loot-022-votes", Kind::VoteScan),
    (22, "loot-022-marks", Kind::VoteApply),
    (
        22,
        "loot-022-cleanup",
        Kind::Cleanup("legacy_loot_votes", None),
    ),
    (23, "loot-023-combat", Kind::Combat),
    (24, "loot-024-names", Kind::Simple(24)),
    // Revisit Unknown bags after Combat has consumed the corrected Loot sources.
    (23, FINAL_COMBAT_REPAIR, Kind::Combat),
];

const SPEC_BOSSES: [(i32, &str); 4] = [
    (super::super::SPEN_OCULON_OBJECT_TYPE, "Overseer Oculon"),
    (
        super::super::SPEN_GRETCH_OBJECT_TYPE,
        "Groundskeeper Gretch",
    ),
    (super::super::SPEN_ZOLE_OBJECT_TYPE, "Griefkeeper Zole"),
    (super::super::SPEN_LOBOTOMIK_OBJECT_TYPE, "Doctor Lobotomik"),
];

#[derive(Default, Deserialize, Serialize)]
struct RunState {
    claimed: u8,
    frequency: [i64; 4],
    options: u16,
    assigned: [i64; 4],
    seen: [i64; 4],
    median: [Option<i64>; 4],
}

struct Bag {
    id: i64,
    dungeon: String,
    seed: i32,
    timestamp: i64,
    bag_type: i32,
    mob: i32,
    name: String,
    items: Vec<i32>,
}

impl Bag {
    fn brown(&self) -> bool {
        matches!(self.bag_type, 1280 | 1709)
    }
}

impl LootDatabase {
    pub(super) fn ensure_final_combat_correlation(&self) -> SqlResult<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO legacy_loot_repairs
                 (repair_id,ordinal,cutoff,combat_cutoff)
             SELECT ?1,?2,cutoff,combat_cutoff FROM legacy_loot_repairs
             WHERE repair_id='loot-023-combat'",
            params![FINAL_COMBAT_REPAIR, (JOBS.len() - 1) as i64],
        )?;
        Ok(())
    }

    pub fn awaits_combat_completion(&self) -> SqlResult<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT repair_id=?1 FROM legacy_loot_repairs
                 WHERE complete=0 ORDER BY ordinal LIMIT 1",
                [FINAL_COMBAT_REPAIR],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    pub(super) fn create_repair_tables(&self) -> SqlResult<()> {
        self.conn.execute_batch(SCHEMA)?;
        self.conn.execute_batch(STAGING_SCHEMA)
    }

    pub(super) fn register_legacy_repairs(
        &self,
        version: i32,
        supplied_combat_cutoff: Option<i64>,
    ) -> SqlResult<()> {
        self.conn.execute_batch(SCHEMA)?;
        let cutoff: i64 =
            self.conn
                .query_row("SELECT COALESCE(MAX(id),0) FROM loot_drops", [], |r| {
                    r.get(0)
                })?;
        let combat_cutoff = if self.combat_path.is_none() {
            Some(0)
        } else if supplied_combat_cutoff.is_some() {
            supplied_combat_cutoff
        } else if version < 23 {
            match self.repair_combat_reader().and_then(|conn| {
                conn.query_row("SELECT COALESCE(MAX(id),0) FROM fights", [], |r| r.get(0))
            }) {
                Ok(cutoff) => Some(cutoff),
                Err(error) => {
                    tracing::warn!("[LOOT] Combat snapshot unavailable at open; deferring correlation: {error}");
                    None
                }
            }
        } else {
            None
        };
        let tx = self.conn.unchecked_transaction()?;
        for (ordinal, &(target, job, _)) in JOBS.iter().enumerate() {
            if version < target && job != FINAL_COMBAT_REPAIR {
                tx.execute(
                    "INSERT OR IGNORE INTO legacy_loot_repairs
                     (repair_id,ordinal,cutoff,combat_cutoff) VALUES (?1,?2,?3,?4)",
                    params![job, ordinal as i64, cutoff, combat_cutoff],
                )?;
            }
        }
        if let Some(cutoff) = supplied_combat_cutoff {
            tx.execute(
                "UPDATE legacy_loot_repairs SET combat_cutoff=?1 WHERE combat_cutoff IS NULL",
                [cutoff],
            )?;
        }
        tx.commit()?;
        self.ensure_final_combat_correlation()?;
        self.conn.execute_batch(STAGING_SCHEMA)
    }

    /// Run one resumable historical correction between capture batches.
    pub fn historical_repair_batch(&mut self) -> SqlResult<(bool, bool)> {
        self.conn.busy_timeout(std::time::Duration::ZERO)?;
        let result = (|| {
            let tx = self.conn.unchecked_transaction()?;
            let batch = self.loot_repair_batch_inner()?;
            tx.commit()?;
            Ok(batch)
        })();
        self.conn.busy_timeout(std::time::Duration::from_secs(5))?;
        result
    }

    fn loot_repair_batch_inner(&self) -> SqlResult<(bool, bool)> {
        let progress: Option<(String, i64, Option<i64>, i64, i64, i64, i64)> = self
            .conn
            .query_row(
                "SELECT repair_id,cutoff,combat_cutoff,cursor,seed_cursor,boss_cursor,time_cursor
             FROM legacy_loot_repairs WHERE complete=0 ORDER BY ordinal LIMIT 1",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .optional()?;
        let Some((job, cutoff, combat_cutoff, mut cursor, mut seed, mut boss, mut time)) = progress
        else {
            return Ok((false, false));
        };
        if !get_asset_manager().is_loaded() {
            return Ok((false, true));
        }
        let Some((_, _, kind)) = JOBS.iter().find(|(_, id, _)| *id == job) else {
            tracing::error!("[LOOT] Unrecognized historical repair: {job}");
            return Err(rusqlite::Error::InvalidQuery);
        };
        let mut changed = false;
        let count = match kind {
            Kind::Cleanup(table, family) => {
                if let Some(family) = family {
                    self.conn.execute(
                        &format!(
                            "DELETE FROM {table} WHERE rowid IN
                            (SELECT rowid FROM {table} WHERE family=?1 LIMIT ?2)"
                        ),
                        params![family, BATCH],
                    )?
                } else {
                    self.conn.execute(
                    &format!("DELETE FROM {table} WHERE rowid IN (SELECT rowid FROM {table} LIMIT ?1)"),
                    [BATCH],
                    )?
                }
            }
            Kind::SpecMedian | Kind::SpecApply => {
                let median = matches!(kind, Kind::SpecMedian);
                let sql = if median {
                    "SELECT drop_id,map_seed,boss,timestamp,name FROM legacy_loot_candidates
                     WHERE (map_seed,boss,timestamp,drop_id)>(?2,?3,?4,?1)
                     ORDER BY map_seed,boss,timestamp,drop_id LIMIT ?5"
                } else {
                    "SELECT drop_id,map_seed,boss,timestamp,name FROM legacy_loot_candidates
                     WHERE drop_id>?1 AND ?2 IS NOT NULL AND ?3 IS NOT NULL AND ?4 IS NOT NULL
                     ORDER BY drop_id LIMIT ?5"
                };
                let rows: Vec<(i64, i32, i32, i64, String)> = self
                    .conn
                    .prepare(sql)?
                    .query_map(params![cursor, seed, boss, time, BATCH], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                    })?
                    .collect::<SqlResult<_>>()?;
                for &(id, map_seed, boss_type, timestamp, ref name) in &rows {
                    let index = SPEC_BOSSES
                        .iter()
                        .position(|(t, _)| *t == boss_type)
                        .ok_or(rusqlite::Error::InvalidQuery)?;
                    let mut state = self.run_state("spec", map_seed)?;
                    if median {
                        if state.seen[index] == state.assigned[index] / 2 {
                            state.median[index] = Some(timestamp);
                        }
                        state.seen[index] += 1;
                        self.save_run_state("spec", map_seed, &state)?;
                    } else {
                        let median = state.median[index].ok_or(rusqlite::Error::InvalidQuery)?;
                        if timestamp.abs_diff(median) <= 300_000 {
                            changed |= self.conn.execute(
                                "UPDATE loot_drops SET mob_type=?2,mob_name=?3 WHERE id=?1",
                                params![id, boss_type, name],
                            )? > 0;
                        }
                    }
                    cursor = id;
                    seed = i64::from(map_seed);
                    boss = i64::from(boss_type);
                    time = timestamp;
                }
                rows.len()
            }
            _ => {
                let dungeon = match kind {
                    Kind::SpecScan | Kind::SpecAssign => {
                        Some(super::super::SPECTRAL_PENITENTIARY_NAME)
                    }
                    Kind::TombScan | Kind::TombPreferred | Kind::TombOther => {
                        Some(super::super::TOMB_OF_THE_ANCIENTS_NAME)
                    }
                    Kind::IceScan | Kind::IceApply => Some(super::super::ICE_TOMB_NAME),
                    _ => None,
                };
                let combat = if matches!(kind, Kind::Combat) {
                    let Some(limit) = combat_cutoff else {
                        tracing::warn!(
                            "[LOOT] Historical correlation awaits a pre-capture Combat snapshot"
                        );
                        return Ok((false, true));
                    };
                    if self.combat_path.is_none() {
                        self.finish_loot_job(&job)?;
                        return Ok((false, true));
                    }
                    let conn = self.repair_combat_reader()?;
                    let has_kills: bool = conn.query_row(
                        "SELECT EXISTS(SELECT 1 FROM fights WHERE id<=?1 AND killed=1 AND boss_object_type>0)",
                        [limit], |r| r.get(0),
                    )?;
                    if !has_kills {
                        self.finish_loot_job(&job)?;
                        return Ok((false, true));
                    }
                    Some((conn, limit))
                } else {
                    None
                };
                let bags = self.repair_bags(cutoff, cursor, dungeon)?;
                let marks = if matches!(kind, Kind::VoteScan | Kind::VoteApply) {
                    get_asset_manager().boss_mark_item_ids()
                } else {
                    HashSet::new()
                };
                for bag in &bags {
                    changed |= match kind {
                        Kind::Simple(version) => self.simple_loot_repair(bag, *version)?,
                        Kind::TombScan | Kind::TombPreferred | Kind::TombOther => {
                            self.repair_tomb(bag, kind)?
                        }
                        Kind::IceScan | Kind::IceApply => self.repair_ice(bag, kind)?,
                        Kind::SpecScan | Kind::SpecAssign => self.repair_spec(bag, kind)?,
                        Kind::VoteScan | Kind::VoteApply => {
                            self.repair_mark_consensus(bag, kind, &marks)?
                        }
                        Kind::Combat => {
                            let (conn, limit) =
                                combat.as_ref().ok_or(rusqlite::Error::InvalidQuery)?;
                            self.repair_bag_from_combat(bag, conn, *limit)?
                        }
                        _ => unreachable!("header-based loot repair"),
                    };
                }
                cursor = bags.last().map_or(cursor, |b| b.id);
                bags.len()
            }
        };
        self.conn.execute(
            "UPDATE legacy_loot_repairs SET cursor=?2,seed_cursor=?3,boss_cursor=?4,
             time_cursor=?5,complete=?6 WHERE repair_id=?1",
            params![job, cursor, seed, boss, time, count < BATCH as usize],
        )?;
        Ok((changed, true))
    }

    fn finish_loot_job(&self, job: &str) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE legacy_loot_repairs SET complete=1 WHERE repair_id=?1",
            [job],
        )?;
        Ok(())
    }

    fn repair_combat_reader(&self) -> SqlResult<Connection> {
        let path = self
            .combat_path
            .as_deref()
            .ok_or(rusqlite::Error::InvalidQuery)?;
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(std::time::Duration::ZERO)?;
        Ok(conn)
    }

    fn repair_bags(&self, cutoff: i64, cursor: i64, dungeon: Option<&str>) -> SqlResult<Vec<Bag>> {
        let sql = if dungeon.is_some() {
            "SELECT id,dungeon,map_seed,timestamp,bag_type,mob_type,mob_name
             FROM loot_drops WHERE id>?1 AND id<=?2 AND dungeon=?3 ORDER BY id LIMIT 64"
        } else {
            "SELECT id,dungeon,map_seed,timestamp,bag_type,mob_type,mob_name
             FROM loot_drops WHERE id>?1 AND id<=?2 ORDER BY id LIMIT 64"
        };
        let mut stmt = self.conn.prepare(sql)?;
        let read = |r: &rusqlite::Row<'_>| {
            Ok(Bag {
                id: r.get(0)?,
                dungeon: r.get(1)?,
                seed: r.get(2)?,
                timestamp: r.get(3)?,
                bag_type: r.get(4)?,
                mob: r.get(5)?,
                name: r.get(6)?,
                items: Vec::new(),
            })
        };
        let rows = if let Some(dungeon) = dungeon {
            stmt.query_map(params![cursor, cutoff, dungeon], read)?
        } else {
            stmt.query_map(params![cursor, cutoff], read)?
        };
        let mut bags = rows.collect::<SqlResult<Vec<_>>>()?;
        let mut items = self
            .conn
            .prepare("SELECT item_id FROM loot_items WHERE drop_id=?1 ORDER BY id")?;
        for bag in &mut bags {
            bag.items = items
                .query_map([bag.id], |r| r.get(0))?
                .collect::<SqlResult<_>>()?;
        }
        Ok(bags)
    }

    fn run_state(&self, family: &str, seed: i32) -> SqlResult<RunState> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT state FROM legacy_loot_runs WHERE family=?1 AND map_seed=?2",
                params![family, seed],
                |r| r.get(0),
            )
            .optional()?;
        match json {
            None => Ok(RunState::default()),
            Some(json) => serde_json::from_str(&json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            }),
        }
    }

    fn save_run_state(&self, family: &str, seed: i32, state: &RunState) -> SqlResult<()> {
        let json = serde_json::to_string(state)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        self.conn.execute(
            "INSERT INTO legacy_loot_runs(family,map_seed,state) VALUES (?1,?2,?3)
             ON CONFLICT(family,map_seed) DO UPDATE SET state=excluded.state",
            params![family, seed, json],
        )?;
        Ok(())
    }

    fn set_repaired_source(&self, bag: &Bag, source: Option<(i32, String)>) -> SqlResult<bool> {
        let Some((mob, name)) = source else {
            return Ok(false);
        };
        if mob == bag.mob && name == bag.name {
            return Ok(false);
        }
        Ok(self.conn.execute(
            "UPDATE loot_drops SET mob_type=?2,mob_name=?3 WHERE id=?1",
            params![bag.id, mob, name],
        )? > 0)
    }

    fn simple_loot_repair(&self, bag: &Bag, version: i32) -> SqlResult<bool> {
        use super::super::*;
        let source = match version {
            4 if bag.mob != JANUS_OBJECT_TYPE && bag.items.contains(&MARK_OF_JANUS_ITEM_ID) => {
                Some((JANUS_OBJECT_TYPE, JANUS_NAME.to_string()))
            }
            5 if bag.mob == 0 && bag.dungeon == LAIR_OF_DRACONIS_NAME => {
                lair_of_draconis_dragon(&bag.items).map(|(t, n)| (t, n.to_string()))
            }
            7 if bag.mob == 0 && !bag.brown() && !has_curated_boss_attribution(&bag.dungeon) => {
                unique_boss_drop(&bag.dungeon, &bag.items)
            }
            16 if bag.dungeon == ICE_TOMB_NAME => ICE_TOMB_BOSSES
                .iter()
                .copied()
                .find(|boss| {
                    ice_tomb_soul_for_boss(boss) != 0 && ice_tomb_soul_for_boss(boss) == bag.mob
                })
                .map(|boss| (ice_tomb_object_type_for_boss(boss), boss.to_string())),
            17 if bag.mob != 0
                && get_asset_manager().object_class(bag.mob).is_some()
                && !get_asset_manager().is_valid_drop_source(bag.mob) =>
            {
                Some((0, "?".to_string()))
            }
            19 if bag.mob == 0 && bag.dungeon == ORYX_SANCTUARY_NAME => {
                LootBagType::from_id(bag.bag_type)
                    .filter(|_| !bag.brown())
                    .filter(|kind| bag_matches_beisa_profile(*kind, &bag.items))
                    .map(|_| (CHIEF_BEISA_OBJECT_TYPE, CHIEF_BEISA_NAME.to_string()))
            }
            20 if bag.mob != TAVERN_BARKEEP_OBJECT_TYPE
                && bag.items.contains(&MARK_OF_THE_BARKEEP_ITEM_ID) =>
            {
                Some((TAVERN_BARKEEP_OBJECT_TYPE, TAVERN_BARKEEP_NAME.to_string()))
            }
            21 if (bag.dungeon.trim_matches(' ').is_empty()
                || bag.dungeon.starts_with('{')
                || bag.dungeon == REALM_NAME)
                && (bag.mob == 0 || bag.mob == EH_EVENT_TAUNT_CONTROLLER_OBJECT_TYPE) =>
            {
                LootBagType::from_id(bag.bag_type)
                    .filter(|_| !bag.brown())
                    .and_then(|_| {
                        LootTracker::resolve_killer_bee_nest(
                            &[],
                            bag.mob,
                            &bag.items,
                            bag.seed,
                            bag.timestamp,
                        )
                    })
            }
            22 if bag.mob == 0
                && !bag.brown()
                && !has_curated_boss_attribution(&bag.dungeon)
                && LootBagType::from_id(bag.bag_type).is_some() =>
            {
                if is_realm_dungeon(&bag.dungeon) {
                    resolve_realm_event_source(&bag.items)
                } else {
                    let candidates = boss_drop_candidates(&bag.dungeon, &bag.items);
                    match candidates.as_slice() {
                        [only] => Some(only.clone()),
                        _ => None,
                    }
                }
            }
            24 if bag.dungeon == THE_SHATTERS_NAME => [
                crate::assets::SHATTERS_BRIDGE_SENTINEL_TYPE,
                crate::assets::SHATTERS_TWILIGHT_ARCHMAGE_TYPE,
                crate::assets::SHATTERS_KING_TYPE,
            ]
            .into_iter()
            .find_map(|t| {
                let revealed = crate::assets::shatters_hm_boss_name(t)?;
                let original = crate::assets::shatters_boss_name(t)?;
                (bag.name == revealed).then(|| (bag.mob, format!("{original} (HM)")))
            }),
            _ => None,
        };
        self.set_repaired_source(bag, source)
    }

    fn tomb_signal(bag: &Bag) -> Option<(i32, String)> {
        use super::super::*;
        if bag.items.contains(&MARK_OF_GEB_ITEM_ID) || is_tomb_spawner_only_bag(&bag.items) {
            return Some((
                TOMB_GEB_OBJECT_TYPE,
                get_asset_manager()
                    .object_name(TOMB_GEB_OBJECT_TYPE)
                    .unwrap_or_else(|| "Geb".to_string()),
            ));
        }
        if matches!(
            bag.mob,
            TOMB_ACTIVE_SARCOPHAGUS_OBJECT_TYPE | TOMB_TREASURE_SARCOPHAGUS_OBJECT_TYPE
        ) {
            return None;
        }
        tomb_unique_boss(&bag.items)
    }

    fn repair_tomb(&self, bag: &Bag, kind: &Kind) -> SqlResult<bool> {
        use super::super::*;
        if bag.brown() {
            return Ok(false);
        }
        let signal = Self::tomb_signal(bag);
        let spawner =
            bag.items.contains(&MARK_OF_GEB_ITEM_ID) || is_tomb_spawner_only_bag(&bag.items);
        let sarcophagus = matches!(
            bag.mob,
            TOMB_ACTIVE_SARCOPHAGUS_OBJECT_TYPE | TOMB_TREASURE_SARCOPHAGUS_OBJECT_TYPE
        );
        let mut state = self.run_state("tomb", bag.seed)?;
        let mut source = None;
        if matches!(kind, Kind::TombScan) {
            if let Some((t, n)) = signal {
                if !spawner {
                    if let Some(index) = TOMB_MAIN_BOSSES.iter().position(|(boss, _)| *boss == t) {
                        state.claimed |= 1 << index;
                    }
                }
                if bag.mob != t {
                    source = Some((t, n));
                }
            }
        } else if signal.is_none() && !sarcophagus {
            let preferred = TOMB_MAIN_BOSSES.iter().position(|(t, _)| *t == bag.mob);
            let life_misattributed =
                preferred.is_none() && bag.mob != 0 && bag.items.contains(&POTION_OF_LIFE_ITEM_ID);
            if matches!(kind, Kind::TombPreferred) == preferred.is_some()
                && (preferred.is_some() || bag.mob == 0 || life_misattributed)
            {
                let placed = preferred
                    .filter(|index| state.claimed & (1 << index) == 0)
                    .or_else(|| {
                        (bag.seed != 0)
                            .then(|| {
                                (0..TOMB_MAIN_BOSSES.len())
                                    .find(|index| state.claimed & (1 << index) == 0)
                            })
                            .flatten()
                    });
                if let Some(index) = placed {
                    state.claimed |= 1 << index;
                    let (t, n) = TOMB_MAIN_BOSSES[index];
                    if bag.mob != t {
                        source = Some((
                            t,
                            get_asset_manager()
                                .object_name(t)
                                .unwrap_or_else(|| n.to_string()),
                        ));
                    }
                } else if life_misattributed {
                    source = Some((0, "Unknown".to_string()));
                }
            }
        }
        self.save_run_state("tomb", bag.seed, &state)?;
        self.set_repaired_source(bag, source)
    }

    fn repair_ice(&self, bag: &Bag, kind: &Kind) -> SqlResult<bool> {
        use super::super::*;
        if bag.brown() {
            return Ok(false);
        }
        let mut state = self.run_state("ice", bag.seed)?;
        let unique = ice_tomb_unique_boss(&bag.items);
        let stored =
            ice_tomb_boss_name_for_object(bag.mob).or_else(|| ice_tomb_boss_name_for_soul(bag.mob));
        let selected = if matches!(kind, Kind::IceScan) {
            unique.or(stored)
        } else if bag.seed != 0
            && bag.mob == 0
            && unique.is_none()
            && stored.is_none()
            && ice_tomb_has_boss_loot(&bag.items)
        {
            ICE_TOMB_BOSSES
                .iter()
                .enumerate()
                .find(|(index, _)| state.claimed & (1 << index) == 0)
                .map(|(_, boss)| *boss)
        } else {
            None
        };
        let mut source = None;
        if let Some(boss) = selected {
            let index = ICE_TOMB_BOSSES
                .iter()
                .position(|b| *b == boss)
                .ok_or(rusqlite::Error::InvalidQuery)?;
            state.claimed |= 1 << index;
            let object = ice_tomb_object_type_for_boss(boss);
            if (!matches!(kind, Kind::IceScan) || unique.is_some()) && bag.mob != object {
                source = Some((object, boss.to_string()));
            }
        }
        self.save_run_state("ice", bag.seed, &state)?;
        self.set_repaired_source(bag, source)
    }

    fn spec_options(bag: &Bag) -> Vec<(i32, &'static str)> {
        use super::super::*;
        let boss_loot = spectral_penitentiary_boss_confirmed(&bag.items);
        if let Some(boss) = spectral_penitentiary_potion_miniboss(&bag.items, boss_loot) {
            vec![boss]
        } else if boss_loot {
            spectral_penitentiary_shard_pair(&bag.items)
                .map(|p| p.to_vec())
                .unwrap_or_else(|| SPEC_BOSSES.to_vec())
        } else {
            Vec::new()
        }
    }

    fn repair_spec(&self, bag: &Bag, kind: &Kind) -> SqlResult<bool> {
        use super::super::*;
        let mut state = self.run_state("spec", bag.seed)?;
        let mut source = None;
        if matches!(kind, Kind::SpecScan) {
            if let Some(index) = SPEC_BOSSES.iter().position(|(t, _)| *t == bag.mob) {
                state.claimed |= 1 << index;
            }
        }
        if bag.mob == 0 && !bag.brown() {
            let reliable = spectral_penitentiary_murcian(&bag.items)
                .or_else(|| spectral_penitentiary_reliable_boss(&bag.items));
            if matches!(kind, Kind::SpecScan) {
                if let Some((t, n)) = reliable {
                    if let Some(index) = SPEC_BOSSES.iter().position(|(boss, _)| *boss == t) {
                        state.claimed |= 1 << index;
                    }
                    source = Some((t, n.to_string()));
                } else {
                    let options = Self::spec_options(bag);
                    let mask = options.iter().fold(0u8, |mask, (t, _)| {
                        mask | SPEC_BOSSES
                            .iter()
                            .position(|(boss, _)| boss == t)
                            .map_or(0, |i| 1 << i)
                    });
                    state.options |= 1u16 << mask;
                    if options.len() == 1 {
                        if let Some(index) =
                            SPEC_BOSSES.iter().position(|(t, _)| *t == options[0].0)
                        {
                            state.frequency[index] += 1;
                        }
                    }
                }
            } else if reliable.is_none() {
                let mut allowed = state.claimed;
                let mut ranked: Vec<_> = (0..4).filter(|&i| state.frequency[i] > 0).collect();
                ranked.sort_by_key(|&i| (-state.frequency[i], SPEC_BOSSES[i].0));
                for index in ranked {
                    if allowed.count_ones() >= 2 {
                        break;
                    }
                    allowed |= 1 << index;
                }
                if allowed.count_ones() < 2
                    && (1u8..16).any(|mask| {
                        state.options & (1u16 << mask) != 0
                            && mask & 1 != 0
                            && mask & state.claimed == 0
                    })
                {
                    allowed |= 1;
                }
                let options: Vec<_> = Self::spec_options(bag)
                    .into_iter()
                    .filter(|(t, _)| {
                        SPEC_BOSSES
                            .iter()
                            .position(|(boss, _)| boss == t)
                            .is_some_and(|i| allowed & (1 << i) != 0)
                    })
                    .collect();
                let chosen = options
                    .iter()
                    .find(|(t, _)| {
                        SPEC_BOSSES
                            .iter()
                            .position(|(boss, _)| boss == t)
                            .is_some_and(|i| state.claimed & (1 << i) != 0)
                    })
                    .or_else(|| options.iter().find(|(t, _)| *t == SPEN_OCULON_OBJECT_TYPE))
                    .or_else(|| {
                        if options.len() == 1 {
                            options.first()
                        } else {
                            None
                        }
                    });
                if let Some(&(t, n)) = chosen {
                    let index = SPEC_BOSSES
                        .iter()
                        .position(|(boss, _)| *boss == t)
                        .ok_or(rusqlite::Error::InvalidQuery)?;
                    self.conn.execute(
                        "INSERT INTO legacy_loot_candidates(drop_id,map_seed,boss,timestamp,name) VALUES (?1,?2,?3,?4,?5)",
                        params![bag.id,bag.seed,t,bag.timestamp,n],
                    )?;
                    state.assigned[index] += 1;
                }
            }
        }
        self.save_run_state("spec", bag.seed, &state)?;
        self.set_repaired_source(bag, source)
    }

    fn repair_mark_consensus(
        &self,
        bag: &Bag,
        kind: &Kind,
        marks: &HashSet<i32>,
    ) -> SqlResult<bool> {
        use super::super::*;
        if has_curated_boss_attribution(&bag.dungeon) {
            return Ok(false);
        }
        let assets = get_asset_manager();
        if matches!(kind, Kind::VoteScan) {
            if bag.mob != 0
                && (assets.is_boss_like(bag.mob) || assets.is_curated_boss(bag.mob))
                && assets.is_valid_drop_source(bag.mob)
            {
                for item in bag.items.iter().filter(|item| marks.contains(item)) {
                    self.conn.execute(
                        "INSERT INTO legacy_loot_votes(dungeon,mark,boss,name,votes) VALUES (?1,?2,?3,?4,1)
                         ON CONFLICT(dungeon,mark,boss,name) DO UPDATE SET votes=votes+1",
                        params![bag.dungeon,item,bag.mob,bag.name],
                    )?;
                }
            }
            return Ok(false);
        }
        if bag.mob != 0 {
            return Ok(false);
        }
        let Some(mark) = bag.items.iter().find(|item| marks.contains(item)) else {
            return Ok(false);
        };
        let winners:Vec<(i32,String,i64)> = self.conn.prepare(
            "SELECT boss,name,votes FROM legacy_loot_votes WHERE dungeon=?1 AND mark=?2 ORDER BY votes DESC LIMIT 2",
        )?.query_map(params![bag.dungeon,mark],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?.collect::<SqlResult<_>>()?;
        let source = match winners.as_slice() {
            [(t, n, _)] => Some((*t, n.clone())),
            [(t, n, votes), (_, _, next)] if votes > next => Some((*t, n.clone())),
            _ => None,
        };
        self.set_repaired_source(bag, source)
    }

    fn repair_bag_from_combat(&self, bag: &Bag, conn: &Connection, cutoff: i64) -> SqlResult<bool> {
        use super::super::tracker::{
            KILL_CORRELATION_EARLY_TOLERANCE_MS, KILL_CORRELATION_MAX_LAG_MS,
        };
        use super::super::*;
        if bag.mob != 0 || has_curated_boss_attribution(&bag.dungeon) {
            return Ok(false);
        }
        let Some(kind) = LootBagType::from_id(bag.bag_type).filter(|_| !bag.brown()) else {
            return Ok(false);
        };
        let mut kills = std::collections::BTreeMap::new();
        let mut add =
            |predicate: &str, order: &str, args: Vec<rusqlite::types::Value>| -> SqlResult<()> {
                let sql = format!(
                    "SELECT id,map_seed,boss_object_type,boss_name,started_at,ended_at FROM fights
                 WHERE map_seed=?1 AND id<=?2 AND killed=1 AND boss_object_type>0 AND {predicate}
                 ORDER BY {order} LIMIT 1"
                );
                let mut values = vec![
                    rusqlite::types::Value::Integer(i64::from(bag.seed)),
                    cutoff.into(),
                ];
                values.extend(args);
                let row = conn
                    .query_row(&sql, rusqlite::params_from_iter(values), |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            RecentBossKill {
                                map_seed: r.get(1)?,
                                object_type: r.get(2)?,
                                name: r.get(3)?,
                                started_at_ms: r.get(4)?,
                                ended_at_ms: r.get(5)?,
                            },
                        ))
                    })
                    .optional()?;
                if let Some((id, kill)) = row {
                    kills.insert(id, kill);
                }
                Ok(())
            };
        if bag_has_boss_mark(&bag.items) {
            add("1", "ended_at DESC,started_at DESC,id DESC", vec![])?;
        }
        if bag.items.contains(&MARK_OF_JANUS_ITEM_ID) {
            let variants = JANUS_VARIANT_TYPES
                .iter()
                .map(i32::to_string)
                .collect::<Vec<_>>()
                .join(",");
            add(
                &format!("(boss_name=?3 OR boss_object_type IN ({variants}))"),
                "ended_at DESC,id DESC",
                vec![JANUS_NAME.to_string().into()],
            )?;
        }
        if bag.seed != 0 {
            let mut types: Vec<i32> = boss_drop_candidates(&bag.dungeon, &bag.items)
                .into_iter()
                .map(|(t, _)| t)
                .collect();
            if is_realm_dungeon(&bag.dungeon) {
                types.extend([
                    YELLOW_BEEHEMOTH_OBJECT_TYPE,
                    RED_BEEHEMOTH_OBJECT_TYPE,
                    BLUE_BEEHEMOTH_OBJECT_TYPE,
                    CORRUPTED_BRAMBLETHORN_OBJECT_TYPE,
                    KEYPER_OBJECT_TYPE,
                ]);
                for t in [
                    YELLOW_BEEHEMOTH_OBJECT_TYPE,
                    RED_BEEHEMOTH_OBJECT_TYPE,
                    BLUE_BEEHEMOTH_OBJECT_TYPE,
                ] {
                    add(
                        "boss_object_type=?3 AND ended_at<=?4",
                        "ended_at DESC,id ASC",
                        vec![t.into(), bag.timestamp.into()],
                    )?;
                    add(
                        "boss_object_type=?3 AND ended_at>=?4",
                        "ended_at ASC,id ASC",
                        vec![t.into(), bag.timestamp.into()],
                    )?;
                }
            }
            types.sort_unstable();
            types.dedup();
            for t in types {
                let args = vec![
                    t.into(),
                    bag.timestamp
                        .saturating_sub(KILL_CORRELATION_MAX_LAG_MS)
                        .into(),
                    bag.timestamp
                        .saturating_add(KILL_CORRELATION_EARLY_TOLERANCE_MS)
                        .into(),
                ];
                add(
                    "boss_object_type=?3 AND ended_at BETWEEN ?4 AND ?5",
                    "ended_at DESC,id DESC",
                    args.clone(),
                )?;
                add(
                    "boss_object_type=?3 AND ended_at BETWEEN ?4 AND ?5",
                    "ended_at DESC,started_at DESC,id DESC",
                    args,
                )?;
            }
            if bag.dungeon == ORYX_SANCTUARY_NAME {
                add(
                    "boss_object_type=?3",
                    "id ASC",
                    vec![CHIEF_BEISA_OBJECT_TYPE.into()],
                )?;
            }
        }
        let kills: Vec<_> = kills.into_values().collect();
        self.set_repaired_source(
            bag,
            LootTracker::resolve_boss_override_with(
                &kills,
                kind,
                &bag.dungeon,
                0,
                &bag.items,
                bag.seed,
                bag.timestamp,
                false,
            ),
        )
    }
}

use super::*;

const BATCH: i64 = 64;
const GAP: i64 = 30 * 60 * 1000;

enum Kind {
    Reset,
    GenericScan,
    GenericApply,
    Runs,
    PruneRuns,
    Snapshot(bool),
    Group(bool),
    Purge(&'static [i32]),
    Label(i32),
    TimeRuns,
    TimeSingles,
    Classify,
}

const JOBS: &[(i32, &str, Kind)] = &[
    (6, "legacy-combat-006-reset", Kind::Reset),
    (6, "legacy-combat-006-scan", Kind::GenericScan),
    (6, "legacy-combat-006-apply", Kind::GenericApply),
    (6, "legacy-combat-006-runs", Kind::Runs),
    (19, "legacy-combat-019-label", Kind::Label(19)),
    (20, "legacy-combat-020-label", Kind::Label(20)),
    (20, "legacy-combat-020-snapshot", Kind::Snapshot(false)),
    (20, "legacy-combat-020-group", Kind::Group(false)),
    (24, "legacy-combat-024-purge", Kind::Purge(&[20553, 20577])),
    (
        25,
        "legacy-combat-025-purge",
        Kind::Purge(&[16933, 16971, 34462, 34587, 51077]),
    ),
    (
        29,
        "legacy-combat-029-purge",
        Kind::Purge(&[34457, 34458, 34459, 34467, 34468, 34469, 34474]),
    ),
    (30, "legacy-combat-030-purge", Kind::Purge(&[34552, 34556])),
    (
        31,
        "legacy-combat-031-purge",
        Kind::Purge(&[18000, 18003, 18047, 18049]),
    ),
    (32, "legacy-combat-032-label", Kind::Label(32)),
    (
        33,
        "legacy-combat-033-purge",
        Kind::Purge(&[18001, 18002, 18048, 18050]),
    ),
    (
        40,
        "legacy-combat-040-purge",
        Kind::Purge(&[24061, 24454, 24455, 24456, 24457, 24458, 24459]),
    ),
    (
        41,
        "legacy-combat-041-purge",
        Kind::Purge(&[
            23708, 23747, 23840, 23932, 24412, 44354, 44355, 44356, 44357, 44358, 44359, 44360,
            44361,
        ]),
    ),
    (41, "legacy-combat-041-runs", Kind::PruneRuns),
    (43, "legacy-combat-043-runs", Kind::TimeRuns),
    (43, "legacy-combat-043-singles", Kind::TimeSingles),
    (44, "legacy-combat-044-classify", Kind::Classify),
    (46, "legacy-combat-046-label", Kind::Label(46)),
    (
        47,
        "legacy-combat-047-purge",
        Kind::Purge(&[
            52513, 52522, 52531, 52532, 52533, 52534, 52535, 52536, 52537, 52538, 52539, 52540,
            52541, 52542, 52543, 52544, 52545, 52546, 52547, 52548, 52549, 52550, 52551, 52552,
            52553, 52554, 52555, 52558, 52559, 52562, 52568, 52569, 52570, 52571, 52572, 52573,
            52574, 52575, 52579, 52580, 52581, 52582, 52583, 52584, 52585, 52590, 52591, 52592,
            52653, 52654, 52655, 52657, 52658, 52659, 52660,
        ]),
    ),
    (
        49,
        "legacy-combat-049-purge",
        Kind::Purge(&[16993, 47928, 47929]),
    ),
    (49, "legacy-combat-049-snapshot", Kind::Snapshot(true)),
    (49, "legacy-combat-049-group", Kind::Group(true)),
    (50, "legacy-combat-050-purge", Kind::Purge(&[34465])),
    (50, "legacy-combat-050-runs", Kind::PruneRuns),
    (54, "legacy-combat-054-label", Kind::Label(54)),
    (59, "legacy-combat-059-label", Kind::Label(59)),
    (59, "legacy-combat-059-reset", Kind::Reset),
    (60, "legacy-combat-060-label", Kind::Label(60)),
];

struct Header {
    id: i64,
    boss: i32,
    dungeon: String,
    char_id: i32,
    seed: i32,
    started: i64,
    ended: i64,
    killed: bool,
    encounter: Option<String>,
    run: Option<String>,
    name: String,
    group: Option<String>,
}

impl CombatDatabase {
    pub fn historical_fight_cutoff(&self) -> SqlResult<i64> {
        self.conn
            .query_row("SELECT COALESCE(MAX(id),0) FROM fights", [], |r| r.get(0))
    }

    pub fn has_legacy_repairs(&self) -> SqlResult<bool> {
        self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM historical_repairs
                 WHERE repair_id LIKE 'legacy-combat-%' AND complete=0)
             OR EXISTS(SELECT 1 FROM legacy_combat_groups)
             OR EXISTS(SELECT 1 FROM legacy_combat_run_spans)",
            [],
            |r| r.get(0),
        )
    }

    pub(super) fn register_legacy_combat_repairs(&self, version: i32) -> SqlResult<()> {
        self.conn.execute_batch(maintenance::PROGRESS_SCHEMA)?;
        for (column, definition) in [("text_cursor", "TEXT NOT NULL DEFAULT ''")] {
            if !self.column_exists("historical_repairs", column)? {
                self.conn.execute_batch(&format!(
                    "ALTER TABLE historical_repairs ADD COLUMN {column} {definition};"
                ))?;
            }
        }
        let cutoff: i64 =
            self.conn
                .query_row("SELECT COALESCE(MAX(id),0) FROM fights", [], |r| r.get(0))?;
        self.conn
            .execute_batch("SAVEPOINT register_legacy_combat")?;
        let result = (|| {
            for &(target, id, _) in JOBS {
                if version < target {
                    self.conn.execute(
                        "INSERT OR IGNORE INTO historical_repairs(repair_id,cutoff)
                         VALUES (?1,?2)",
                        params![id, cutoff],
                    )?;
                }
            }
            self.conn.execute_batch("RELEASE register_legacy_combat")
        })();
        if result.is_err() {
            if let Err(error) = self
                .conn
                .execute_batch("ROLLBACK TO register_legacy_combat; RELEASE register_legacy_combat")
            {
                tracing::error!("[COMBAT] Legacy repair registration rollback failed: {error}");
            }
        }
        result
    }

    pub(super) fn legacy_combat_repair_batch(&mut self) -> SqlResult<Option<RepairBatch>> {
        for &(_, job, ref kind) in JOBS {
            let progress: Option<(i64, i64, i64, String)> = self
                .conn
                .query_row(
                    "SELECT cutoff,cursor,time_cursor,text_cursor
                 FROM historical_repairs WHERE repair_id=?1 AND complete=0",
                    [job],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            let Some((cutoff, cursor, time, text)) = progress else {
                continue;
            };
            if matches!(kind, Kind::Classify) && !crate::assets::get_asset_manager().is_loaded() {
                return Ok(Some(RepairBatch {
                    pending: true,
                    ..Default::default()
                }));
            }
            let (next, next_time, next_text, count, changed) = if matches!(
                kind,
                Kind::Runs | Kind::PruneRuns | Kind::TimeRuns
            ) {
                let runs: Vec<(String, String, i64, i64, bool)> = {
                    let mut stmt = self.conn.prepare(
                        "SELECT r.run_id,r.dungeon,r.started_at,r.ended_at,r.killed FROM encounter_runs r
                         WHERE r.run_id>?1 AND (r.live_origin=0 OR EXISTS(
                             SELECT 1 FROM fights f WHERE f.encounter_run_id=r.run_id AND f.id<=?2))
                         ORDER BY r.run_id LIMIT ?3",
                    )?;
                    let rows = stmt.query_map(params![text, cutoff, BATCH], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                    })?;
                    rows.collect::<SqlResult<_>>()?
                };
                for (run, dungeon, started, ended, killed) in &runs {
                    if matches!(kind, Kind::Runs | Kind::PruneRuns) {
                        self.refresh_legacy_run(run, matches!(kind, Kind::PruneRuns))?;
                    } else {
                        let span: (Option<i64>, Option<i64>) = self.conn.query_row(
                            "SELECT MIN(started_at),MAX(ended_at) FROM fights
                             WHERE encounter_run_id=?1 AND id<=?2",
                            params![run, cutoff],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )?;
                        self.add_legacy_time(
                            dungeon,
                            span.0.unwrap_or(*started),
                            span.1.unwrap_or(*ended),
                            *killed,
                        )?;
                    }
                }
                (
                    cursor,
                    time,
                    runs.last().map_or(text, |r| r.0.clone()),
                    runs.len(),
                    !runs.is_empty(),
                )
            } else {
                let ordered = matches!(kind, Kind::GenericScan | Kind::Group(_));
                let sql = if ordered {
                    "SELECT id,boss_object_type,dungeon,local_char_id,map_seed,started_at,ended_at,killed,
                            encounter_id,encounter_run_id,boss_name,boss_group FROM fights
                     WHERE id<=?1 AND (started_at,id)>(?3,?2) ORDER BY started_at,id LIMIT ?4"
                } else {
                    "SELECT id,boss_object_type,dungeon,local_char_id,map_seed,started_at,ended_at,killed,
                            encounter_id,encounter_run_id,boss_name,boss_group FROM fights
                     WHERE id<=?1 AND id>?2 AND ?3 IS NOT NULL ORDER BY id LIMIT ?4"
                };
                let rows: Vec<Header> = {
                    let mut stmt = self.conn.prepare(sql)?;
                    let rows = stmt.query_map(params![cutoff, cursor, time, BATCH], |r| {
                        Ok(Header {
                            id: r.get(0)?,
                            boss: r.get(1)?,
                            dungeon: r.get(2)?,
                            char_id: r.get(3)?,
                            seed: r.get(4)?,
                            started: r.get(5)?,
                            ended: r.get(6)?,
                            killed: r.get(7)?,
                            encounter: r.get(8)?,
                            run: r.get(9)?,
                            name: r.get(10)?,
                            group: r.get(11)?,
                        })
                    })?;
                    rows.collect::<SqlResult<_>>()?
                };
                for row in &rows {
                    match kind {
                        Kind::Reset if row.run.as_ref().is_some_and(|r| r.starts_with("bf-")) => {
                            self.conn.execute("UPDATE fights SET encounter_id=NULL,encounter_run_id=NULL WHERE id=?1",[row.id])?;
                            self.refresh_legacy_run(row.run.as_deref().unwrap(), true)?;
                        }
                        Kind::GenericScan => self.scan_legacy_group(row)?,
                        Kind::GenericApply => self.apply_legacy_group(row)?,
                        Kind::Snapshot(realm) => self.snapshot_legacy_run(job, row, *realm)?,
                        Kind::Group(realm) => self.group_legacy_leftover(job, row, *realm)?,
                        Kind::Purge(types) if types.contains(&row.boss) => {
                            self.conn.execute(
                                "DELETE FROM fight_participants WHERE fight_id=?1",
                                [row.id],
                            )?;
                            self.conn
                                .execute("DELETE FROM fights WHERE id=?1", [row.id])?;
                            if let Some(run) = &row.run {
                                self.refresh_legacy_run(run, true)?;
                            }
                        }
                        Kind::Label(version) => self.label_legacy_header(row, *version)?,
                        Kind::Classify => {
                            let group =
                                crate::assets::boss_group(&row.dungeon, row.boss, &row.name)
                                    .map(|g| g.as_str().to_string());
                            if group != row.group {
                                self.conn.execute(
                                    "UPDATE fights SET boss_group=?2 WHERE id=?1",
                                    params![row.id, group],
                                )?;
                            }
                        }
                        Kind::TimeSingles if row.encounter.is_none() => {
                            self.add_legacy_time(&row.dungeon, row.started, row.ended, row.killed)?;
                        }
                        _ => {}
                    }
                }
                (
                    rows.last().map_or(cursor, |r| r.id),
                    rows.last().map_or(time, |r| r.started),
                    text,
                    rows.len(),
                    !rows.is_empty(),
                )
            };
            self.conn.execute(
                "UPDATE historical_repairs SET cursor=?2,time_cursor=?3,text_cursor=?4,complete=?5
                 WHERE repair_id=?1",
                params![job, next, next_time, next_text, count < BATCH as usize],
            )?;
            return Ok(Some(RepairBatch {
                history_changed: changed,
                pending: true,
                ..Default::default()
            }));
        }
        for table in ["legacy_combat_groups", "legacy_combat_run_spans"] {
            let deleted = self.conn.execute(
                &format!("DELETE FROM {table} WHERE rowid IN (SELECT rowid FROM {table} LIMIT ?1)"),
                [BATCH],
            )?;
            if deleted > 0 {
                return Ok(Some(RepairBatch {
                    pending: true,
                    ..Default::default()
                }));
            }
        }
        Ok(None)
    }

    fn scan_legacy_group(&self, row: &Header) -> SqlResult<()> {
        if row.char_id == 0
            || row.seed == 0
            || !super::super::tracker::is_groupable_dungeon(&row.dungeon)
        {
            return Ok(());
        }
        let previous: Option<(i64, i64, String, Option<String>, bool)> = self
            .conn
            .query_row(
                "SELECT first_id,ended_at,run_id,live_run,conflict FROM legacy_combat_groups
             WHERE phase='generic' AND char_id=?1 AND dungeon=?2 AND map_seed=?3
             ORDER BY started_at DESC,first_id DESC LIMIT 1",
                params![row.char_id, row.dungeon, row.seed],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let (first, end, mut chosen, mut live, mut conflict) = previous
            .filter(|p| row.started.saturating_sub(p.1) <= GAP)
            .unwrap_or_else(|| (row.id, row.ended, String::new(), None, false));
        if let Some(run) = &row.run {
            if !run.starts_with("bf-") && !run.starts_with("dg-") {
                conflict |= live.as_ref().is_some_and(|old| old != run);
                live.get_or_insert_with(|| run.clone());
                chosen = live.clone().unwrap();
            } else if live.is_none() && (chosen.is_empty() || run < &chosen) {
                chosen = run.clone();
            }
        }
        self.conn.execute(
            "INSERT INTO legacy_combat_groups
             (phase,first_id,char_id,dungeon,map_seed,started_at,ended_at,run_id,live_run,conflict)
             VALUES ('generic',?1,?2,?3,?4,?5,?6,?7,?8,?9)
             ON CONFLICT(phase,first_id) DO UPDATE SET ended_at=excluded.ended_at,
                 run_id=excluded.run_id,live_run=excluded.live_run,conflict=excluded.conflict",
            params![
                first,
                row.char_id,
                row.dungeon,
                row.seed,
                row.started,
                end.max(row.ended),
                chosen,
                live,
                conflict
            ],
        )?;
        Ok(())
    }

    fn apply_legacy_group(&self, row: &Header) -> SqlResult<()> {
        let decision: Option<(i64, String, bool)> = self
            .conn
            .query_row(
                "SELECT first_id,run_id,conflict FROM legacy_combat_groups WHERE phase='generic'
             AND char_id=?1 AND dungeon=?2 AND map_seed=?3 AND started_at<=?4
             ORDER BY started_at DESC,first_id DESC LIMIT 1",
                params![row.char_id, row.dungeon, row.seed, row.started],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if let Some((first, mut run, false)) = decision {
            if run.is_empty() {
                run = format!("dg-{}-{:x}-{first}", row.char_id, row.seed as u32);
            }
            let encounter = row
                .encounter
                .as_deref()
                .filter(|e| *e != "dungeon_run")
                .unwrap_or("dungeon_run");
            self.assign_legacy_run(row, encounter, &run, &row.dungeon, row.killed)?;
        }
        Ok(())
    }

    fn assign_legacy_run(
        &self,
        row: &Header,
        encounter: &str,
        run: &str,
        dungeon: &str,
        killed: bool,
    ) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE fights SET encounter_id=?2,encounter_run_id=?3 WHERE id=?1",
            params![row.id, encounter, run],
        )?;
        self.conn.execute(
            "INSERT INTO encounter_runs(run_id,encounter_id,dungeon,started_at,ended_at,killed)
             VALUES (?1,'dungeon_run',?2,?3,?4,?5) ON CONFLICT(run_id) DO UPDATE SET
             started_at=MIN(started_at,excluded.started_at),ended_at=MAX(ended_at,excluded.ended_at),
             killed=MAX(killed,excluded.killed)",
            params![run,dungeon,row.started,row.ended,killed],
        )?;
        Ok(())
    }

    fn refresh_legacy_run(&self, run: &str, preserve_killed: bool) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE encounter_runs SET
             started_at=(SELECT MIN(started_at) FROM fights WHERE encounter_run_id=?1),
             ended_at=(SELECT MAX(ended_at) FROM fights WHERE encounter_run_id=?1),
             killed=CASE WHEN ?2 THEN killed ELSE (SELECT MAX(killed) FROM fights WHERE encounter_run_id=?1) END
             WHERE run_id=?1 AND EXISTS(SELECT 1 FROM fights WHERE encounter_run_id=?1)",
            params![run,preserve_killed],
        )?;
        self.conn.execute(
            "DELETE FROM encounter_runs WHERE run_id=?1 AND NOT EXISTS(SELECT 1 FROM fights WHERE encounter_run_id=?1)",[run],
        )?;
        Ok(())
    }

    fn snapshot_legacy_run(&self, job: &str, row: &Header, realm: bool) -> SqlResult<()> {
        let Some(run) = row.run.as_ref() else {
            return Ok(());
        };
        let encounter = if realm {
            let Some(enc) = crate::assets::encounter_for_boss_type(row.boss) else {
                return Ok(());
            };
            if super::super::tracker::is_groupable_dungeon(&row.dungeon)
                || !crate::assets::encounter_realm_grouped(enc.id)
            {
                return Ok(());
            }
            enc.id
        } else {
            "dungeon_run"
        };
        let killed: Option<bool> = self
            .conn
            .query_row(
                "SELECT killed FROM encounter_runs WHERE run_id=?1",
                [run],
                |r| r.get(0),
            )
            .optional()?;
        let Some(killed) = killed else { return Ok(()) };
        self.conn.execute(
            "INSERT INTO legacy_combat_run_spans
             (phase,run_id,encounter,char_id,dungeon,map_seed,started_at,ended_at,killed)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(phase,run_id) DO UPDATE SET
             char_id=CASE WHEN char_id=0 THEN excluded.char_id ELSE char_id END,
             map_seed=CASE WHEN map_seed=0 THEN excluded.map_seed ELSE map_seed END,
             started_at=MIN(started_at,excluded.started_at),ended_at=MAX(ended_at,excluded.ended_at),
             killed=MAX(killed,excluded.killed)",
            params![job,run,encounter,row.char_id,row.dungeon,row.seed,row.started,row.ended,killed],
        )?;
        Ok(())
    }

    fn group_legacy_leftover(&self, job: &str, row: &Header, realm: bool) -> SqlResult<()> {
        if row.run.is_some() || row.char_id == 0 || row.seed == 0 {
            return Ok(());
        }
        let encounter = if realm {
            let Some(enc) = crate::assets::encounter_for_boss_type(row.boss) else {
                return Ok(());
            };
            if super::super::tracker::is_groupable_dungeon(&row.dungeon)
                || !crate::assets::encounter_realm_grouped(enc.id)
            {
                return Ok(());
            }
            enc.id
        } else {
            if !super::super::tracker::is_groupable_dungeon(&row.dungeon) {
                return Ok(());
            }
            "dungeon_run"
        };
        let snapshot = if realm {
            "legacy-combat-049-snapshot"
        } else {
            "legacy-combat-020-snapshot"
        };
        let existing: Option<(String, String, i64, bool)> = self
            .conn
            .query_row(
                "SELECT run_id,dungeon,ended_at,killed FROM legacy_combat_run_spans
             WHERE phase=?1 AND char_id=?2 AND map_seed=?3 AND encounter=?4
             AND (?5 OR (dungeon=?6 AND ?7<=ended_at+?9 AND ?8>=started_at-?9))
             ORDER BY CASE WHEN ?5 THEN -ended_at ELSE 0 END,run_id LIMIT 1",
                params![
                    snapshot,
                    row.char_id,
                    row.seed,
                    encounter,
                    realm,
                    row.dungeon,
                    row.started,
                    row.ended,
                    GAP
                ],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let carry: Option<(String, String, i64, bool)> = self
            .conn
            .query_row(
                "SELECT run_id,dungeon,ended_at,killed FROM legacy_combat_run_spans
             WHERE phase=?1 AND char_id=?2 AND map_seed=?3 AND encounter=?4 AND (?5 OR dungeon=?6)
             ORDER BY ended_at DESC,run_id LIMIT 1",
                params![job, row.char_id, row.seed, encounter, realm, row.dungeon],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let adopt = !realm && existing.is_some();
        let candidate = if adopt { existing } else { carry.or(existing) };
        let selected = if realm || !adopt {
            candidate.filter(|p| row.started.saturating_sub(p.2) <= GAP)
        } else {
            candidate
        };
        let (run, dungeon, _, killed) = selected.unwrap_or_else(|| {
            (
                if realm {
                    format!(
                        "rr-{encounter}-{}-{:x}-{}",
                        row.char_id, row.seed as u32, row.id
                    )
                } else {
                    format!("di-{:x}-{}", row.seed as u32, row.id)
                },
                row.dungeon.clone(),
                row.ended,
                false,
            )
        });
        self.assign_legacy_run(row, encounter, &run, &dungeon, killed || row.killed)?;
        if !adopt {
            self.conn.execute(
                "INSERT INTO legacy_combat_run_spans
                 (phase,run_id,encounter,char_id,dungeon,map_seed,started_at,ended_at,killed)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(phase,run_id) DO UPDATE SET
                 started_at=MIN(started_at,excluded.started_at),ended_at=MAX(ended_at,excluded.ended_at),
                 killed=MAX(killed,excluded.killed)",
                params![job,run,encounter,row.char_id,dungeon,row.seed,row.started,row.ended,killed||row.killed],
            )?;
        }
        Ok(())
    }

    fn add_legacy_time(
        &self,
        dungeon: &str,
        started: i64,
        ended: i64,
        killed: bool,
    ) -> SqlResult<()> {
        if super::super::tracker::is_groupable_dungeon(dungeon) && ended > started {
            self.conn.execute(
                "INSERT INTO dungeon_time_totals(dungeon,total_time_ms,completions) VALUES (?1,?2,?3)
                 ON CONFLICT(dungeon) DO UPDATE SET total_time_ms=total_time_ms+excluded.total_time_ms,
                 completions=completions+excluded.completions",
                params![dungeon,ended.saturating_sub(started),killed],
            )?;
        }
        Ok(())
    }

    fn label_legacy_header(&self, row: &Header, version: i32) -> SqlResult<()> {
        if version == 54 {
            self.conn.execute(
                "UPDATE fights SET boss_name=?2 WHERE id=?1 AND boss_object_type=?3 AND boss_name<>?2",
                params![row.id, crate::assets::object_name_override(crate::assets::SHATTERS_QUEEN_TYPE)
                    .unwrap_or("The Shattered Queen"), crate::assets::SHATTERS_QUEEN_TYPE],
            )?;
            return Ok(());
        }
        let sql=match version {
            19 => "UPDATE fights SET dungeon='Wine Cellar' WHERE id=?1 AND boss_object_type=2354 AND dungeon<>'Wine Cellar'",
            20 => "UPDATE fights SET dungeon='Oryx''s Castle' WHERE id=?1 AND boss_object_type IN (8200,3448,3449) AND dungeon<>'Oryx''s Castle'",
            32 => "UPDATE fights SET boss_name=CASE boss_name WHEN 'Marble Colossus (First Coming)' THEN 'Marble Colossus (Pre-survival)' ELSE 'Marble Colossus (Post-survival)' END WHERE id=?1 AND boss_name IN ('Marble Colossus (First Coming)','Marble Colossus (Second Coming)')",
            46 => "UPDATE fights SET boss_name=substr(boss_name,1,length(boss_name)-length(' (Pre-survival)'))
                   WHERE id=?1 AND boss_object_type=45073 AND killed=1 AND boss_start_hp=boss_max_hp
                   AND boss_name LIKE '% (Pre-survival)' AND encounter_run_id IS NOT NULL
                   AND EXISTS(SELECT 1 FROM fight_participants p WHERE p.fight_id=fights.id
                              AND fights.local_object_id<>0 AND p.object_id=fights.local_object_id AND p.end_status='present')
                   AND NOT EXISTS(SELECT 1 FROM fights f2 WHERE f2.encounter_run_id=fights.encounter_run_id
                                  AND f2.boss_name LIKE '% (Post-survival)')",
            59 => "UPDATE fights SET dungeon='Oryx''s Castle' WHERE id=?1 AND dungeon='Realm' AND boss_object_type IN (8200,3448,3449,46385,46390,46391)",
            60 => "UPDATE fights SET dungeon=CASE boss_object_type WHEN 45973 THEN 'Oryx''s Chamber' ELSE 'Wine Cellar' END WHERE id=?1 AND dungeon='Realm' AND boss_object_type IN (45973,28989)",
            _ => unreachable!("known legacy label migration"),
        };
        self.conn.execute(sql, [row.id])?;
        Ok(())
    }

    pub(super) fn recover_retired_stat_cards(&mut self) -> SqlResult<RepairBatch> {
        let keys: Vec<String> = self
            .conn
            .prepare(
                "SELECT card_key FROM pending_stat_card_retirements ORDER BY card_key LIMIT 8",
            )?
            .query_map([], |r| r.get(0))?
            .collect::<SqlResult<_>>()?;
        let mut owners = HashSet::new();
        for key in &keys {
            let owner: Option<i32> = self
                .conn
                .query_row(
                    "SELECT char_id FROM stat_awards WHERE card_key=?1",
                    [key],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(owner) = owner {
                owners.insert(owner);
            }
            let exists: bool = if let Some(id) = key.strip_prefix('S') {
                self.conn.query_row("SELECT EXISTS(SELECT 1 FROM fights WHERE id=CAST(?1 AS INTEGER) AND encounter_run_id IS NULL)",[id],|r|r.get(0))?
            } else {
                self.conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM fights WHERE encounter_run_id=?1)",
                    [&key[1..]],
                    |r| r.get(0),
                )?
            };
            if exists {
                self.conn.execute(
                    "INSERT OR IGNORE INTO pending_stat_cards(card_key) VALUES (?1)",
                    [key],
                )?;
            } else {
                self.conn
                    .execute("DELETE FROM stat_awards WHERE card_key=?1", [key])?;
            }
            self.conn.execute(
                "DELETE FROM pending_stat_card_retirements WHERE card_key=?1",
                [key],
            )?;
        }
        let mut restored = self.persisted_combat_totals(&owners.into_iter().collect::<Vec<_>>())?;
        restored.pending = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pending_stat_card_retirements)",
            [],
            |r| r.get(0),
        )?;
        Ok(restored)
    }
}

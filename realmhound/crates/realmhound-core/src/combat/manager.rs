//! Combat tracking coordinator.
//!
//! Wires the pure [`CombatTracker`] engine to the [`CombatDatabase`] and exposes
//! the same event hooks the processor already calls for loot. Completed fights
//! are persisted here and kept briefly in memory for logging / later UI use.

use super::database::CombatDatabase;
use super::tracker::{is_groupable_dungeon, normalize_dungeon, CombatTracker};
use super::types::{CompletedFight, FightSelection};
use crate::assets::boss_group;
use crate::protocol::data::ObjectStatusData;
use crate::settings::CombatHistorySettings;

/// Coordinates fight reconstruction and persistence.
pub struct CombatManager {
    tracker: CombatTracker,
    database: Option<CombatDatabase>,
    /// Which boss groups / solo fights are recorded. A snapshot pushed from the
    /// UI settings; defaults to tracking everything.
    history_settings: CombatHistorySettings,
    /// Per-character authoritative lifetime award totals awaiting drain by the
    /// processor into the character cache.
    /// `char_id -> (lone, last, most)`.
    pending_stat_awards: std::collections::HashMap<i32, (i64, i64, i64)>,
    /// Per-character lifetime close-call totals summed from the persisted fight
    /// cards, awaiting drain by the processor. Applied as a monotonic backfill
    /// (raising a tally, never lowering it). `char_id -> total`.
    pending_close_call_totals: std::collections::HashMap<i32, i64>,
    /// In-flight per-dungeon time accumulator for the current run. Set on the
    /// map change that enters a groupable dungeon, updated as bosses die, and
    /// flushed into `dungeon_time_totals` on the next map change / disconnect.
    dungeon_timer: Option<DungeonRunTimer>,
    /// The most recently flushed run's `(map_seed, elapsed_ms)`, awaiting drain
    /// by the processor so the Live Feed timer can settle to the committed value.
    pending_dungeon_freeze: Option<(i32, i64)>,
    /// Realm-event encounters whose card should latch Completed because a loot
    /// bag from their core boss was seen this instance, keyed by
    /// `(map_seed, encounter_id)`. Loot usually drops before the escaped segment
    /// fights persist, so the signal is buffered and re-applied on every persist;
    /// cleared on map change. See [`crate::assets::encounter_loot_completes`].
    pending_loot_completions: std::collections::HashSet<(i32, &'static str)>,
}

/// Tracks the current dungeon run so its elapsed time can be accumulated into
/// the running per-dungeon counter, independent of the Combat History toggles.
struct DungeonRunTimer {
    /// Normalized dungeon / map name.
    dungeon: String,
    /// Map seed (fp) of the instance, used to target the matching Live Feed row.
    map_seed: i32,
    /// When the local player entered the instance (epoch millis).
    entered_at: i64,
    /// Latest killed non-add-on boss death (epoch millis); the run's upper
    /// bound when a boss was defeated. `None` if no boss was killed yet.
    boss_death: Option<i64>,
}

impl Default for CombatManager {
    fn default() -> Self {
        Self::new()
    }
}

impl CombatManager {
    /// Create a manager with no database (in-memory only).
    pub fn new() -> Self {
        Self {
            tracker: CombatTracker::new(),
            database: None,
            history_settings: CombatHistorySettings::default(),
            pending_stat_awards: std::collections::HashMap::new(),
            pending_close_call_totals: std::collections::HashMap::new(),
            dungeon_timer: None,
            pending_dungeon_freeze: None,
            pending_loot_completions: std::collections::HashSet::new(),
        }
    }

    /// Open (or create) the combat database for persistence at an explicit path.
    /// `loot_path` pairs it with the loot-history DB so loot-correlating
    /// migrations (the Legacy Lair of Draconis completion backfill) can read it.
    /// Logs and continues without persistence on failure.
    pub fn open_database(&mut self, path: &std::path::Path, loot_path: Option<&std::path::Path>) {
        match CombatDatabase::open_writer_with_loot(path, loot_path) {
            Ok(db) => {
                self.database = Some(db);
                tracing::info!("[COMBAT] Combat history database opened");
                // First reconciliation backfills every historical qualifying card
                // into the award ledger and buffers the increments.
                self.reconcile_awards();
                self.reconcile_close_calls();
            }
            Err(e) => {
                tracing::error!("[COMBAT] Failed to open combat database: {}", e);
            }
        }
    }

    /// Open (or create) the combat database, propagating the failure instead of
    /// degrading to in-memory. Selected-profile startup opens the combat writer
    /// before the loot writer (the loot backfill consults the combat DB) and
    /// must not silently continue without persistence. `loot_path` pairs it with
    /// the loot-history DB for loot-correlating migrations.
    pub fn open_database_strict(
        &mut self,
        path: &std::path::Path,
        loot_path: Option<&std::path::Path>,
    ) -> Result<(), rusqlite::Error> {
        let db = CombatDatabase::open_writer_with_loot(path, loot_path)?;
        self.database = Some(db);
        tracing::info!("[COMBAT] Combat history database opened (strict)");
        self.reconcile_awards();
        self.reconcile_close_calls();
        Ok(())
    }

    /// Checkpoint and close the combat database. No-op in in-memory mode.
    pub fn close(&mut self) -> Result<(), rusqlite::Error> {
        match self.database.take() {
            Some(db) => db.close(),
            None => Ok(()),
        }
    }

    /// Reconcile the award ledger and stage the authoritative per-character
    /// lifetime totals for the processor to apply to the character cache.
    fn reconcile_awards(&mut self) {
        if let Some(db) = self.database.as_mut() {
            match db.reconcile_stat_awards() {
                Ok(totals) => {
                    // Preserve targeted zeroes already staged for a previous
                    // owner that no longer has a row in the ledger.
                    self.pending_stat_awards.extend(totals);
                }
                Err(e) => tracing::error!("[COMBAT] Failed to reconcile stat awards: {}", e),
            }
        }
    }

    /// Drain the authoritative per-character lifetime award totals staged since
    /// the last call (Lone fighter / Last hero standing / Most damage taken).
    pub fn take_pending_stat_awards(&mut self) -> std::collections::HashMap<i32, (i64, i64, i64)> {
        std::mem::take(&mut self.pending_stat_awards)
    }

    fn reconcile_awards_for_selections(&mut self, selections: &[FightSelection]) {
        let Some(db) = self.database.as_mut() else {
            return;
        };
        let totals = match db.reconcile_stat_awards_for_selections(selections) {
            Ok(totals) => Ok(totals),
            Err(e) => {
                tracing::error!(
                    "[COMBAT] Failed to reconcile changed stat awards: {}; retrying full reconciliation",
                    e
                );
                db.reconcile_stat_awards()
            }
        };
        match totals {
            Ok(totals) => self.pending_stat_awards.extend(totals),
            Err(e) => tracing::error!("[COMBAT] Failed to reconcile stat awards: {}", e),
        }
    }

    /// Reconcile the per-character lifetime close-call totals from the persisted
    /// fight cards and stage them for the processor to backfill onto the
    /// character cache (monotonic: raises a tally, never lowers it).
    fn reconcile_close_calls(&mut self) {
        if let Some(db) = self.database.as_ref() {
            match db.reconcile_close_calls() {
                Ok(totals) => self.pending_close_call_totals = totals,
                Err(e) => tracing::error!("[COMBAT] Failed to reconcile close calls: {}", e),
            }
        }
    }

    /// Drain the per-character lifetime close-call totals staged since the last
    /// call.
    pub fn take_pending_close_call_totals(&mut self) -> std::collections::HashMap<i32, i64> {
        std::mem::take(&mut self.pending_close_call_totals)
    }

    /// Access the underlying engine (for tests / inspection).
    pub fn tracker(&self) -> &CombatTracker {
        &self.tracker
    }

    /// Drain the lifetime close-call counter accumulated in the tracker since the
    /// last call.
    pub fn take_pending_close_calls(&mut self) -> u32 {
        self.tracker.take_pending_close_calls()
    }

    /// Drain the most recently flushed dungeon run's `(map_seed, elapsed_ms)` so
    /// the Live Feed timer can settle to the committed per-run value.
    pub fn take_dungeon_freeze(&mut self) -> Option<(i32, i64)> {
        self.pending_dungeon_freeze.take()
    }

    /// Replace the combat-history tracking settings (pushed from the UI when the
    /// user changes them). Affects only fights recorded after this call.
    pub fn set_history_settings(&mut self, settings: CombatHistorySettings) {
        self.history_settings = settings;
    }

    /// Whether a completed fight should be recorded given the current settings.
    /// Solo fights (only the local player) are gated by `track_solo`; every fight
    /// with a curated boss group is gated by that group's toggle. Fights that map
    /// to no group are always recorded.
    fn should_record(&self, fight: &CompletedFight) -> bool {
        let s = &self.history_settings;
        let has_others = fight
            .participants
            .iter()
            .any(|p| !p.is_local && !p.name.trim().is_empty());
        if !has_others && !s.track_solo {
            return false;
        }
        match boss_group(&fight.dungeon, fight.boss_object_type, &fight.boss_name) {
            Some(group) => s.tracks(group),
            None => true,
        }
    }

    // --- Event hooks (mirrors the loot tracker call sites in the processor) ---

    /// Local player loaded into a map.
    pub fn on_player_loaded(&mut self, object_id: i32, char_id: i32) {
        self.tracker.on_player_loaded(object_id, char_id);
    }

    /// Entered a new map.
    pub fn on_map_change(&mut self, map_name: &str, map_seed: i32, time_ms: i64) {
        let finished = self.tracker.on_map_change(map_name, map_seed, time_ms);
        self.persist(finished);
        // The escaped segment fights of a loot-completed realm event persist here
        // (on leaving the realm), so apply any buffered loot completions before
        // the signal is dropped, then clear it for the next instance.
        self.apply_pending_loot_completions();
        self.pending_loot_completions.clear();
        // Close out the run we just left, then arm a timer if the new map is a
        // groupable dungeon instance (Realm / Nexus / hubs are excluded).
        self.flush_dungeon_timer(time_ms);
        let normalized = normalize_dungeon(map_name);
        self.dungeon_timer = is_groupable_dungeon(&normalized).then(|| DungeonRunTimer {
            dungeon: normalized,
            map_seed,
            entered_at: time_ms,
            boss_death: None,
        });
    }

    /// A loot bag was attributed to a mob. Realm events whose core boss can drop
    /// its bag without ever being seen dying (Towering Perfection) latch their
    /// card to Completed on the bag, even when only the segments were damaged.
    pub fn on_boss_loot(&mut self, mob_type: i32, map_seed: i32) {
        if map_seed == 0 {
            return;
        }
        if let Some(enc_id) = crate::assets::encounter_loot_completes(mob_type) {
            self.pending_loot_completions.insert((map_seed, enc_id));
            // The run may already be persisted (bag walked over after the segments
            // finalized); mark it now. Buffering also covers the common
            // loot-before-persist ordering, re-applied on the next persist.
            self.apply_pending_loot_completions();
        }
    }

    /// Latch `killed` on every persisted encounter run matching a buffered
    /// loot-completion signal. Idempotent: a run stays killed once set.
    fn apply_pending_loot_completions(&mut self) {
        let Some(db) = self.database.as_mut() else {
            return;
        };
        for &(map_seed, enc_id) in &self.pending_loot_completions {
            if let Err(e) = db.mark_encounter_killed_by_loot(map_seed, enc_id) {
                tracing::warn!("[COMBAT] loot-completion update failed: {}", e);
            }
        }
    }

    /// Disconnected from the server.
    pub fn on_disconnect(&mut self, time_ms: i64) {
        let finished = self.tracker.on_disconnect(time_ms);
        self.persist(finished);
        self.flush_dungeon_timer(time_ms);
    }

    /// A new object spawned (`Update`).
    pub fn on_object_spawn(
        &mut self,
        object_id: i32,
        object_type: i32,
        status: &ObjectStatusData,
        time_ms: i64,
    ) {
        self.tracker
            .on_object_spawn(object_id, object_type, status, time_ms);
    }

    /// An existing object's stats changed (`NewTick`).
    pub fn on_object_status(&mut self, object_id: i32, status: &ObjectStatusData, time_ms: i64) {
        let finished = self.tracker.on_object_status(object_id, status, time_ms);
        self.persist(finished);
    }

    /// An object was removed from the world (`Update` drop).
    pub fn on_object_removed(&mut self, object_id: i32, time_ms: i64) {
        if let Some(fight) = self.tracker.on_object_removed(object_id, time_ms) {
            self.persist(vec![fight]);
        }
    }

    /// A boss taunt (`TextPacket` from the boss's own object id) arrived. Drives
    /// the Marble Colossus survival-phase split off its authoritative
    /// second-coming taunt, persisting the finalized pre-survival segment.
    pub fn on_boss_text(&mut self, object_id: i32, text: &str, time_ms: i64) {
        let finished = self.tracker.on_boss_text(object_id, text, time_ms);
        self.persist(finished);
    }

    /// The local player died (`CharacterDied`). Buffers the death so the
    /// finalizing fight can mark the local participant as dead.
    pub fn on_local_death(&mut self, char_id: i32, gravestone_type: i32, time_ms: i64) {
        self.tracker
            .on_local_death(char_id, gravestone_type, time_ms);
        // A local death ends the run: count entry -> death (or entry -> the boss
        // kill that preceded it). The following nexus/disconnect flush is a no-op.
        self.flush_dungeon_timer(time_ms);
    }

    /// A remote player died, parsed from the server death-broadcast text. Buffers
    /// the death by name so the finalizing fight marks that participant as dead
    /// even when their gravestone couldn't be position-correlated.
    pub fn on_remote_death(&mut self, player_name: &str, time_ms: i64) {
        self.tracker.on_remote_death(player_name, time_ms);
    }

    /// Incoming `DamagePacket`: `attacker_id` dealt `amount` to `target_id`.
    pub fn on_damage(&mut self, target_id: i32, attacker_id: i32, amount: i64, time_ms: i64) {
        self.tracker
            .on_damage(target_id, attacker_id, amount, time_ms);
    }

    /// Outgoing local `EnemyHit` on `target_id` by bullet `bullet_id`. `shooter_id`
    /// is the firing entity (local player or a summon); `main_id` the owning player.
    pub fn on_local_hit(
        &mut self,
        target_id: i32,
        bullet_id: i16,
        shooter_id: i32,
        main_id: i32,
        time_ms: i64,
    ) {
        self.tracker
            .on_local_hit(target_id, bullet_id, shooter_id, main_id, time_ms);
    }

    /// Outgoing local `PlayerShoot`: rolls the shot's damage for later hit
    /// correlation. Must be called for every local shot to keep the RNG in sync.
    pub fn on_local_shot(
        &mut self,
        weapon_id: i32,
        projectile_id: i32,
        bullet_id: i16,
        client_time: i32,
        shot_x: f32,
        shot_y: f32,
        angle: f32,
        time_ms: i64,
    ) {
        self.tracker.on_local_shot(
            weapon_id,
            projectile_id,
            bullet_id,
            client_time,
            shot_x,
            shot_y,
            angle,
            time_ms,
        );
    }

    /// Incoming `ServerPlayerShoot`: a summon/ally shot with server-precomputed
    /// damage. Learns summon ownership and stashes the local player's own summon
    /// damage for later hit correlation.
    pub fn on_ally_shot(
        &mut self,
        owner_id: i32,
        summoner_id: i32,
        bullet_id: i16,
        bullet_count: i8,
        damage: i16,
        container_type: i32,
        bullet_type: i8,
    ) {
        self.tracker.on_ally_shot(
            owner_id,
            summoner_id,
            bullet_id,
            bullet_count,
            damage,
            container_type,
            bullet_type,
        );
    }

    /// Incoming `EnemyShoot`: stashes the enemy projectile's pre-defense damage so
    /// a later local `PlayerHit` can estimate the local player's damage taken.
    pub fn on_enemy_shot(
        &mut self,
        owner_id: i32,
        bullet_id: i16,
        bullet_type: u8,
        damage: i16,
        num_shots: u8,
    ) {
        self.tracker
            .on_enemy_shot(owner_id, bullet_id, bullet_type, damage, num_shots);
    }

    /// Outgoing local `PlayerHit`: the local player was hit by `enemy_id`'s bullet
    /// `bullet_id`. Estimates and records the local player's damage taken.
    pub fn on_player_hit(&mut self, bullet_id: i16, enemy_id: i32, time_ms: i64) {
        self.tracker.on_player_hit(bullet_id, enemy_id, time_ms);
    }

    /// Outgoing local `UseItem`: opens a Knight-shield damage-reduction window
    /// when the used item is a shield, and self-computes orb ability damage
    /// centered on the throw position `(x, y)`.
    pub fn on_use_item(&mut self, item_id: i32, x: f32, y: f32, time_ms: i64) {
        self.tracker.on_use_item(item_id, x, y, time_ms);
    }

    /// Periodic tick: finalize killed / timed-out fights.
    pub fn on_tick(&mut self, time_ms: i64) {
        #[cfg(feature = "latency-diagnostics")]
        let started = std::time::Instant::now();
        let finished = self.tracker.on_tick(time_ms);
        #[cfg(feature = "latency-diagnostics")]
        let fight_count = finished.len();
        #[cfg(feature = "latency-diagnostics")]
        let participant_count = finished
            .iter()
            .map(|fight| fight.participant_count())
            .sum::<usize>();
        self.persist(finished);
        #[cfg(feature = "latency-diagnostics")]
        {
            let duration = started.elapsed();
            if duration >= std::time::Duration::from_millis(100) {
                tracing::warn!(
                    "[LATENCY][STORAGE] operation=combat_on_tick_and_persist duration_ms={} \
                     fight_count={} participant_count={}",
                    duration.as_millis(),
                    fight_count,
                    participant_count
                );
            }
        }
    }

    fn persist(&mut self, fights: Vec<CompletedFight>) {
        let mut changed_cards = std::collections::HashSet::new();
        for fight in fights {
            // Advance the current dungeon run's upper bound as real bosses die,
            // independent of whether the fight is tracked in Combat History.
            self.note_boss_death(&fight);
            let recorded = self.should_record(&fight);
            let outcome = if recorded { "recorded" } else { "untracked" };
            tracing::info!(
                "[COMBAT] Fight {}: boss='{}' dungeon='{}' killed={} duration={}s participants={} observed_dmg={} tracked={}",
                outcome,
                fight.boss_name,
                fight.dungeon,
                fight.killed,
                fight.duration_ms() / 1000,
                fight.participant_count(),
                fight.total_damage(),
                recorded,
            );
            if let Some(me) = fight.participants.iter().find(|p| p.is_local) {
                tracing::info!(
                    "[COMBAT]   You: damage={} hits={} provenance={}",
                    me.damage,
                    me.hits,
                    me.provenance.as_str(),
                );
            }
            if !recorded {
                continue;
            }
            if let Some(db) = self.database.as_mut() {
                match db.insert_fight(&fight) {
                    Ok(fight_id) => {
                        let selection = fight
                            .encounter_run_id
                            .as_ref()
                            .map_or(FightSelection::Single(fight_id), |run_id| {
                                FightSelection::Encounter(run_id.clone())
                            });
                        changed_cards.insert(selection);
                    }
                    Err(e) => {
                        tracing::error!("[COMBAT] Failed to persist fight: {}", e);
                    }
                }
            }
        }
        // A newly recorded fight may complete a qualifying card; award it now.
        if !changed_cards.is_empty() {
            // A buffered loot bag (Towering Perfection core) latches Completed on
            // the escaped segment run that was just persisted. Loot-completed
            // encounters are currently non-Exaltation; if that changes, include
            // every affected run in `changed_cards`.
            self.apply_pending_loot_completions();
            let changed_cards: Vec<FightSelection> = changed_cards.into_iter().collect();
            self.reconcile_awards_for_selections(&changed_cards);
            self.reconcile_close_calls();
        }
    }

    /// Advance the current dungeon run's boss-death upper bound when a real
    /// (non-add-on) boss is killed inside the instance this timer belongs to.
    fn note_boss_death(&mut self, fight: &CompletedFight) {
        if !fight.killed || Self::is_addon_boss(fight.boss_object_type) {
            return;
        }
        if let Some(timer) = self.dungeon_timer.as_mut() {
            // Only credit deaths belonging to this run's instance.
            if fight.dungeon_entered_at == Some(timer.entered_at) {
                let death = timer
                    .boss_death
                    .map_or(fight.ended_at, |prev| prev.max(fight.ended_at));
                timer.boss_death = Some(death);
            }
        }
    }

    /// Close out the active dungeon run, adding its elapsed time to the running
    /// per-dungeon counter. Time is measured entry -> final boss death when a
    /// boss was killed, else entry -> `exit_time` (the player left / died).
    fn flush_dungeon_timer(&mut self, exit_time: i64) {
        let Some(timer) = self.dungeon_timer.take() else {
            return;
        };
        let end_ref = timer.boss_death.unwrap_or(exit_time);
        let elapsed = (end_ref - timer.entered_at).max(0);
        // Settle the Live Feed timer to the committed per-run value regardless of
        // whether the elapsed is large enough to record in the DB.
        self.pending_dungeon_freeze = Some((timer.map_seed, elapsed));
        if elapsed <= 0 {
            return;
        }
        let completed = timer.boss_death.is_some();
        if let Some(db) = self.database.as_mut() {
            if let Err(e) = db.add_dungeon_time(&timer.dungeon, elapsed, completed) {
                tracing::error!("[COMBAT] Failed to accumulate dungeon time: {}", e);
            }
        }
    }

    /// Whether a boss object type is an add-on (aux member such as a boss's
    /// minion, or a treasure crate) rather than a real boss. Add-ons never
    /// bound a run's time.
    fn is_addon_boss(boss_type: i32) -> bool {
        crate::assets::aux_target_for_type(boss_type).is_some()
            || crate::assets::is_treasure_crate_type(boss_type)
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::{DamageProvenance, FightParticipant};
    use super::*;

    fn fight(dungeon: &str, boss: &str, solo: bool) -> CompletedFight {
        let mut participants = vec![FightParticipant {
            object_id: 1,
            object_type: 0x0321,
            skin_id: 0,
            tex1: 0,
            tex2: 0,
            name: "You".to_string(),
            equipment: [0; 4],
            equipment_enchants: Default::default(),
            damage: 100,
            hits: 10,
            is_local: true,
            provenance: DamageProvenance::SelfComputed,
            end_status: super::super::types::ParticipantEndStatus::Present,
            pet: None,
            damage_taken: None,
            damage_taken_provenance: super::super::types::DamageTakenProvenance::Observed,
            damage_blocked: None,
            guarded_damage: None,
            guarded_hits: None,
        }];
        if !solo {
            participants.push(FightParticipant {
                object_id: 2,
                object_type: 0x0321,
                skin_id: 0,
                tex1: 0,
                tex2: 0,
                name: "Ally".to_string(),
                equipment: [0; 4],
                equipment_enchants: Default::default(),
                damage: 200,
                hits: 20,
                is_local: false,
                provenance: DamageProvenance::Observed,
                end_status: super::super::types::ParticipantEndStatus::Present,
                pet: None,
                damage_taken: None,
                damage_taken_provenance: super::super::types::DamageTakenProvenance::Observed,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
            });
        }
        CompletedFight {
            started_at: 0,
            ended_at: 1000,
            dungeon: dungeon.to_string(),
            dungeon_entered_at: None,
            map_seed: 1,
            boss_object_type: 100,
            boss_name: boss.to_string(),
            boss_max_hp: 1000,
            boss_start_hp: 1000,
            local_object_id: 1,
            local_char_id: 7,
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
    fn disabled_group_is_not_recorded() {
        let mut m = CombatManager::new();
        let mut s = CombatHistorySettings::default();
        s.track_adept = false; // Undead Lair (3.5) is Adept.
        m.set_history_settings(s);
        assert!(!m.should_record(&fight("Undead Lair", "Septavius", false)));
        // Exaltation stays enabled.
        assert!(m.should_record(&fight("The Shatters", "The Forgotten King", false)));
    }

    #[test]
    fn solo_gate_only_affects_solo_fights() {
        let mut m = CombatManager::new();
        let mut s = CombatHistorySettings::default();
        s.track_solo = false;
        m.set_history_settings(s);
        assert!(!m.should_record(&fight("The Shatters", "King", true)));
        assert!(m.should_record(&fight("The Shatters", "King", false)));
    }

    #[test]
    fn ungrouped_fight_is_always_recorded() {
        let mut m = CombatManager::new();
        // An unrated realm map with a boss that maps to no group is ungated even
        // when every group toggle is off.
        let s = CombatHistorySettings {
            track_exaltation: false,
            track_expert: false,
            track_adept: false,
            track_beginner: false,
            track_beacon_guardians: false,
            track_rookie_heroes: false,
            track_adept_heroes: false,
            track_veteran_heroes: false,
            track_adept_encounters: false,
            track_veteran_encounters: false,
            track_seasonal_encounters: false,
            track_treasure_crates: false,
            track_solo: true,
        };
        m.set_history_settings(s);
        assert!(m.should_record(&fight("Realm of the Mad God", "Realm Critter", false)));
    }

    fn killed_boss(dungeon: &str, entered_at: i64, ended_at: i64) -> CompletedFight {
        let mut f = fight(dungeon, "King", true);
        f.dungeon_entered_at = Some(entered_at);
        f.ended_at = ended_at;
        f.killed = true;
        f
    }

    #[test]
    fn dungeon_timer_counts_entry_to_boss_death_on_completion() {
        let mut m = CombatManager::new();
        m.database = Some(super::super::database::CombatDatabase::open_in_memory().unwrap());
        m.on_map_change("The Shatters", 42, 1_000);
        m.persist(vec![killed_boss("The Shatters", 1_000, 5_000)]);
        // Player lingers, then leaves: post-kill time is not counted.
        m.on_map_change("Nexus", 0, 9_000);

        let totals = m.database.as_ref().unwrap().dungeon_time_totals().unwrap();
        assert_eq!(totals.len(), 1);
        assert_eq!(totals[0].dungeon, "The Shatters");
        assert_eq!(totals[0].total_time_ms, 4_000);
        assert_eq!(totals[0].completions, 1);
    }

    #[test]
    fn dungeon_timer_counts_entry_to_exit_when_boss_not_killed() {
        let mut m = CombatManager::new();
        m.database = Some(super::super::database::CombatDatabase::open_in_memory().unwrap());
        m.on_map_change("The Nest", 7, 1_000);
        // No boss kill; player disconnects mid-run.
        m.on_disconnect(4_000);

        let totals = m.database.as_ref().unwrap().dungeon_time_totals().unwrap();
        assert_eq!(totals.len(), 1);
        assert_eq!(totals[0].dungeon, "The Nest");
        assert_eq!(totals[0].total_time_ms, 3_000);
        assert_eq!(totals[0].completions, 0);
    }

    #[test]
    fn dungeon_timer_ignores_realm_and_hubs() {
        let mut m = CombatManager::new();
        m.database = Some(super::super::database::CombatDatabase::open_in_memory().unwrap());
        m.on_map_change("{s.rotmg}", 1, 1_000); // normalizes to Realm
        m.on_map_change("Nexus", 0, 9_000);
        assert!(m
            .database
            .as_ref()
            .unwrap()
            .dungeon_time_totals()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn dungeon_timer_flushes_on_local_death_before_boss_kill() {
        let mut m = CombatManager::new();
        m.database = Some(super::super::database::CombatDatabase::open_in_memory().unwrap());
        m.on_map_change("The Shatters", 42, 1_000);
        // Player dies at t=3000 without killing the boss; later nexus is a no-op.
        m.on_local_death(7, 0, 3_000);
        m.on_map_change("Nexus", 0, 20_000);

        let totals = m.database.as_ref().unwrap().dungeon_time_totals().unwrap();
        assert_eq!(totals.len(), 1);
        assert_eq!(totals[0].dungeon, "The Shatters");
        assert_eq!(totals[0].total_time_ms, 2_000);
        assert_eq!(totals[0].completions, 0);
    }

    #[test]
    fn flush_records_dungeon_freeze_with_seed_and_committed_elapsed() {
        let mut m = CombatManager::new();
        m.database = Some(super::super::database::CombatDatabase::open_in_memory().unwrap());
        m.on_map_change("The Shatters", 42, 1_000);
        m.persist(vec![killed_boss("The Shatters", 1_000, 5_000)]);
        // No freeze until the run ends.
        assert_eq!(m.take_dungeon_freeze(), None);
        // Leaving to the nexus flushes: freeze carries the seed and the committed
        // entry -> boss-death elapsed (post-kill lingering excluded).
        m.on_map_change("Nexus", 0, 9_000);
        assert_eq!(m.take_dungeon_freeze(), Some((42, 4_000)));
        // Drained once.
        assert_eq!(m.take_dungeon_freeze(), None);
    }
}

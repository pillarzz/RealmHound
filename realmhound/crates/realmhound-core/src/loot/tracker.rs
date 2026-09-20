//! Loot tracker coordinator.
//!
//! Coordinates all loot tracking components and handles packet events.
//! This is the main entry point for the loot tracking system.

use chrono::Utc;

use std::collections::{HashMap, HashSet};

use crate::api::character::parse_enchant_ids;
use crate::assets::get_asset_manager;
use crate::protocol::data::{ObjectStatusData, StatType};
use crate::settings::LootTrackingSettings;

use super::attribution::LootAttributionManager;
use super::bag_types::LootBagType;
use super::database::{LootDatabase, NewLootDrop, NewLootItem};
use super::detection::{LootDetector, PendingBag};
use super::player_state::{PlayerSnapshot, PlayerStateTracker};

/// How long after a boss kill a bag may still be correlated to it (millis).
const KILL_CORRELATION_MAX_LAG_MS: i64 = 90_000;
/// Tolerance for a bag whose timestamp slightly precedes the fight finalize.
const KILL_CORRELATION_EARLY_TOLERANCE_MS: i64 = 15_000;

/// Tight window for correlating an *Unknown, colourless* Killer Bee Nest bag to
/// a Beehemoth kill: bags drop ~1s after their bee dies, so a short lag keeps
/// each per-colour bag on its own bee while never stealing unrelated Realm bags.
const BEEHEMOTH_UNKNOWN_MAX_LAG_MS: i64 = 20_000;
/// A Beehemoth bag may log a few seconds before the fight finalize.
const BEEHEMOTH_UNKNOWN_EARLY_TOLERANCE_MS: i64 = 5_000;

/// How recently a Tomb main boss must have been seen with HP>0 to count as
/// "still alive on-screen" and thus be excluded as an offscreen-bag candidate.
const TOMB_BOSS_ALIVE_WINDOW_MS: u64 = 4_000;

/// Live-state tracking for the three Tomb of the Ancients main bosses, used to
/// attribute bags from bosses that died offscreen. A boss counts as alive while
/// its entity is on-screen reporting HP>0; once it stops (killed or the player
/// left the room) it becomes a candidate for an unattributed bag.
#[derive(Default)]
struct TombState {
    /// Map seed the tracked state belongs to.
    map_seed: i32,
    /// Object id -> main-boss object type, for entities seen this instance.
    boss_object_types: HashMap<i32, i32>,
    /// Main-boss object type -> last game time (ms) observed with HP>0.
    last_alive_ms: HashMap<i32, u64>,
    /// Main-boss object types already credited a bag this instance, so the
    /// alphabetical fallback hands distinct offscreen bags to distinct bosses.
    attributed: HashSet<i32>,
    /// Main-boss object types in the order their unique rage-phase taunt fired.
    /// A taunt fires once shortly before that boss dies, so this order tracks
    /// death order -- used to hand distinct offscreen bags to distinct bosses in
    /// the order they died rather than alphabetically. This is a heuristic:
    /// taunt order approximates but does not guarantee death order when rage
    /// phases overlap, so it only refines the previously-alphabetical fallback
    /// and never overrides a proximity/unique-drop attribution.
    rage_order: Vec<i32>,
    /// Main bosses proven dead this instance (e.g. the Mark of Geb only spawns
    /// once all three main bosses are defeated). A confirmed-dead boss overrides
    /// the on-screen alive window so its offscreen bag can still be attributed.
    confirmed_dead: HashSet<i32>,
}

impl TombState {
    fn reset(&mut self, map_seed: i32) {
        self.map_seed = map_seed;
        self.boss_object_types.clear();
        self.last_alive_ms.clear();
        self.attributed.clear();
        self.rage_order.clear();
        self.confirmed_dead.clear();
    }

    fn note_spawn(&mut self, object_id: i32, object_type: i32, hp: Option<i32>, time_ms: u64) {
        if !super::is_tomb_main_boss(object_type) {
            return;
        }
        self.boss_object_types.insert(object_id, object_type);
        // A boss proven dead this instance never revives, so ignore late spawns.
        if self.confirmed_dead.contains(&object_type) {
            return;
        }
        // A spawn with no HP stat still means the boss is present and alive.
        if hp.map_or(true, |h| h > 0) {
            self.last_alive_ms.insert(object_type, time_ms);
        }
    }

    fn note_update(&mut self, object_id: i32, hp: Option<i32>, time_ms: u64) {
        if let Some(&object_type) = self.boss_object_types.get(&object_id) {
            // A boss proven dead this instance never revives, so ignore late HP.
            if self.confirmed_dead.contains(&object_type) {
                return;
            }
            if hp.is_some_and(|h| h > 0) {
                self.last_alive_ms.insert(object_type, time_ms);
            }
        }
    }

    fn is_alive(&self, object_type: i32, now_ms: u64) -> bool {
        if self.confirmed_dead.contains(&object_type) {
            return false;
        }
        self.last_alive_ms
            .get(&object_type)
            .is_some_and(|&t| now_ms.saturating_sub(t) <= TOMB_BOSS_ALIVE_WINDOW_MS)
    }

    /// Mark a main boss as proven dead this instance, overriding the on-screen
    /// alive window so its offscreen bag becomes an attribution candidate.
    fn note_confirmed_dead(&mut self, object_type: i32) {
        if !super::is_tomb_main_boss(object_type) {
            return;
        }
        self.confirmed_dead.insert(object_type);
        self.last_alive_ms.remove(&object_type);
    }

    /// Main bosses eligible for an offscreen bag, alphabetical: not currently
    /// alive on-screen and not already credited a bag this instance.
    fn dead_candidates(&self, now_ms: u64) -> Vec<(i32, &'static str)> {
        super::TOMB_MAIN_BOSSES
            .iter()
            .copied()
            .filter(|(t, _)| !self.is_alive(*t, now_ms) && !self.attributed.contains(t))
            .collect()
    }

    /// Record a boss's rage-phase taunt. The taunt proves the boss is alive at
    /// this instant (rule: a taunting boss is still fighting, so it is excluded
    /// from offscreen-bag candidates until its alive window lapses) and pins its
    /// place in death order for later offscreen bag attribution.
    fn note_taunt(&mut self, object_type: i32, time_ms: u64) {
        self.last_alive_ms.insert(object_type, time_ms);
        if !self.rage_order.contains(&object_type) {
            self.rage_order.push(object_type);
        }
    }

    /// Pick the earliest-raged main boss that has since died (past its alive
    /// window) and is not yet credited a bag this instance. Assigns distinct
    /// offscreen bags to distinct bosses in death order, which is far more
    /// reliable than the alphabetical fallback. Returns `None` when no raged
    /// boss qualifies (rage skipped, or all raged bosses already credited).
    fn raged_candidate(&self, now_ms: u64) -> Option<(i32, &'static str)> {
        self.rage_order
            .iter()
            .copied()
            .find(|t| !self.is_alive(*t, now_ms) && !self.attributed.contains(t))
            .and_then(|t| {
                super::TOMB_MAIN_BOSSES
                    .iter()
                    .copied()
                    .find(|(bt, _)| *bt == t)
            })
    }
}

/// Attribution state for an Ice Tomb run. The three bosses each spawn a soul on
/// death; the souls linger until all three bosses are dead, then die together
/// and drop their loot. Because seeing any soul bag proves all three bosses are
/// dead, there is no alive-window to track: this only needs to remember which
/// bosses already hold a bag (to hand distinct ambiguous bags to distinct
/// bosses) and the order their rage taunts fired (to refine that distribution).
#[derive(Default)]
struct IceTombState {
    /// Boss names already credited a bag this run, so an ambiguous later bag is
    /// handed to a boss that does not yet have one.
    attributed: HashSet<&'static str>,
    /// Boss names in the order their unique rage taunt fired (approximate death
    /// order), used to refine the alphabetical distinct-bag fallback.
    rage_order: Vec<&'static str>,
}

impl IceTombState {
    fn reset(&mut self) {
        self.attributed.clear();
        self.rage_order.clear();
    }

    fn note_taunt(&mut self, boss: &'static str) {
        if !self.rage_order.contains(&boss) {
            self.rage_order.push(boss);
        }
    }

    fn credit(&mut self, boss: &'static str) {
        self.attributed.insert(boss);
    }

    /// The next boss without a bag yet, preferring rage-taunt (death) order and
    /// falling back to the alphabetical roster.
    fn next_uncredited(&self) -> Option<&'static str> {
        self.rage_order
            .iter()
            .copied()
            .find(|b| !self.attributed.contains(b))
            .or_else(|| {
                super::ICE_TOMB_BOSSES
                    .iter()
                    .copied()
                    .find(|b| !self.attributed.contains(b))
            })
    }
}

/// Read an entity's current HP from a status update, if present.
fn read_hp(status: &ObjectStatusData) -> Option<i32> {
    status
        .stats
        .iter()
        .find(|s| s.stat_type_id == StatType::HP as u8)
        .map(|s| s.stat_value)
}

/// A fully processed loot drop ready for display/storage.
#[derive(Debug, Clone)]
pub struct ProcessedLootDrop {
    /// Timestamp (Unix millis)
    pub timestamp: i64,
    /// Player snapshot at time of drop
    pub player: PlayerSnapshot,
    /// Bag type
    pub bag_type: LootBagType,
    /// Mob type ID that dropped the bag
    pub mob_type: i32,
    /// Mob name (resolved from ID)
    pub mob_name: String,
    /// Was attribution from a text trigger (delayed drop)?
    pub fabricated_attribution: bool,
    /// Items in the bag
    pub items: Vec<ProcessedLootItem>,
}

/// A processed loot item.
#[derive(Debug, Clone)]
pub struct ProcessedLootItem {
    /// Slot index (0-7)
    pub slot: i32,
    /// Item type ID
    pub item_id: i32,
    /// Item name (resolved from ID)
    pub item_name: String,
    /// Enchantment IDs
    pub enchant_ids: Vec<i32>,
}

/// A recently completed boss kill, injected from the combat tracker so loot
/// attribution can fall back to fight-correlation. Some bosses transform or
/// despawn on death and never register as a killed entity for proximity
/// attribution, so their bags are stored as Unknown; matching against a boss
/// the combat tracker confirmed died in the same instance recovers them.
#[derive(Debug, Clone)]
pub struct RecentBossKill {
    /// Map seed of the instance the boss died in.
    pub map_seed: i32,
    /// Boss object type.
    pub object_type: i32,
    /// Resolved boss name.
    pub name: String,
    /// When the fight started (epoch millis). Breaks ties when several bosses
    /// share an `ended_at_ms` (e.g. Moonlight Village dancers finalized together
    /// by one completion marker): the latest-started was the last fought.
    pub started_at_ms: i64,
    /// When the fight ended (epoch millis).
    pub ended_at_ms: i64,
}

/// Loot tracker coordinator.
///
/// Wires together:
/// - LootDetector (bag detection from Update packets)
/// - LootAttributionManager (mob attribution)
/// - PlayerStateTracker (player state snapshots)
/// - LootDatabase (persistence)
pub struct LootTracker {
    /// Loot bag detector
    detector: LootDetector,
    /// Drop attribution manager
    attribution: LootAttributionManager,
    /// Player state tracker
    player_state: PlayerStateTracker,
    /// Database (None if not opened)
    database: Option<LootDatabase>,
    /// Recent drops (for UI, kept in memory)
    recent_drops: Vec<ProcessedLootDrop>,
    /// Max recent drops to keep in memory
    max_recent: usize,
    /// Callback for new drops (for UI notification)
    on_drop_callback: Option<Box<dyn Fn(&ProcessedLootDrop) + Send + Sync>>,
    /// Counter for periodic stats logging
    tick_counter: u64,
    /// Total bags seen this session (for debugging)
    total_bags_seen: u64,
    /// Tracking settings (what gets captured to DB)
    tracking_settings: LootTrackingSettings,
    /// Recently killed bosses (from the combat tracker) for fight-correlation.
    recent_boss_kills: Vec<RecentBossKill>,
    /// Live-boss state for Tomb of the Ancients offscreen-death attribution.
    tomb: TombState,
    /// Attribution state for Ice Tomb soul drops.
    ice_tomb: IceTombState,
}

impl Default for LootTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl LootTracker {
    /// Create a new loot tracker.
    pub fn new() -> Self {
        Self {
            detector: LootDetector::new(),
            attribution: LootAttributionManager::new(),
            player_state: PlayerStateTracker::new(),
            database: None,
            recent_drops: Vec::new(),
            max_recent: 100,
            on_drop_callback: None,
            tick_counter: 0,
            total_bags_seen: 0,
            tracking_settings: LootTrackingSettings::default(),
            recent_boss_kills: Vec::new(),
            tomb: TombState::default(),
            ice_tomb: IceTombState::default(),
        }
    }

    /// Create a new loot tracker with a database opened at explicit paths.
    ///
    /// `loot_path` is the loot history database; `combat_path` is the paired
    /// combat history database consulted by the v13 Unknown-source backfill.
    pub fn with_database(
        loot_path: &std::path::Path,
        combat_path: Option<&std::path::Path>,
    ) -> Result<Self, rusqlite::Error> {
        let mut tracker = Self::new();
        tracker.database = Some(LootDatabase::open_writer(loot_path, combat_path)?);
        Ok(tracker)
    }

    /// Update the tracking settings.
    pub fn set_tracking_settings(&mut self, settings: &LootTrackingSettings) {
        self.tracking_settings = settings.clone();
        tracing::debug!(
            "[LOOT] Updated tracking settings: shiny={}, loot_enchants={}, unique_enchants={}, awakened_enchants={}, legendary+={}, ut={}, potions={}, marks={}, tiered={}(T{}+)",
            settings.track_shiny,
            settings.track_loot_enchants,
            settings.track_unique_enchants,
            settings.track_awakened_enchants,
            settings.track_legendary_plus,
            settings.track_ut_items,
            settings.track_potions,
            settings.track_marks,
            settings.track_tiered_items,
            settings.min_tiered_tier,
        );
    }

    /// Get the current tracking settings.
    pub fn tracking_settings(&self) -> &LootTrackingSettings {
        &self.tracking_settings
    }

    /// Provide the combat tracker's recently killed bosses for the current tick
    /// so Unknown bags can fall back to fight-correlation attribution.
    pub fn set_recent_boss_kills(&mut self, kills: Vec<RecentBossKill>) {
        self.recent_boss_kills = kills;
    }

    /// Set callback for new drops.
    pub fn set_on_drop_callback<F>(&mut self, callback: F)
    where
        F: Fn(&ProcessedLootDrop) + Send + Sync + 'static,
    {
        self.on_drop_callback = Some(Box::new(callback));
    }

    /// Get recent drops (most recent first).
    pub fn recent_drops(&self) -> &[ProcessedLootDrop] {
        &self.recent_drops
    }

    /// Get the current map name (empty if not in a map).
    pub fn current_map(&self) -> &str {
        self.detector.map_name()
    }

    /// Check if we're currently tracking (in a map with valid seed).
    pub fn is_tracking(&self) -> bool {
        !self.detector.map_name().is_empty()
    }

    /// Clear session drops (keeps database records).
    pub fn clear_session(&mut self) {
        self.recent_drops.clear();
    }

    /// Get the database (if opened).
    pub fn database(&self) -> Option<&LootDatabase> {
        self.database.as_ref()
    }

    /// Get mutable database reference.
    pub fn database_mut(&mut self) -> Option<&mut LootDatabase> {
        self.database.as_mut()
    }

    // ==================== Packet Event Handlers ====================

    /// Called when CreateSuccessPacket is received.
    pub fn on_create_success(&mut self, char_id: i32, object_id: i32) {
        self.player_state.on_create_success(char_id, object_id);
    }

    /// Called when MapInfoPacket is received.
    pub fn on_map_info(&mut self, map_name: &str, map_seed: i32) {
        self.detector.on_new_map(map_name, map_seed);
        self.attribution.clear();
        self.player_state.on_new_map(map_name, map_seed);
        self.tomb.reset(map_seed);
        self.ice_tomb.reset();
    }

    /// Called when disconnecting.
    pub fn on_disconnect(&mut self) {
        self.detector.clear();
        self.attribution.clear();
        self.player_state.on_disconnect();
        self.tomb.reset(0);
        self.ice_tomb.reset();
    }

    /// Called when TextPacket is received (for delayed drop triggers).
    pub fn on_text_packet(&mut self, name: &str, text: &str) {
        self.attribution
            .handle_text_packet(name, text, self.detector.map_seed());
        // Tomb main-boss rage-phase taunts pin death order for offscreen bags.
        if let Some(boss) = super::tomb_rage_taunt_boss(text) {
            let now = Utc::now().timestamp_millis().max(0) as u64;
            self.tomb.note_taunt(boss, now);
        }
        // Ice Tomb boss rage taunts refine ambiguous soul-bag distribution.
        if let Some(boss) = super::ice_tomb_rage_taunt_boss(text) {
            self.ice_tomb.note_taunt(boss);
        }
    }

    /// Called when player stats are updated (from Update/NewTick).
    pub fn on_player_stats(&mut self, object_id: i32, status: &ObjectStatusData, time_ms: u64) {
        self.player_state
            .update_player_stats(object_id, &status.stats, time_ms);
    }

    /// Called when player object type is known (from Update packet).
    pub fn on_player_object_type(&mut self, object_type: i32) {
        self.player_state.set_player_class(object_type);
    }

    /// Current tracked player's live fame (`StatType::CurrFame`), if a player is
    /// tracked. Used to keep the character cache's alive fame current.
    pub fn current_player_fame(&self) -> Option<i32> {
        self.player_state.current_player().map(|p| p.fame)
    }

    /// Process object spawns from Update packet.
    /// Call this for each new object in the Update packet.
    pub fn on_object_spawn(
        &mut self,
        object_id: i32,
        object_type: i32,
        status: &ObjectStatusData,
        time_ms: u64,
    ) {
        self.detector
            .on_object_spawn(object_id, object_type, status, time_ms);
        self.tomb
            .note_spawn(object_id, object_type, read_hp(status), time_ms);
    }

    /// Process object status update from NewTick/Update.
    pub fn on_object_update(&mut self, object_id: i32, status: &ObjectStatusData, time_ms: u64) {
        self.detector.on_object_update(object_id, status, time_ms);
        self.tomb.note_update(object_id, read_hp(status), time_ms);
    }

    /// Record that the player hit an entity.
    /// This is required for loot attribution - only hit entities can be droppers.
    pub fn on_entity_hit(&mut self, target_id: i32) {
        self.detector.on_entity_hit(target_id);
    }

    /// Record an entity being dropped (removed from map).
    /// Only adds to killed list if the entity was hit by the player.
    pub fn on_entity_dropped(&mut self, object_id: i32, time_ms: u64) {
        self.detector.on_entity_dropped(object_id, time_ms);
    }

    /// Remove an entity from tracking.
    pub fn on_entity_removed(&mut self, object_id: i32) {
        self.detector.on_entity_removed(object_id);
    }

    /// Called each game tick (from NewTick packet).
    /// This is where we process pending bags and write to database.
    ///
    /// Returns any new drops that were processed this tick.
    /// Drain bags still buffered by the detector before shutdown. It double-buffers
    /// spawns, so one tick can leave the freshest buffer unprocessed; tick twice.
    pub fn flush_pending(&mut self, time_ms: u64) {
        self.on_tick(time_ms);
        self.on_tick(time_ms);
    }

    /// Checkpoint and close the loot database. No-op in in-memory mode.
    pub fn close(&mut self) -> Result<(), rusqlite::Error> {
        match self.database.take() {
            Some(db) => db.close(),
            None => Ok(()),
        }
    }

    pub fn on_tick(&mut self, time_ms: u64) -> Vec<ProcessedLootDrop> {
        self.tick_counter += 1;
        let map_seed = self.detector.map_seed();

        // Begin attribution tick
        self.attribution.begin_loot_tick(map_seed);

        // Get bags from previous tick (double-buffer pattern)
        let pending_bags = self.detector.begin_tick();

        if !pending_bags.is_empty() {
            self.total_bags_seen += pending_bags.len() as u64;
            tracing::trace!(
                "[LOOT_TICK] Processing {} pending bags (total: {})",
                pending_bags.len(),
                self.total_bags_seen
            );
        }

        // Process each bag that should be tracked
        let mut new_drops = Vec::new();
        for bag in pending_bags {
            // Filter: only track bags that should be tracked based on settings
            // (high-tier bags always, shiny always, low-tier based on settings)
            if !bag.should_be_tracked_with_settings(&self.tracking_settings) {
                tracing::trace!(
                    "[LOOT_TICK] Skipping bag (not tracked): type={:?}",
                    bag.bag_type()
                );
                continue;
            }

            let bag_type_for_log = bag.bag_type();
            if let Some(drop) = self.process_bag(bag, map_seed, time_ms) {
                tracing::info!(
                    "[LOOT] {} dropped {} - {} items (player: {}, class_id: {})",
                    drop.mob_name,
                    drop.bag_type.name(),
                    drop.items.len(),
                    if drop.player.name.is_empty() {
                        "unknown"
                    } else {
                        &drop.player.name
                    },
                    drop.player.class_id,
                );
                new_drops.push(drop);
            } else {
                tracing::trace!(
                    "[LOOT_TICK] Failed to process bag: type={:?}",
                    bag_type_for_log
                );
            }
        }

        // Apply overrides for fabricated attributions
        self.attribution.apply_per_tick_overrides();

        // End tick housekeeping
        self.attribution.end_loot_tick();
        self.detector.end_tick();

        // Store and notify
        for drop in &new_drops {
            self.store_drop(drop);
            if let Some(callback) = &self.on_drop_callback {
                callback(drop);
            }
        }

        new_drops
    }

    /// Process a single pending bag into a fully resolved drop.
    fn process_bag(
        &mut self,
        bag: PendingBag,
        map_seed: i32,
        time_ms: u64,
    ) -> Option<ProcessedLootDrop> {
        // Get bag type
        let bag_type = bag.bag_type()?;

        // Find attribution (which mob dropped this)
        let attribution = self.attribution.find_attribution_for_bag(
            &bag.entity.pos,
            self.detector.killed_entities(),
            map_seed,
            time_ms,
        );

        // Get mob info
        let (mob_type, mob_name, fabricated) = if let Some(attr) = &attribution {
            let name =
                self.resolve_mob_name(attr.object_type, attr.loot_mob_id_override.as_deref());
            (attr.object_type, name, attr.fabricated_attribution)
        } else {
            (0, "Unknown".to_string(), false)
        };

        // Capture player snapshot
        let player = self.player_state.capture_snapshot()?;

        // Skip drops in Nexus (loot there is from other players, not mobs)
        if player.dungeon == "{s.nexus}" || player.dungeon.eq_ignore_ascii_case("nexus") {
            tracing::trace!("[LOOT] Skipping Nexus drop (not a mob drop)");
            return None;
        }

        // Process items
        let items = self.process_items(&bag);
        let now = Utc::now().timestamp_millis();

        // Resolve the final boss attribution, applying signal-based overrides
        // for bosses that proximity attribution misses (transform/despawn on
        // death). Brown bags are player/public bags and are never overridden.
        let item_ids: Vec<i32> = items.iter().map(|i| i.item_id).collect();
        let override_hit = if player.dungeon == super::TOMB_OF_THE_ANCIENTS_NAME {
            self.resolve_tomb_attribution(bag_type, mob_type, &item_ids, time_ms)
        } else if player.dungeon == super::ICE_TOMB_NAME {
            self.resolve_ice_tomb_attribution(bag_type, mob_type, &item_ids)
        } else {
            self.resolve_boss_override(
                bag_type,
                &player.dungeon,
                mob_type,
                &item_ids,
                map_seed,
                now,
            )
        };
        let (mob_type, mob_name) = match override_hit {
            Some((t, n)) => (t, n),
            None => (mob_type, mob_name),
        };

        Some(ProcessedLootDrop {
            timestamp: now,
            player,
            bag_type,
            mob_type,
            mob_name,
            fabricated_attribution: fabricated,
            items,
        })
    }

    /// Tomb of the Ancients curated attribution. The three main bosses (Bes,
    /// Geb, Nut) sit in separate rooms and frequently die offscreen, so their
    /// bags arrive Unknown; and Life bags can be proximity-attributed to nearby
    /// artifacts that never drop Life. This:
    ///   - forces Geb for a bag holding his invisible-spawner-emitted Mark;
    ///   - strips a Life bag credited to a non-Life-dropper and re-resolves it;
    ///   - recovers Unknown bags via unique drop signatures, then via the boss
    ///     that is no longer alive on-screen -- preferring death order pinned by
    ///     the bosses' unique rage-phase taunts, falling back to alphabetical.
    /// Returns Some((type, name)) to override, or None to keep the proximity
    /// attribution. Records the chosen main boss so a later offscreen bag in the
    /// same instance falls to a different one.
    fn resolve_tomb_attribution(
        &mut self,
        bag_type: LootBagType,
        prior_mob_type: i32,
        item_ids: &[i32],
        time_ms: u64,
    ) -> Option<(i32, String)> {
        // Brown/public bags are never boss drops.
        if matches!(bag_type, LootBagType::Brown | LootBagType::BoostedBrown) {
            return None;
        }

        // (5) Mark of Geb drops only from Geb, via an invisible spawner, so a bag
        // holding it is always Geb -- even over a proximity guess. The Mark is a
        // separate spawner bag, not one of Geb's own loot bags, so it does not
        // consume Geb's slot in the alphabetical fallback: a later Life bag can
        // still be handed to Geb. The Mark also only spawns once all three main
        // bosses are defeated, so it confirms every main boss dead -- overriding
        // the on-screen alive window so an offscreen boss's real bag (which may
        // have dropped within that window) becomes an attribution candidate.
        if item_ids.contains(&super::MARK_OF_GEB_ITEM_ID) {
            for (t, _) in super::TOMB_MAIN_BOSSES {
                self.tomb.note_confirmed_dead(t);
            }
            return Some(self.tomb_boss_name(super::TOMB_GEB_OBJECT_TYPE, "Geb"));
        }

        // Certain dungeon modifiers spawn extra bags (pet food, eggs, skin
        // tokens, effusions, mystery-stat crates) from an invisible entity, just
        // like the Mark. A bag holding only such non-boss loot is credited to Geb
        // (the conventional last main boss) without consuming a boss's one-bag
        // slot, so it cannot steal the slot of the boss's real (offscreen) bag.
        if super::is_tomb_spawner_only_bag(item_ids) {
            return Some(self.tomb_boss_name(super::TOMB_GEB_OBJECT_TYPE, "Geb"));
        }

        // (1) Life never drops from artifacts. A Life bag credited by proximity
        // to something that cannot drop Life was mis-attributed; treat it as
        // Unknown and re-resolve below.
        let has_life = item_ids.contains(&super::POTION_OF_LIFE_ITEM_ID);
        let life_misattributed =
            has_life && prior_mob_type != 0 && !super::is_tomb_life_dropper(prior_mob_type);
        // A main boss holds at most one real bag per run. If proximity credited
        // this bag to a main boss that already holds one, the real dropper is a
        // different (offscreen) boss -- the Mark of Geb proves all three died --
        // so re-resolve instead of letting one boss keep two bags.
        let duplicate_boss = super::is_tomb_main_boss(prior_mob_type)
            && self.tomb.attributed.contains(&prior_mob_type);
        let needs_recovery = prior_mob_type == 0 || life_misattributed || duplicate_boss;

        if !needs_recovery {
            // Keep the good attribution, but remember it so the alphabetical
            // fallback hands later offscreen bags to a different boss.
            if super::is_tomb_main_boss(prior_mob_type) {
                self.tomb.attributed.insert(prior_mob_type);
            }
            return None;
        }

        // (3) A unique drop signature names the boss outright.
        if let Some((t, n)) = super::tomb_unique_boss(item_ids) {
            return Some(self.credit_tomb_boss(t, &n));
        }

        // (2 & 4) Otherwise assign to a boss no longer alive on-screen. Prefer
        // the earliest-raged dead boss so distinct offscreen bags map to distinct
        // bosses in death order (their unique rage taunts pin that order). Fall
        // back to the alphabetically-first dead boss when no raged boss qualifies
        // (rage phase skipped, which is rare).
        if let Some((t, n)) = self.tomb.raged_candidate(time_ms) {
            return Some(self.credit_tomb_boss(t, n));
        }
        if let Some((t, n)) = self.tomb.dead_candidates(time_ms).first().copied() {
            return Some(self.credit_tomb_boss(t, n));
        }

        // Nothing recovered it. A duplicated main-boss bag keeps its (already
        // valid, if over-counted) boss rather than becoming Unknown -- better a
        // slightly wrong boss than no boss. A mis-attributed Life bag must not
        // keep its wrong (artifact) credit; an already-Unknown bag stays Unknown.
        if duplicate_boss {
            return None;
        }
        if life_misattributed {
            return Some((0, "Unknown".to_string()));
        }
        None
    }

    /// Record a Tomb main boss as credited (for the alphabetical fallback) and
    /// resolve its display name, falling back to the provided static name.
    /// Record a Tomb main boss as credited (for the alphabetical fallback) and
    /// resolve its display name. Use this for a bag that is one of the boss's
    /// real loot bags, so a later offscreen bag is handed to a different boss.
    fn credit_tomb_boss(&mut self, object_type: i32, fallback_name: &str) -> (i32, String) {
        if super::is_tomb_main_boss(object_type) {
            self.tomb.attributed.insert(object_type);
        }
        self.tomb_boss_name(object_type, fallback_name)
    }

    /// Resolve a Tomb boss's display name without crediting it. Used for the
    /// Mark of Geb bag, which is emitted by a separate invisible spawner rather
    /// than being one of Geb's own loot bags, so it must not consume Geb's slot
    /// in the alphabetical distinct-bag fallback.
    fn tomb_boss_name(&self, object_type: i32, fallback_name: &str) -> (i32, String) {
        let name = get_asset_manager()
            .object_name(object_type)
            .unwrap_or_else(|| fallback_name.to_string());
        (object_type, name)
    }

    /// Ice Tomb curated attribution. Each boss's soul is whitelisted as a loot
    /// dropper (see [`is_forced_loot_dropper`](super::is_forced_loot_dropper)),
    /// so proximity usually tags a bag with the soul that dropped it -- which we
    /// remap here to the boss (Frimar/Polaris/Glacius). The souls all die in one
    /// small room, so two can die overlapping and leave a bag ambiguous; and a
    /// soul the player never saw leaves its bag Unknown. Seeing any soul bag
    /// proves all three bosses are dead, so ambiguous bags are recovered by:
    ///   - a unique per-boss ring (the only precise drop signature), else
    ///   - handing the bag to a boss without one yet, in rage-taunt (death)
    ///     order, falling back to alphabetical.
    /// Returns Some((type, name)) to override, or None to keep the proximity
    /// attribution.
    fn resolve_ice_tomb_attribution(
        &mut self,
        bag_type: LootBagType,
        prior_mob_type: i32,
        item_ids: &[i32],
    ) -> Option<(i32, String)> {
        // Brown/public bags are never boss drops.
        if matches!(bag_type, LootBagType::Brown | LootBagType::BoostedBrown) {
            return None;
        }

        // (1) A unique per-boss ring is ground truth and overrides proximity:
        // two souls dying overlapping can tag a bag with the wrong soul, so the
        // ring -- when present -- wins outright.
        if let Some(boss) = super::ice_tomb_unique_boss(item_ids) {
            self.ice_tomb.credit(boss);
            return Some(self.ice_tomb_boss_output(boss));
        }

        // A bag proximity-tagged with a soul is that soul's boss. If the boss
        // already holds a bag (two souls died overlapping), the real dropper is
        // a different boss, so re-resolve below.
        let prior_boss = super::ice_tomb_boss_name_for_soul(prior_mob_type);
        let duplicate = prior_boss.is_some_and(|b| self.ice_tomb.attributed.contains(b));

        // Trust a soul tag when that boss isn't already accounted for.
        if let Some(boss) = prior_boss {
            if !duplicate {
                self.ice_tomb.credit(boss);
                return Some(self.ice_tomb_boss_output(boss));
            }
        }

        // Whether this bag holds any Ice Tomb boss loot at all, per the drop
        // tables. A non-soul proximity match (a minion/artifact) that holds boss
        // loot was mis-attributed and must be recovered.
        let has_boss_loot = super::ice_tomb_has_boss_loot(item_ids);
        let misattributed = prior_boss.is_none() && prior_mob_type != 0 && has_boss_loot;
        let needs_recovery = prior_mob_type == 0 || duplicate || misattributed;

        if !needs_recovery {
            // A non-boss bag near a hit minion keeps its proximity attribution.
            return None;
        }

        // Only distribute bags that actually hold boss loot; a stray Unknown bag
        // with no boss loot stays Unknown.
        if !has_boss_loot {
            if duplicate {
                // Keep the (over-counted) soul's boss rather than losing it.
                if let Some(boss) = prior_boss {
                    return Some(self.ice_tomb_boss_output(boss));
                }
            }
            return None;
        }

        // (2) Hand the bag to a boss that has no bag yet, in death order.
        if let Some(boss) = self.ice_tomb.next_uncredited() {
            self.ice_tomb.credit(boss);
            return Some(self.ice_tomb_boss_output(boss));
        }

        // Every boss already holds a bag. Keep a duplicated soul's boss (over-
        // counted but valid) rather than dropping it to Unknown.
        if let Some(boss) = prior_boss {
            return Some(self.ice_tomb_boss_output(boss));
        }
        None
    }

    /// Resolve an Ice Tomb boss's stored object id and display name. The real
    /// boss object id is stored as `mob_type` so the loot history draws the boss
    /// sprite (the soul id is only used for live proximity detection).
    fn ice_tomb_boss_output(&self, boss: &str) -> (i32, String) {
        (super::ice_tomb_object_type_for_boss(boss), boss.to_string())
    }

    /// Resolve a stronger boss attribution than proximity provided, or `None`
    /// to keep the current one. Skips brown bags entirely (bosses never drop
    /// loot in brown bags, so those are player/public bags).
    fn resolve_boss_override(
        &self,
        bag_type: LootBagType,
        dungeon: &str,
        prior_mob_type: i32,
        item_ids: &[i32],
        map_seed: i32,
        now_ms: i64,
    ) -> Option<(i32, String)> {
        Self::resolve_boss_override_with(
            &self.recent_boss_kills,
            bag_type,
            dungeon,
            prior_mob_type,
            item_ids,
            map_seed,
            now_ms,
        )
    }

    /// Fight-correlation core of [`resolve_boss_override`], taking the recent
    /// boss kills explicitly so the legacy loot backfill can reuse the exact
    /// same attribution rules with kills sourced from recorded Combat History.
    pub(crate) fn resolve_boss_override_with(
        recent_boss_kills: &[RecentBossKill],
        bag_type: LootBagType,
        dungeon: &str,
        prior_mob_type: i32,
        item_ids: &[i32],
        map_seed: i32,
        now_ms: i64,
    ) -> Option<(i32, String)> {
        if matches!(bag_type, LootBagType::Brown | LootBagType::BoostedBrown) {
            return None;
        }

        // Mark of Janus is a guaranteed, exclusive Janus drop, so a bag holding
        // it is always a genuine Janus drop even if proximity misattributed it.
        if item_ids.contains(&super::MARK_OF_JANUS_ITEM_ID) {
            return Some((super::JANUS_OBJECT_TYPE, super::JANUS_NAME.to_string()));
        }

        // Mark of the Barkeep is a guaranteed, exclusive Bradley the Barkeep
        // drop. He never drops loot directly (bags arrive Unknown), so a bag
        // holding the Mark is always his.
        if item_ids.contains(&super::MARK_OF_THE_BARKEEP_ITEM_ID) {
            return Some((
                super::TAVERN_BARKEEP_OBJECT_TYPE,
                super::TAVERN_BARKEEP_NAME.to_string(),
            ));
        }

        // Any boss completion Mark is a guaranteed, exclusive drop of its
        // dungeon's main boss, so a bag holding one is definitively that boss's
        // drop even if proximity left it Unknown or pinned it to a nearby
        // minion. The Mark itself proves the main boss died this instance;
        // attribute the bag to the boss the combat tracker recorded killed here
        // (variant-safe: e.g. Archdemon Malphas vs Malphas, Gilded Forgemaster).
        // Curated dungeons run their own Mark logic (Geb, Soulwarden) below, so
        // skip them here. Legacy bags with no recorded kill are handled by the
        // per-dungeon consensus migration instead.
        if !super::has_curated_boss_attribution(dungeon) && super::bag_has_boss_mark(item_ids) {
            if let Some(hit) = Self::select_last_boss_in_instance(recent_boss_kills, map_seed) {
                return Some(hit);
            }
        }

        // Killer Bee Nest realm event: the invisible EH Event Taunt Controller
        // emits every event bag, so proximity tags them all to it (or leaves
        // them Unknown). The three Beehemoths die one-by-one; the last, grown
        // Beehemoth drops a soulbound bag whose single loot colour identifies it.
        // A multi-colour bag comes from Corrupted Bramblethorn / The Keyper,
        // which drop the same items across colours. Runs before the
        // `prior_mob_type != 0` early-out because event bags arrive tagged with
        // the taunt controller (a valid proximity source), not Unknown.
        if super::is_realm_dungeon(dungeon)
            && (prior_mob_type == 0 || super::is_killer_bee_nest_event_source(prior_mob_type))
        {
            if let Some(hit) = Self::resolve_killer_bee_nest(
                recent_boss_kills,
                prior_mob_type,
                item_ids,
                map_seed,
                now_ms,
            ) {
                return Some(hit);
            }
        }

        // The remaining signals only recover bags proximity left as Unknown.
        if prior_mob_type != 0 {
            return None;
        }

        // Lair of Draconis elemental dragons transform on death and emit loot
        // anonymously; identify the dragon from its item signature.
        if dungeon == super::LAIR_OF_DRACONIS_NAME {
            if let Some((t, n)) = super::lair_of_draconis_dragon(item_ids) {
                return Some((t, n.to_string()));
            }
        }

        // Moonlight Village bosses go invulnerable and never die; their loot is
        // emitted at dungeon end by invisible dropper entities the player never
        // hits, so every bag arrives Unknown. Attribute it to the last boss the
        // player fought in this instance (the Challenge Gate's second bag is
        // attributed the same way). Item signatures can't help here: the dropper
        // pools the whole dungeon's loot, so we key off the fight, not the items.
        if dungeon == super::MOONLIGHT_VILLAGE_NAME {
            return Self::select_last_boss_in_instance(recent_boss_kills, map_seed);
        }

        // Spectral Penitentiary minibosses transform/despawn on death and emit
        // loot anonymously. Prefer a confident item identity (unique white, or a
        // stat potion confirmed by mana pairing or boss-only loot).
        if dungeon == super::SPECTRAL_PENITENTIARY_NAME {
            if let Some((t, n)) = super::spectral_penitentiary_boss(item_ids) {
                return Some((t, n.to_string()));
            }
        }

        // General, drop-table-gated fallback. Only bags holding an item a boss
        // is known to drop in this dungeon are eligible (never a minion or public
        // bag). A unique drop attributes directly; an ambiguous one is resolved
        // to a boss the combat tracker confirmed died in this same instance.
        // Skipped for dungeons with curated logic (handled above / below).
        if !super::has_curated_boss_attribution(dungeon) {
            if let Some(hit) = Self::attribute_by_boss_drop_with(
                recent_boss_kills,
                dungeon,
                item_ids,
                map_seed,
                now_ms,
            ) {
                return Some(hit);
            }
        }

        // Open-realm event loot (e.g. the Ethereal Shrine) is emitted by an
        // invisible dropper the player never hits, so it arrives Unknown. When
        // the bag's rare loot points to a single non-seasonal realm event, that
        // event is its source; ambiguous loot (multiple possible events) stays
        // Unknown.
        if super::is_realm_dungeon(dungeon) {
            if let Some(hit) = super::resolve_realm_event_source(item_ids) {
                return Some(hit);
            }
        }

        // drop (boss-only loot) but with no identity and no matching kill is
        // almost always Oculon, the miniboss most prone to going unrecorded.
        if dungeon == super::SPECTRAL_PENITENTIARY_NAME {
            if let Some((t, n)) = super::spectral_penitentiary_boss_confirmed_default(item_ids) {
                return Some((t, n.to_string()));
            }
        }

        // Chief Beisa's oversized Oryx's Sanctuary room means he routinely dies
        // offscreen, so his high-tier bags arrive Unknown even though he was
        // fought. The other O3 minibosses die onscreen and are attributed at
        // death, so an Unknown O3 bag fitting Beisa's profile (Greater
        // Life/Mana, red bag + T13, or a Beisa drop-table item) is his -- gated
        // on his fight being recorded in this instance. Bags that don't fit stay
        // Unknown rather than guess a wrong boss.
        if dungeon == super::ORYX_SANCTUARY_NAME
            && super::beisa_engaged(recent_boss_kills, map_seed)
            && super::bag_matches_beisa_profile(bag_type, item_ids)
        {
            return Some((
                super::CHIEF_BEISA_OBJECT_TYPE,
                super::CHIEF_BEISA_NAME.to_string(),
            ));
        }

        None
    }

    /// Resolve the source of a Killer Bee Nest event bag (dungeon "Realm", prior
    /// attribution the taunt controller / event hive / Unknown). The event drops
    /// soulbound loot in a single colour matching the last Beehemoth the player
    /// damaged, so:
    ///   - exactly one colour of loot -> that Beehemoth;
    ///   - multiple colours -> Corrupted Bramblethorn / The Keyper (both drop the
    ///     same items across colours), preferring whichever was last fought in
    ///     this instance, defaulting to Corrupted Bramblethorn;
    ///   - no colour, but the bag is positively an event bag (taunt controller /
    ///     hive) -> the last Beehemoth killed in this instance, defaulting to Blue
    ///     (alphabetically first);
    ///   - no colour and merely Unknown -> `None` (not necessarily a Beehemoth
    ///     bag; leave it to other signals).
    /// Passing an empty `recent_boss_kills` yields the legacy-migration defaults
    /// (Corrupted Bramblethorn for mixed, Blue for colourless event bags).
    pub(crate) fn resolve_killer_bee_nest(
        recent_boss_kills: &[RecentBossKill],
        prior_mob_type: i32,
        item_ids: &[i32],
        map_seed: i32,
        now_ms: i64,
    ) -> Option<(i32, String)> {
        let colors = super::beehemoth_colors(item_ids);
        match colors.len() {
            1 => {
                let c = colors[0];
                Some((c.object_type(), c.name().to_string()))
            }
            n if n >= 2 => Some(Self::select_bramble_or_keyper(
                recent_boss_kills,
                map_seed,
                now_ms,
            )),
            _ => {
                if super::is_killer_bee_nest_event_source(prior_mob_type) {
                    return Some(
                        Self::select_recent_kill_of(
                            recent_boss_kills,
                            &[
                                super::YELLOW_BEEHEMOTH_OBJECT_TYPE,
                                super::RED_BEEHEMOTH_OBJECT_TYPE,
                                super::BLUE_BEEHEMOTH_OBJECT_TYPE,
                            ],
                            map_seed,
                            now_ms,
                        )
                        .unwrap_or((
                            super::BLUE_BEEHEMOTH_OBJECT_TYPE,
                            super::BeehemothColor::Blue.name().to_string(),
                        )),
                    );
                }
                // A colourless bag that arrived Unknown is still routinely a
                // Beehemoth event drop: the invisible EH dropper is never hit, so
                // proximity leaves the bag untagged rather than tagging the taunt
                // controller. When a Beehemoth was killed in this instance close
                // to the bag, attribute it to that Beehemoth (per-colour, matching
                // the death closest to the bag). No nearby Beehemoth kill -> leave
                // it Unknown so unrelated Realm bags are never stolen.
                if prior_mob_type == 0 {
                    return Self::select_closest_beehemoth_kill(
                        recent_boss_kills,
                        map_seed,
                        now_ms,
                    );
                }
                None
            }
        }
    }

    /// The Beehemoth (Yellow / Red / Blue) killed in this instance whose death is
    /// closest to `bag_time_ms`, within a tight correlation window -- so each of
    /// the three per-Beehemoth event bags (which drop ~1s after their bee dies)
    /// maps to its own colour. Returns `None` when no Beehemoth kill correlates
    /// (or `map_seed` is 0), leaving the bag Unknown.
    fn select_closest_beehemoth_kill(
        recent_boss_kills: &[RecentBossKill],
        map_seed: i32,
        bag_time_ms: i64,
    ) -> Option<(i32, String)> {
        if map_seed == 0 {
            return None;
        }
        const BEEHEMOTHS: [i32; 3] = [
            super::YELLOW_BEEHEMOTH_OBJECT_TYPE,
            super::RED_BEEHEMOTH_OBJECT_TYPE,
            super::BLUE_BEEHEMOTH_OBJECT_TYPE,
        ];
        recent_boss_kills
            .iter()
            .filter(|k| k.map_seed == map_seed && BEEHEMOTHS.contains(&k.object_type))
            .filter(|k| {
                let lag = bag_time_ms - k.ended_at_ms;
                (-BEEHEMOTH_UNKNOWN_EARLY_TOLERANCE_MS..=BEEHEMOTH_UNKNOWN_MAX_LAG_MS)
                    .contains(&lag)
            })
            .min_by_key(|k| (bag_time_ms - k.ended_at_ms).abs())
            .map(|k| (k.object_type, k.name.clone()))
    }

    /// Pick Corrupted Bramblethorn or The Keyper for a mixed-colour Killer Bee
    /// Nest bag: the one last fought in this instance (within the kill window),
    /// defaulting to Corrupted Bramblethorn when neither was recorded.
    fn select_bramble_or_keyper(
        recent_boss_kills: &[RecentBossKill],
        map_seed: i32,
        now_ms: i64,
    ) -> (i32, String) {
        Self::select_recent_kill_of(
            recent_boss_kills,
            &[
                super::CORRUPTED_BRAMBLETHORN_OBJECT_TYPE,
                super::KEYPER_OBJECT_TYPE,
            ],
            map_seed,
            now_ms,
        )
        .unwrap_or((
            super::CORRUPTED_BRAMBLETHORN_OBJECT_TYPE,
            super::CORRUPTED_BRAMBLETHORN_NAME.to_string(),
        ))
    }

    /// The most recently killed boss among `object_types` in the same instance
    /// (`map_seed`) within the kill-correlation window around `now_ms`, or `None`
    /// (including when `map_seed` is 0 or no matching kill exists).
    fn select_recent_kill_of(
        recent_boss_kills: &[RecentBossKill],
        object_types: &[i32],
        map_seed: i32,
        now_ms: i64,
    ) -> Option<(i32, String)> {
        if map_seed == 0 {
            return None;
        }
        recent_boss_kills
            .iter()
            .filter(|k| k.map_seed == map_seed)
            .filter(|k| object_types.contains(&k.object_type))
            .filter(|k| {
                let lag = now_ms - k.ended_at_ms;
                lag >= -KILL_CORRELATION_EARLY_TOLERANCE_MS && lag <= KILL_CORRELATION_MAX_LAG_MS
            })
            .max_by_key(|k| (k.ended_at_ms, k.started_at_ms))
            .map(|k| (k.object_type, k.name.clone()))
    }

    /// Attribute an Unknown bag from the dungeon's boss drop tables, gated on the
    /// bag actually holding boss-only loot. A single possible boss is taken
    /// directly; multiple possible bosses are disambiguated by a confirmed kill
    /// in the same instance (so a boss the player merely shot but that cannot
    /// drop these items is never chosen).
    fn attribute_by_boss_drop_with(
        recent_boss_kills: &[RecentBossKill],
        dungeon: &str,
        item_ids: &[i32],
        map_seed: i32,
        bag_time_ms: i64,
    ) -> Option<(i32, String)> {
        let candidates = super::boss_drop_candidates(dungeon, item_ids);
        Self::select_boss_from_candidates(&candidates, recent_boss_kills, map_seed, bag_time_ms)
    }

    /// Pick the boss for an Unknown bag from its drop-table candidates: none when
    /// the bag is not boss-only loot, the sole candidate when unambiguous, else
    /// the most recently killed candidate boss within the correlation window.
    fn select_boss_from_candidates(
        candidates: &[(i32, String)],
        kills: &[RecentBossKill],
        map_seed: i32,
        bag_time_ms: i64,
    ) -> Option<(i32, String)> {
        match candidates.len() {
            0 => None,
            1 => Some(candidates[0].clone()),
            _ => {
                if map_seed == 0 {
                    return None;
                }
                kills
                    .iter()
                    .filter(|k| k.object_type > 0 && k.map_seed == map_seed)
                    .filter(|k| candidates.iter().any(|(t, _)| *t == k.object_type))
                    .filter(|k| {
                        let lag = bag_time_ms - k.ended_at_ms;
                        lag >= -KILL_CORRELATION_EARLY_TOLERANCE_MS
                            && lag <= KILL_CORRELATION_MAX_LAG_MS
                    })
                    .max_by_key(|k| k.ended_at_ms)
                    .map(|k| (k.object_type, k.name.clone()))
            }
        }
    }

    /// Pick the last boss the player fought in a given instance: the most recent
    /// killed boss (`ended_at_ms`) among `kills` sharing `map_seed`. Used for
    /// Moonlight Village, whose end-of-dungeon loot has no per-bag identity.
    /// Returns None when the instance has no recorded boss kill.
    fn select_last_boss_in_instance(
        kills: &[RecentBossKill],
        map_seed: i32,
    ) -> Option<(i32, String)> {
        if map_seed == 0 {
            return None;
        }
        kills
            .iter()
            .filter(|k| k.object_type > 0 && k.map_seed == map_seed)
            .max_by_key(|k| (k.ended_at_ms, k.started_at_ms))
            .map(|k| (k.object_type, k.name.clone()))
    }

    /// Process bag items into resolved item list.
    fn process_items(&self, bag: &PendingBag) -> Vec<ProcessedLootItem> {
        let mut items = Vec::new();

        for (slot, &item_id) in bag.items.iter().enumerate() {
            if item_id > 0 {
                let item_name = self.resolve_item_name(item_id);
                let enchant_ids = self.parse_enchants(bag.enchants[slot].as_deref());

                items.push(ProcessedLootItem {
                    slot: slot as i32,
                    item_id,
                    item_name,
                    enchant_ids,
                });
            }
        }

        items
    }

    /// Resolve mob name from ID.
    fn resolve_mob_name(&self, mob_type: i32, override_id: Option<&str>) -> String {
        let mgr = get_asset_manager();

        // If there's an override (like "29003HM"), use that for display
        if let Some(ov) = override_id {
            // Try to get base name and append suffix
            if let Some(base_name) = mgr.object_name(mob_type) {
                if ov.ends_with("HM") {
                    return format!("{} (HM)", base_name);
                } else if ov.ends_with("TR") {
                    return format!("{} (True)", base_name);
                }
            }
        }

        mgr.object_name(mob_type)
            .unwrap_or_else(|| format!("Unknown ({})", mob_type))
    }

    /// Resolve item name from ID.
    fn resolve_item_name(&self, item_id: i32) -> String {
        get_asset_manager()
            .object_name(item_id)
            .unwrap_or_else(|| format!("Unknown ({})", item_id))
    }

    /// Parse enchant IDs from base64 data.
    fn parse_enchants(&self, enchant_data: Option<&str>) -> Vec<i32> {
        let Some(data) = enchant_data else {
            return Vec::new();
        };

        // Parse base64 enchant data using the existing parser
        parse_enchant_ids(data)
            .into_iter()
            .map(|id| id as i32)
            .collect()
    }

    /// Store a drop to recent list and database.
    fn store_drop(&mut self, drop: &ProcessedLootDrop) {
        // Add to recent drops
        self.recent_drops.insert(0, drop.clone());
        if self.recent_drops.len() > self.max_recent {
            self.recent_drops.pop();
        }

        // Write to database
        if let Some(db) = &mut self.database {
            let new_drop = NewLootDrop {
                timestamp: drop.timestamp,
                player: drop.player.clone(),
                bag_type: drop.bag_type,
                mob_type: drop.mob_type,
                mob_name: drop.mob_name.clone(),
                items: drop
                    .items
                    .iter()
                    .map(|i| NewLootItem {
                        slot: i.slot,
                        item_id: i.item_id,
                        item_name: i.item_name.clone(),
                        enchant_ids: i.enchant_ids.clone(),
                    })
                    .collect(),
            };

            if let Err(e) = db.insert_loot_drop(&new_drop) {
                tracing::error!("Failed to write loot drop to database: {}", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_tracker() {
        let tracker = LootTracker::new();
        assert!(tracker.recent_drops().is_empty());
        assert!(tracker.database().is_none());
    }

    fn kill(map_seed: i32, object_type: i32, name: &str, ended_at_ms: i64) -> RecentBossKill {
        RecentBossKill {
            map_seed,
            object_type,
            name: name.to_string(),
            started_at_ms: 0,
            ended_at_ms,
        }
    }

    fn kill_span(
        map_seed: i32,
        object_type: i32,
        name: &str,
        started_at_ms: i64,
        ended_at_ms: i64,
    ) -> RecentBossKill {
        RecentBossKill {
            map_seed,
            object_type,
            name: name.to_string(),
            started_at_ms,
            ended_at_ms,
        }
    }

    #[test]
    fn select_boss_none_when_not_boss_loot() {
        // Empty candidate set means the bag holds no known boss drop.
        let picked = LootTracker::select_boss_from_candidates(&[], &[], 42, 1000);
        assert!(picked.is_none());
    }

    #[test]
    fn select_boss_unique_drop_needs_no_kill() {
        let cands = vec![(100, "Boss A".to_string())];
        let picked = LootTracker::select_boss_from_candidates(&cands, &[], 42, 1000);
        assert_eq!(picked, Some((100, "Boss A".to_string())));
    }

    #[test]
    fn select_boss_ambiguous_uses_matching_kill() {
        let cands = vec![(100, "Boss A".to_string()), (200, "Boss B".to_string())];
        // Only Boss B was killed in this instance, so the bag is Boss B's.
        let kills = vec![kill(42, 200, "Boss B", 1000)];
        let picked = LootTracker::select_boss_from_candidates(&cands, &kills, 42, 1000);
        assert_eq!(picked, Some((200, "Boss B".to_string())));
    }

    // --- Killer Bee Nest realm event ---

    const KBN_BLUE_QUIVER: i32 = 4338;
    const KBN_BLUE_ARMOR: i32 = 4310;
    const KBN_RED_QUIVER: i32 = 4339;
    const KBN_YELLOW_SHINY: i32 = 5343;

    #[test]
    fn kbn_single_color_maps_to_that_beehemoth() {
        // A soulbound bag holding one Blue item is the Blue Beehemoth's, no
        // recorded kills needed.
        let picked = LootTracker::resolve_killer_bee_nest(
            &[],
            super::super::EH_EVENT_TAUNT_CONTROLLER_OBJECT_TYPE,
            &[KBN_BLUE_QUIVER, KBN_BLUE_ARMOR],
            42,
            1000,
        );
        assert_eq!(
            picked,
            Some((
                super::super::BLUE_BEEHEMOTH_OBJECT_TYPE,
                "Blue Beehemoth".to_string()
            ))
        );
    }

    #[test]
    fn kbn_single_color_reattributes_unknown_bag() {
        // Even a bag left Unknown (prior 0) is reassigned when it holds a
        // single-colour Beehemoth item.
        let picked = LootTracker::resolve_killer_bee_nest(&[], 0, &[KBN_RED_QUIVER], 42, 1000);
        assert_eq!(
            picked,
            Some((
                super::super::RED_BEEHEMOTH_OBJECT_TYPE,
                "Red Beehemoth".to_string()
            ))
        );
    }

    #[test]
    fn kbn_mixed_colors_default_bramblethorn() {
        // Two colours in one bag -> Bramblethorn/Keyper; with no kills the legacy
        // default is Corrupted Bramblethorn.
        let picked = LootTracker::resolve_killer_bee_nest(
            &[],
            super::super::EH_EVENT_TAUNT_CONTROLLER_OBJECT_TYPE,
            &[KBN_BLUE_QUIVER, KBN_RED_QUIVER],
            42,
            1000,
        );
        assert_eq!(
            picked,
            Some((
                super::super::CORRUPTED_BRAMBLETHORN_OBJECT_TYPE,
                super::super::CORRUPTED_BRAMBLETHORN_NAME.to_string()
            ))
        );
    }

    #[test]
    fn kbn_mixed_colors_prefer_last_fought_keyper() {
        // Live: mixed-colour bag attributed to the boss last fought in-instance.
        let kills = vec![
            kill(
                42,
                super::super::CORRUPTED_BRAMBLETHORN_OBJECT_TYPE,
                "Corrupted Bramblethorn",
                1000,
            ),
            kill(42, super::super::KEYPER_OBJECT_TYPE, "The Keyper", 5000),
        ];
        let picked = LootTracker::resolve_killer_bee_nest(
            &kills,
            0,
            &[KBN_BLUE_QUIVER, KBN_YELLOW_SHINY],
            42,
            6000,
        );
        assert_eq!(
            picked,
            Some((super::super::KEYPER_OBJECT_TYPE, "The Keyper".to_string()))
        );
    }

    #[test]
    fn kbn_no_color_event_bag_defaults_blue() {
        // A colourless bag positively tagged as the event (taunt controller) with
        // no recorded kills falls to Blue (alphabetically first).
        let picked = LootTracker::resolve_killer_bee_nest(
            &[],
            super::super::EH_EVENT_TAUNT_CONTROLLER_OBJECT_TYPE,
            &[9999],
            42,
            1000,
        );
        assert_eq!(
            picked,
            Some((
                super::super::BLUE_BEEHEMOTH_OBJECT_TYPE,
                "Blue Beehemoth".to_string()
            ))
        );
    }

    #[test]
    fn kbn_no_color_event_bag_prefers_last_killed_beehemoth() {
        // Live: a colourless event bag is attributed to the last Beehemoth killed.
        let kills = vec![
            kill(
                42,
                super::super::YELLOW_BEEHEMOTH_OBJECT_TYPE,
                "Yellow Beehemoth",
                2000,
            ),
            kill(
                42,
                super::super::RED_BEEHEMOTH_OBJECT_TYPE,
                "Red Beehemoth",
                6000,
            ),
        ];
        let picked = LootTracker::resolve_killer_bee_nest(
            &kills,
            super::super::EH_EVENT_HIVE_OBJECT_TYPE,
            &[9999],
            42,
            7000,
        );
        assert_eq!(
            picked,
            Some((
                super::super::RED_BEEHEMOTH_OBJECT_TYPE,
                "Red Beehemoth".to_string()
            ))
        );
    }

    #[test]
    fn kbn_no_color_unknown_bag_left_alone() {
        // A colourless Unknown bag is not necessarily a Beehemoth bag; leave it.
        let picked = LootTracker::resolve_killer_bee_nest(&[], 0, &[9999], 42, 1000);
        assert!(picked.is_none());
    }

    #[test]
    fn kbn_unknown_colourless_bags_map_to_closest_beehemoth() {
        // Live regression: the three Beehemoths die one-by-one and each drops a
        // colourless bag ~1s later that arrives Unknown (proximity can't tag the
        // invisible dropper). Each bag must attribute to its own bee by matching
        // the death closest to the bag, not stay "?".
        let kills = vec![
            kill(
                42,
                super::super::RED_BEEHEMOTH_OBJECT_TYPE,
                "Red Beehemoth",
                11_000,
            ),
            kill(
                42,
                super::super::YELLOW_BEEHEMOTH_OBJECT_TYPE,
                "Yellow Beehemoth",
                26_000,
            ),
            kill(
                42,
                super::super::BLUE_BEEHEMOTH_OBJECT_TYPE,
                "Blue Beehemoth",
                48_000,
            ),
        ];
        let red = LootTracker::resolve_killer_bee_nest(&kills, 0, &[9999], 42, 12_000);
        assert_eq!(
            red,
            Some((
                super::super::RED_BEEHEMOTH_OBJECT_TYPE,
                "Red Beehemoth".to_string()
            ))
        );
        let yellow = LootTracker::resolve_killer_bee_nest(&kills, 0, &[9999], 42, 27_000);
        assert_eq!(
            yellow,
            Some((
                super::super::YELLOW_BEEHEMOTH_OBJECT_TYPE,
                "Yellow Beehemoth".to_string()
            ))
        );
        let blue = LootTracker::resolve_killer_bee_nest(&kills, 0, &[9999], 42, 49_000);
        assert_eq!(
            blue,
            Some((
                super::super::BLUE_BEEHEMOTH_OBJECT_TYPE,
                "Blue Beehemoth".to_string()
            ))
        );
    }

    #[test]
    fn kbn_unknown_colourless_bag_far_from_kill_stays_unknown() {
        // An Unknown colourless Realm bag with no Beehemoth kill nearby must not
        // be stolen by a distant Beehemoth death.
        let kills = vec![kill(
            42,
            super::super::RED_BEEHEMOTH_OBJECT_TYPE,
            "Red Beehemoth",
            1_000,
        )];
        let picked = LootTracker::resolve_killer_bee_nest(&kills, 0, &[9999], 42, 60_000);
        assert!(picked.is_none());
    }

    #[test]
    fn kbn_override_fires_over_taunt_controller_prior() {
        // The full override path must reclassify a taunt-controller-attributed
        // Realm bag even though prior_mob_type != 0.
        let picked = LootTracker::resolve_boss_override_with(
            &[],
            LootBagType::White,
            "{s.rotmg}",
            super::super::EH_EVENT_TAUNT_CONTROLLER_OBJECT_TYPE,
            &[KBN_BLUE_QUIVER],
            42,
            1000,
        );
        assert_eq!(
            picked,
            Some((
                super::super::BLUE_BEEHEMOTH_OBJECT_TYPE,
                "Blue Beehemoth".to_string()
            ))
        );
    }

    #[test]
    fn kbn_override_does_not_steal_real_bramblethorn_bag() {
        // A bag already attributed to Corrupted Bramblethorn (a real proximity
        // source, prior != event source) is left untouched even if it holds a
        // single Beehemoth colour.
        let picked = LootTracker::resolve_boss_override_with(
            &[],
            LootBagType::White,
            super::super::REALM_NAME,
            super::super::CORRUPTED_BRAMBLETHORN_OBJECT_TYPE,
            &[KBN_BLUE_QUIVER],
            42,
            1000,
        );
        assert!(picked.is_none());
    }

    #[test]
    fn select_boss_ambiguous_ignores_unrelated_kill() {
        let cands = vec![(100, "Boss A".to_string()), (200, "Boss B".to_string())];
        // A boss was killed but it cannot drop these items (shoot-and-teleport
        // case): the bag must stay Unknown rather than be misattributed.
        let kills = vec![kill(42, 999, "Unrelated", 1000)];
        let picked = LootTracker::select_boss_from_candidates(&cands, &kills, 42, 1000);
        assert!(picked.is_none());
    }

    #[test]
    fn select_boss_ambiguous_requires_same_instance() {
        let cands = vec![(100, "Boss A".to_string()), (200, "Boss B".to_string())];
        // Right boss, wrong instance (different map seed): not correlated.
        let kills = vec![kill(7, 200, "Boss B", 1000)];
        let picked = LootTracker::select_boss_from_candidates(&cands, &kills, 42, 1000);
        assert!(picked.is_none());
    }

    #[test]
    fn select_boss_ambiguous_prefers_most_recent_kill() {
        let cands = vec![(100, "Boss A".to_string()), (200, "Boss B".to_string())];
        let kills = vec![kill(42, 100, "Boss A", 1000), kill(42, 200, "Boss B", 5000)];
        let picked = LootTracker::select_boss_from_candidates(&cands, &kills, 42, 6000);
        assert_eq!(picked, Some((200, "Boss B".to_string())));
    }

    #[test]
    fn select_boss_ambiguous_rejects_stale_kill() {
        let cands = vec![(100, "Boss A".to_string()), (200, "Boss B".to_string())];
        // Kill is far in the past relative to the bag (outside the window).
        let kills = vec![kill(42, 200, "Boss B", 1000)];
        let picked = LootTracker::select_boss_from_candidates(&cands, &kills, 42, 1_000_000);
        assert!(picked.is_none());
    }

    #[test]
    fn last_boss_none_without_kills() {
        assert!(LootTracker::select_last_boss_in_instance(&[], 42).is_none());
    }

    #[test]
    fn last_boss_none_for_unknown_seed() {
        let kills = vec![kill(42, 20493, "Kitsune Umi", 1000)];
        // map_seed 0 means the instance is unknown; never guess.
        assert!(LootTracker::select_last_boss_in_instance(&kills, 0).is_none());
    }

    #[test]
    fn last_boss_ignores_other_instances() {
        let kills = vec![kill(7, 20493, "Kitsune Umi", 5000)];
        assert!(LootTracker::select_last_boss_in_instance(&kills, 42).is_none());
    }

    #[test]
    fn last_boss_picks_most_recent_in_instance() {
        // Dancers fought first, Umi last: MV loot attributes to Umi.
        let kills = vec![
            kill(42, 20451, "Dancer Miko", 1000),
            kill(42, 20493, "Kitsune Umi", 5000),
            kill(9, 20450, "Sage Genji", 9000),
        ];
        let picked = LootTracker::select_last_boss_in_instance(&kills, 42);
        assert_eq!(picked, Some((20493, "Kitsune Umi".to_string())));
    }

    #[test]
    fn last_boss_breaks_ended_tie_by_start() {
        // The three MV dancers are finalized together by one completion marker,
        // so they share an ended_at; the latest-started was fought last.
        let kills = vec![
            kill_span(42, 20451, "Dancer Miko", 1000, 9000),
            kill_span(42, 20452, "Drummer Kaguya", 2000, 9000),
            kill_span(42, 20450, "Sage Genji", 3000, 9000),
        ];
        let picked = LootTracker::select_last_boss_in_instance(&kills, 42);
        assert_eq!(picked, Some((20450, "Sage Genji".to_string())));
    }

    #[test]
    fn test_on_map_info() {
        let mut tracker = LootTracker::new();
        tracker.on_map_info("The Void", 12345);
        // Just verify no panic
    }

    #[test]
    fn test_on_create_success() {
        let mut tracker = LootTracker::new();
        tracker.on_create_success(1234, 5678);
        // Just verify no panic
    }

    // --- Tomb of the Ancients liveness + attribution ---

    fn status_with_hp(hp: i32) -> ObjectStatusData {
        ObjectStatusData {
            object_id: 0,
            pos: crate::protocol::data::WorldPosData::default(),
            stats: vec![crate::protocol::data::StatData {
                stat_type_id: StatType::HP as u8,
                stat_type: StatType::HP,
                stat_value: hp,
                string_stat_value: None,
                stat_value_two: 0,
            }],
        }
    }

    #[test]
    fn tomb_read_hp_from_status() {
        assert_eq!(read_hp(&status_with_hp(500)), Some(500));
        let empty = ObjectStatusData {
            object_id: 0,
            pos: crate::protocol::data::WorldPosData::default(),
            stats: vec![],
        };
        assert_eq!(read_hp(&empty), None);
    }

    #[test]
    fn tomb_onscreen_boss_is_alive_and_not_a_candidate() {
        let mut state = TombState::default();
        state.note_spawn(1, super::super::TOMB_BES_OBJECT_TYPE, Some(1000), 10_000);
        assert!(state.is_alive(super::super::TOMB_BES_OBJECT_TYPE, 11_000));
        // Bes is alive; only Geb and Nut are dead candidates.
        let dead: Vec<_> = state
            .dead_candidates(11_000)
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        assert_eq!(
            dead,
            vec![
                super::super::TOMB_GEB_OBJECT_TYPE,
                super::super::TOMB_NUT_OBJECT_TYPE
            ]
        );
    }

    #[test]
    fn tomb_boss_becomes_candidate_after_alive_window() {
        let mut state = TombState::default();
        state.note_spawn(1, super::super::TOMB_NUT_OBJECT_TYPE, Some(1000), 10_000);
        // Well past the alive window: Nut is now a dead candidate.
        assert!(!state.is_alive(super::super::TOMB_NUT_OBJECT_TYPE, 20_000));
        let dead: Vec<_> = state
            .dead_candidates(20_000)
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        assert!(dead.contains(&super::super::TOMB_NUT_OBJECT_TYPE));
    }

    #[test]
    fn tomb_non_boss_spawn_ignored() {
        let mut state = TombState::default();
        state.note_spawn(1, 99999, Some(1000), 10_000);
        assert!(state.boss_object_types.is_empty());
    }

    #[test]
    fn tomb_reset_clears_state() {
        let mut state = TombState::default();
        state.note_spawn(1, super::super::TOMB_GEB_OBJECT_TYPE, Some(1000), 10_000);
        state.attributed.insert(super::super::TOMB_GEB_OBJECT_TYPE);
        state.reset(42);
        assert_eq!(state.map_seed, 42);
        assert!(state.boss_object_types.is_empty());
        assert!(state.last_alive_ms.is_empty());
        assert!(state.attributed.is_empty());
    }

    #[test]
    fn tomb_brown_bag_never_overridden() {
        let mut tracker = LootTracker::new();
        // Even with everything dead, a brown/public bag is never a boss drop.
        let hit = tracker.resolve_tomb_attribution(LootBagType::Brown, 0, &[], 10_000);
        assert!(hit.is_none());
    }

    #[test]
    fn tomb_unknown_bag_assigned_to_dead_boss_alphabetically() {
        let mut tracker = LootTracker::new();
        // No boss seen alive -> all three are dead candidates. Two distinct
        // offscreen (Unknown) bags go to distinct bosses, alphabetical order.
        let first = tracker.resolve_tomb_attribution(LootBagType::White, 0, &[], 10_000);
        assert_eq!(
            first.as_ref().map(|(t, _)| *t),
            Some(super::super::TOMB_BES_OBJECT_TYPE)
        );
        let second = tracker.resolve_tomb_attribution(LootBagType::White, 0, &[], 10_000);
        assert_eq!(
            second.as_ref().map(|(t, _)| *t),
            Some(super::super::TOMB_GEB_OBJECT_TYPE)
        );
    }

    #[test]
    fn tomb_rage_taunt_maps_to_boss() {
        assert_eq!(
            super::super::tomb_rage_taunt_boss("The end of your path is here!"),
            Some(super::super::TOMB_BES_OBJECT_TYPE)
        );
        assert_eq!(
            super::super::tomb_rage_taunt_boss("This cannot be! You shall not succeed!"),
            Some(super::super::TOMB_NUT_OBJECT_TYPE)
        );
        assert_eq!(
            super::super::tomb_rage_taunt_boss("Argh! You shall pay for your crimes!"),
            Some(super::super::TOMB_GEB_OBJECT_TYPE)
        );
        assert_eq!(super::super::tomb_rage_taunt_boss("Some other line"), None);
    }

    #[test]
    fn tomb_rage_order_overrides_alphabetical() {
        // Reproduces the reported bug: Nut raged (died) first, Geb second, both
        // offscreen. Two Unknown bags must map in death order (Nut, then Geb),
        // NOT alphabetically (which would wrongly start at Bes/Geb).
        let mut tracker = LootTracker::new();
        tracker
            .tomb
            .note_taunt(super::super::TOMB_NUT_OBJECT_TYPE, 10_000);
        tracker
            .tomb
            .note_taunt(super::super::TOMB_GEB_OBJECT_TYPE, 11_000);
        // Resolve past both alive windows so both count as dead.
        let first = tracker.resolve_tomb_attribution(LootBagType::White, 0, &[], 20_000);
        assert_eq!(
            first.as_ref().map(|(t, _)| *t),
            Some(super::super::TOMB_NUT_OBJECT_TYPE)
        );
        let second = tracker.resolve_tomb_attribution(LootBagType::White, 0, &[], 20_000);
        assert_eq!(
            second.as_ref().map(|(t, _)| *t),
            Some(super::super::TOMB_GEB_OBJECT_TYPE)
        );
    }

    #[test]
    fn tomb_raging_boss_still_alive_is_excluded() {
        // A boss that just taunted is still alive, so an offscreen bag must go to
        // an already-dead raged boss instead. Nut raged earlier (dead); Bes
        // raged just now (alive) -> the bag is Nut's, not Bes's.
        let mut tracker = LootTracker::new();
        tracker
            .tomb
            .note_taunt(super::super::TOMB_NUT_OBJECT_TYPE, 10_000);
        tracker
            .tomb
            .note_taunt(super::super::TOMB_BES_OBJECT_TYPE, 18_000);
        let hit = tracker.resolve_tomb_attribution(LootBagType::White, 0, &[], 19_000);
        assert_eq!(
            hit.as_ref().map(|(t, _)| *t),
            Some(super::super::TOMB_NUT_OBJECT_TYPE)
        );
    }

    #[test]
    fn tomb_rage_falls_back_to_alphabetical_when_no_taunt() {
        // With no taunts captured (rage skipped), behavior is unchanged: the
        // alphabetical dead-boss fallback still applies.
        let mut tracker = LootTracker::new();
        let hit = tracker.resolve_tomb_attribution(LootBagType::White, 0, &[], 10_000);
        assert_eq!(
            hit.as_ref().map(|(t, _)| *t),
            Some(super::super::TOMB_BES_OBJECT_TYPE)
        );
    }

    #[test]
    fn tomb_note_taunt_records_order_once() {
        let mut state = TombState::default();
        state.note_taunt(super::super::TOMB_GEB_OBJECT_TYPE, 10_000);
        state.note_taunt(super::super::TOMB_GEB_OBJECT_TYPE, 12_000);
        assert_eq!(state.rage_order, vec![super::super::TOMB_GEB_OBJECT_TYPE]);
        state.reset(0);
        assert!(state.rage_order.is_empty());
    }

    #[test]
    fn tomb_alive_boss_excluded_from_offscreen_assignment() {
        let mut tracker = LootTracker::new();
        // Bes is on-screen alive; an Unknown bag must go to Geb (next dead one).
        tracker
            .tomb
            .note_spawn(1, super::super::TOMB_BES_OBJECT_TYPE, Some(1000), 10_000);
        let hit = tracker.resolve_tomb_attribution(LootBagType::White, 0, &[], 11_000);
        assert_eq!(
            hit.as_ref().map(|(t, _)| *t),
            Some(super::super::TOMB_GEB_OBJECT_TYPE)
        );
    }

    #[test]
    fn tomb_good_attribution_recorded_and_kept() {
        let mut tracker = LootTracker::new();
        // A bag already attributed to a live main boss keeps its attribution,
        // and that boss is recorded so a later offscreen bag skips it.
        let hit = tracker.resolve_tomb_attribution(
            LootBagType::White,
            super::super::TOMB_BES_OBJECT_TYPE,
            &[],
            10_000,
        );
        assert!(hit.is_none());
        assert!(tracker
            .tomb
            .attributed
            .contains(&super::super::TOMB_BES_OBJECT_TYPE));
        // Next offscreen bag skips Bes -> Geb.
        let next = tracker.resolve_tomb_attribution(LootBagType::White, 0, &[], 10_000);
        assert_eq!(
            next.as_ref().map(|(t, _)| *t),
            Some(super::super::TOMB_GEB_OBJECT_TYPE)
        );
    }

    #[test]
    fn tomb_mark_of_geb_forces_geb_over_proximity() {
        let mut tracker = LootTracker::new();
        // A bag holding the Mark of Geb is always Geb, even when proximity
        // guessed another boss (here Nut).
        let hit = tracker.resolve_tomb_attribution(
            LootBagType::White,
            super::super::TOMB_NUT_OBJECT_TYPE,
            &[super::super::MARK_OF_GEB_ITEM_ID],
            10_000,
        );
        assert_eq!(
            hit.as_ref().map(|(t, _)| *t),
            Some(super::super::TOMB_GEB_OBJECT_TYPE)
        );
    }

    #[test]
    fn tomb_mark_of_geb_does_not_consume_geb_slot() {
        let mut tracker = LootTracker::new();
        // The Mark bag (a separate spawner bag) credits Geb but must not seed the
        // alphabetical fallback, so a later Life bag can still be handed to Geb.
        tracker.resolve_tomb_attribution(
            LootBagType::White,
            0,
            &[super::super::MARK_OF_GEB_ITEM_ID],
            10_000,
        );
        assert!(!tracker
            .tomb
            .attributed
            .contains(&super::super::TOMB_GEB_OBJECT_TYPE));
        // Two other bosses get credited, leaving Geb for a following bag.
        tracker
            .tomb
            .attributed
            .insert(super::super::TOMB_BES_OBJECT_TYPE);
        tracker
            .tomb
            .attributed
            .insert(super::super::TOMB_NUT_OBJECT_TYPE);
        let life = tracker.resolve_tomb_attribution(
            LootBagType::Blue,
            0,
            &[super::super::POTION_OF_LIFE_ITEM_ID],
            10_000,
        );
        assert_eq!(
            life.as_ref().map(|(t, _)| *t),
            Some(super::super::TOMB_GEB_OBJECT_TYPE)
        );
    }

    #[test]
    fn tomb_confirmed_dead_overrides_alive_window_and_resets() {
        let mut state = TombState::default();
        state.note_spawn(1, super::super::TOMB_GEB_OBJECT_TYPE, Some(1000), 10_000);
        assert!(state.is_alive(super::super::TOMB_GEB_OBJECT_TYPE, 11_000));
        state.note_confirmed_dead(super::super::TOMB_GEB_OBJECT_TYPE);
        assert!(!state.is_alive(super::super::TOMB_GEB_OBJECT_TYPE, 11_000));
        // A late HP update must not revive a confirmed-dead boss this instance.
        state.note_update(1, Some(1000), 12_000);
        assert!(!state.is_alive(super::super::TOMB_GEB_OBJECT_TYPE, 12_500));
        state.reset(0);
        assert!(state.confirmed_dead.is_empty());
        // After reset (new instance) the boss can be seen alive again.
        state.note_spawn(1, super::super::TOMB_GEB_OBJECT_TYPE, Some(1000), 20_000);
        assert!(state.is_alive(super::super::TOMB_GEB_OBJECT_TYPE, 20_500));
    }

    #[test]
    fn tomb_mark_confirms_offscreen_boss_dead_for_life_bag() {
        // Reported bug: Geb ran offscreen still "alive" (last seen within the 4s
        // window), then died there. Bes and Nut were credited on-screen. The Mark
        // of Geb bag (discovered first, center of room) proves all three died, so
        // the following offscreen Life bag must attribute to Geb rather than
        // staying Unknown.
        let mut tracker = LootTracker::new();
        tracker
            .tomb
            .attributed
            .insert(super::super::TOMB_BES_OBJECT_TYPE);
        tracker
            .tomb
            .attributed
            .insert(super::super::TOMB_NUT_OBJECT_TYPE);
        // Geb last seen alive at t=10_000; the Life bag arrives at t=11_000,
        // inside the alive window, so without the Mark he'd be excluded.
        tracker
            .tomb
            .note_spawn(1, super::super::TOMB_GEB_OBJECT_TYPE, Some(1000), 10_000);
        assert!(tracker
            .tomb
            .is_alive(super::super::TOMB_GEB_OBJECT_TYPE, 11_000));
        // Mark bag resolves first -> confirms all three main bosses dead.
        tracker.resolve_tomb_attribution(
            LootBagType::White,
            0,
            &[super::super::MARK_OF_GEB_ITEM_ID],
            10_500,
        );
        assert!(!tracker
            .tomb
            .is_alive(super::super::TOMB_GEB_OBJECT_TYPE, 11_000));
        // Geb's real offscreen Life bag now attributes to Geb.
        let hit = tracker.resolve_tomb_attribution(
            LootBagType::White,
            0,
            &[super::super::POTION_OF_LIFE_ITEM_ID],
            11_000,
        );
        assert_eq!(
            hit.as_ref().map(|(t, _)| *t),
            Some(super::super::TOMB_GEB_OBJECT_TYPE)
        );
    }

    #[test]
    fn tomb_life_on_artifact_is_stripped_when_unrecoverable() {
        let mut tracker = LootTracker::new();
        // A Life bag proximity-attributed to an artifact is mis-attributed, but
        // with every main boss already credited it cannot be re-resolved, so it
        // must be stripped to Unknown rather than keep the artifact credit.
        tracker
            .tomb
            .attributed
            .insert(super::super::TOMB_BES_OBJECT_TYPE);
        tracker
            .tomb
            .attributed
            .insert(super::super::TOMB_GEB_OBJECT_TYPE);
        tracker
            .tomb
            .attributed
            .insert(super::super::TOMB_NUT_OBJECT_TYPE);
        let artifact_type = 3361; // Nile Artifact - never drops Life.
        let hit = tracker.resolve_tomb_attribution(
            LootBagType::Blue,
            artifact_type,
            &[super::super::POTION_OF_LIFE_ITEM_ID],
            10_000,
        );
        assert_eq!(hit, Some((0, "Unknown".to_string())));
    }

    #[test]
    fn tomb_life_on_main_boss_is_kept() {
        let mut tracker = LootTracker::new();
        // Life legitimately drops from the main bosses, so a Life bag already
        // attributed to Bes must be kept (and seed the run).
        let hit = tracker.resolve_tomb_attribution(
            LootBagType::Blue,
            super::super::TOMB_BES_OBJECT_TYPE,
            &[super::super::POTION_OF_LIFE_ITEM_ID],
            10_000,
        );
        assert!(hit.is_none());
        assert!(tracker
            .tomb
            .attributed
            .contains(&super::super::TOMB_BES_OBJECT_TYPE));
    }

    #[test]
    fn tomb_second_life_bag_on_same_boss_goes_to_dead_boss() {
        let mut tracker = LootTracker::new();
        // Bes and Geb are on-screen alive; Nut died offscreen. Proximity credits
        // two Life bags to Geb. The first keeps Geb; the second must not stack on
        // Geb -- it is redistributed to the dead un-credited boss, Nut.
        tracker
            .tomb
            .note_spawn(1, super::super::TOMB_BES_OBJECT_TYPE, Some(1000), 5_000);
        tracker
            .tomb
            .note_spawn(2, super::super::TOMB_GEB_OBJECT_TYPE, Some(1000), 5_000);

        let first = tracker.resolve_tomb_attribution(
            LootBagType::Blue,
            super::super::TOMB_GEB_OBJECT_TYPE,
            &[super::super::POTION_OF_LIFE_ITEM_ID],
            6_000,
        );
        assert!(first.is_none());
        assert!(tracker
            .tomb
            .attributed
            .contains(&super::super::TOMB_GEB_OBJECT_TYPE));

        let second = tracker.resolve_tomb_attribution(
            LootBagType::Blue,
            super::super::TOMB_GEB_OBJECT_TYPE,
            &[super::super::POTION_OF_LIFE_ITEM_ID],
            6_000,
        );
        assert_eq!(
            second.as_ref().map(|(t, _)| *t),
            Some(super::super::TOMB_NUT_OBJECT_TYPE)
        );
    }

    #[test]
    fn tomb_duplicate_boss_bag_keeps_boss_when_no_free_boss() {
        let mut tracker = LootTracker::new();
        // All three main bosses already credited. A second bag proximity-credited
        // to Geb has no free boss to move to; it keeps Geb (a slightly over-counted
        // but valid boss) rather than becoming Unknown.
        tracker
            .tomb
            .attributed
            .insert(super::super::TOMB_BES_OBJECT_TYPE);
        tracker
            .tomb
            .attributed
            .insert(super::super::TOMB_GEB_OBJECT_TYPE);
        tracker
            .tomb
            .attributed
            .insert(super::super::TOMB_NUT_OBJECT_TYPE);
        let hit = tracker.resolve_tomb_attribution(
            LootBagType::Blue,
            super::super::TOMB_GEB_OBJECT_TYPE,
            &[super::super::POTION_OF_LIFE_ITEM_ID],
            10_000,
        );
        assert!(hit.is_none());
    }

    #[test]
    fn tomb_spawner_only_bag_goes_to_geb_without_slot() {
        // Needs real assets to categorize the cookie as a spawner item.
        let mgr = get_asset_manager();
        if let Some(dir) = crate::assets::find_assets_dir() {
            mgr.set_assets_dir(&dir);
        }
        if !mgr.try_load() || mgr.get_object(3268).is_none() {
            eprintln!("skipping: game assets not available");
            return;
        }

        let mut tracker = LootTracker::new();
        // A cookie-only spawner bag proximity-attributed to Nut must be credited
        // to Geb without consuming Nut's slot, so Nut's real bag is unaffected.
        let hit = tracker.resolve_tomb_attribution(
            LootBagType::White,
            super::super::TOMB_NUT_OBJECT_TYPE,
            &[3268], // Chocolate Cream Sandwich Cookie (pet food)
            10_000,
        );
        assert_eq!(
            hit.as_ref().map(|(t, _)| *t),
            Some(super::super::TOMB_GEB_OBJECT_TYPE)
        );
        assert!(!tracker
            .tomb
            .attributed
            .contains(&super::super::TOMB_NUT_OBJECT_TYPE));
        assert!(!tracker
            .tomb
            .attributed
            .contains(&super::super::TOMB_GEB_OBJECT_TYPE));
    }

    // --- Ice Tomb soul attribution ---

    #[test]
    fn ice_tomb_soul_maps_to_boss() {
        assert_eq!(
            super::super::ice_tomb_boss_name_for_soul(45638),
            Some("Frimar")
        );
        assert_eq!(
            super::super::ice_tomb_boss_name_for_soul(45639),
            Some("Polaris")
        );
        assert_eq!(
            super::super::ice_tomb_boss_name_for_soul(45640),
            Some("Glacius")
        );
        assert_eq!(super::super::ice_tomb_boss_name_for_soul(3367), None);
        assert!(super::super::is_forced_loot_dropper(45638));
        assert!(!super::super::is_forced_loot_dropper(3367));
    }

    #[test]
    fn ice_tomb_unique_ring_names_boss() {
        assert_eq!(super::super::ice_tomb_unique_boss(&[32724]), Some("Frimar"));
        assert_eq!(
            super::super::ice_tomb_unique_boss(&[32722]),
            Some("Polaris")
        );
        assert_eq!(
            super::super::ice_tomb_unique_boss(&[32723]),
            Some("Glacius")
        );
        // Shared drops (Freezing Quiver) carry no unique signal.
        assert_eq!(super::super::ice_tomb_unique_boss(&[5139]), None);
        // Two different bosses' rings in one bag is ambiguous.
        assert_eq!(super::super::ice_tomb_unique_boss(&[32724, 32723]), None);
    }

    #[test]
    fn ice_tomb_rage_taunt_maps_to_boss() {
        assert_eq!(
            super::super::ice_tomb_rage_taunt_boss(
                "NO, NO! I SHALL ENCASE YOU IN THIS PRISON'S HEART!"
            ),
            Some("Frimar")
        );
        assert_eq!(
            super::super::ice_tomb_rage_taunt_boss("I'LL SHOW YOU A MELTDOWN!"),
            Some("Polaris")
        );
        assert_eq!(
            super::super::ice_tomb_rage_taunt_boss(
                "I...am...failing...you......Geb......I...am sorry..."
            ),
            Some("Glacius")
        );
        assert_eq!(
            super::super::ice_tomb_rage_taunt_boss("Some other line"),
            None
        );
    }

    #[test]
    fn ice_tomb_has_boss_loot_from_drop_table() {
        // Freezing Quiver is an Ice Tomb boss drop in the embedded tables.
        assert!(super::super::ice_tomb_has_boss_loot(&[5139]));
        // A random unrelated item is not.
        assert!(!super::super::ice_tomb_has_boss_loot(&[999_999]));
    }

    #[test]
    fn ice_tomb_state_next_uncredited_prefers_rage_order() {
        let mut state = IceTombState::default();
        state.note_taunt("Polaris");
        state.note_taunt("Glacius");
        // Rage (death) order takes priority over the alphabetical roster.
        assert_eq!(state.next_uncredited(), Some("Polaris"));
        state.credit("Polaris");
        assert_eq!(state.next_uncredited(), Some("Glacius"));
        state.credit("Glacius");
        // No taunt captured for Frimar, but it is the only boss left.
        assert_eq!(state.next_uncredited(), Some("Frimar"));
    }

    #[test]
    fn ice_tomb_soul_bag_remaps_to_boss() {
        let mut tracker = LootTracker::new();
        // A bag proximity-tagged with the Support soul is Polaris' bag.
        let hit = tracker.resolve_ice_tomb_attribution(
            LootBagType::White,
            super::super::ICE_TOMB_SUPPORT_SOUL_OBJECT_TYPE,
            &[5139],
        );
        assert_eq!(
            hit,
            Some((
                super::super::ICE_TOMB_POLARIS_OBJECT_TYPE,
                "Polaris".to_string()
            ))
        );
        assert!(tracker.ice_tomb.attributed.contains(&"Polaris"));
    }

    #[test]
    fn ice_tomb_unknown_bag_distributes_by_roster() {
        let mut tracker = LootTracker::new();
        // Two offscreen soul bags arrive Unknown but hold boss loot; they are
        // handed to distinct bosses in alphabetical order (no taunts captured).
        let first = tracker.resolve_ice_tomb_attribution(LootBagType::White, 0, &[5139]);
        assert_eq!(first.as_ref().map(|(_, n)| n.as_str()), Some("Frimar"));
        let second = tracker.resolve_ice_tomb_attribution(LootBagType::White, 0, &[5139]);
        assert_eq!(second.as_ref().map(|(_, n)| n.as_str()), Some("Glacius"));
    }

    #[test]
    fn ice_tomb_duplicate_soul_bag_goes_to_other_boss() {
        let mut tracker = LootTracker::new();
        // Two souls died overlapping, so proximity tagged both bags with Polaris.
        let first = tracker.resolve_ice_tomb_attribution(
            LootBagType::White,
            super::super::ICE_TOMB_SUPPORT_SOUL_OBJECT_TYPE,
            &[5139],
        );
        assert_eq!(first.as_ref().map(|(_, n)| n.as_str()), Some("Polaris"));
        // The second Polaris-tagged bag is redistributed to an un-credited boss.
        let second = tracker.resolve_ice_tomb_attribution(
            LootBagType::White,
            super::super::ICE_TOMB_SUPPORT_SOUL_OBJECT_TYPE,
            &[5139],
        );
        assert_eq!(second.as_ref().map(|(_, n)| n.as_str()), Some("Frimar"));
    }

    #[test]
    fn ice_tomb_unique_ring_overrides_wrong_soul_tag() {
        let mut tracker = LootTracker::new();
        // Overlapping deaths mis-tagged this bag with Polaris' soul, but it holds
        // Frimar's unique ring -- the ring wins and credits Frimar, not Polaris.
        let hit = tracker.resolve_ice_tomb_attribution(
            LootBagType::White,
            super::super::ICE_TOMB_SUPPORT_SOUL_OBJECT_TYPE,
            &[32724],
        );
        assert_eq!(hit.as_ref().map(|(_, n)| n.as_str()), Some("Frimar"));
        assert!(tracker.ice_tomb.attributed.contains(&"Frimar"));
        assert!(!tracker.ice_tomb.attributed.contains(&"Polaris"));
    }

    #[test]
    fn ice_tomb_non_boss_unknown_bag_stays_unknown() {
        let mut tracker = LootTracker::new();
        // An Unknown bag that holds no Ice Tomb boss loot is not forced to a boss.
        let hit = tracker.resolve_ice_tomb_attribution(LootBagType::White, 0, &[999_999]);
        assert!(hit.is_none());
    }

    #[test]
    fn ice_tomb_brown_bag_is_never_a_boss_drop() {
        let mut tracker = LootTracker::new();
        let hit = tracker.resolve_ice_tomb_attribution(LootBagType::Brown, 0, &[5139]);
        assert!(hit.is_none());
    }

    #[test]
    fn mark_of_the_barkeep_attributes_to_bradley() {
        // Bradley never drops loot directly, so his bag arrives Unknown; the Mark
        // of the Barkeep is exclusive to him and forces the attribution.
        let hit = LootTracker::resolve_boss_override_with(
            &[],
            LootBagType::White,
            "The Tavern",
            0,
            &[super::super::MARK_OF_THE_BARKEEP_ITEM_ID],
            0,
            0,
        );
        assert_eq!(
            hit,
            Some((
                super::super::TAVERN_BARKEEP_OBJECT_TYPE,
                super::super::TAVERN_BARKEEP_NAME.to_string()
            ))
        );
    }

    #[test]
    fn mark_of_the_barkeep_overrides_a_misattributed_bag() {
        // Even when proximity credited the bag to another mob, the Mark wins.
        let hit = LootTracker::resolve_boss_override_with(
            &[],
            LootBagType::White,
            "The Tavern",
            12345,
            &[super::super::MARK_OF_THE_BARKEEP_ITEM_ID],
            0,
            0,
        );
        assert_eq!(
            hit.as_ref().map(|(t, _)| *t),
            Some(super::super::TAVERN_BARKEEP_OBJECT_TYPE)
        );
    }
}

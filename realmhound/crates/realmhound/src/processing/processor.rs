//! The `PacketProcessor` owns the full packet-processing model and runs the
//! entire `process_packets` + `dispatch_event` pipeline, emitting derived
//! [`UiUpdate`]s and maintaining a coalesced [`ViewState`] snapshot.
//!
//! Session 1: driven synchronously from the UI's `update()`. Session 2 moves it
//! onto a worker thread; the public surface (`process_available`, `apply_control`,
//! the accessors, and the contract types) is shaped to make that move mechanical.

use std::sync::{Arc, RwLock};

use std::path::PathBuf;

use realmhound_core::{
    capture::{CaptureWriter, RawPacket},
    combat::CombatManager,
    loot::{LootTracker, RecentBossKill},
    protocol::{ip_to_server_name, parse_packet_with_status, Packet, ParsedPacket},
    session::{AccountVerifyResult, HelloAction},
    stats::{
        extract_account_identity, extract_encounter_spawns, extract_inventory_changes,
        extract_pet_identity, extract_pet_inventory_updates, parse_player_appearance,
        parse_player_inventory,
    },
    stream::{parse_tcp_segment, ConnectionKey, TcpReassembler},
    vault::{
        AccountData, AccountDataRepository, CloseCallSync, LiveVaultData, LiveVaultItem,
        StatAwardSync, VaultType,
    },
    GameEvent, GameSession, PacketRouter, RouteResult, Settings,
};

use crate::panels::chat::{ChatMessage, ChatType};
use crate::processing::mission_tracker::{MissionTracker, MissionView};
use crate::sound::SoundType;

use super::contract::{
    AccountOperationScope, AudioCommand, ControlMsg, DbCloseError, UiPayload, UiUpdate, ViewState,
};
use realmhound_core::account::{AccountId, AccountKey};

/// Failure opening a selected profile's database writers during startup.
///
/// Returned by [`PacketProcessor::new_selected`] so selected-profile startup can
/// present a recoverable error instead of running silently without persistence.
#[derive(Debug)]
pub enum ProcessorInitError {
    /// A history database writer could not be opened.
    Database {
        /// Which writer failed (`"combat"` or `"loot"`).
        label: &'static str,
        /// Redacted underlying error detail.
        detail: String,
    },
}

impl std::fmt::Display for ProcessorInitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProcessorInitError::Database { label, detail } => {
                write!(f, "failed to open the {label} history database: {detail}")
            }
        }
    }
}

impl std::error::Error for ProcessorInitError {}

/// Current wall-clock time in epoch milliseconds. Used for combat fight timing
/// on events that do not carry a packet timestamp (map change, damage, hits).
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Extract a JSON string field value: given text containing `"key":"value"`,
/// returns `value`. Minimal, dependency-free parser tolerant of surrounding
/// text and whitespace after the colon.
fn extract_json_string_field(text: &str, key: &str) -> Option<String> {
    let anchor = format!("\"{key}\"");
    let after_key = text.find(&anchor)? + anchor.len();
    let rest = &text[after_key..];
    let colon = rest.find(':')?;
    let after_colon = &rest[colon + 1..];
    let open = after_colon.find('"')? + 1;
    let value = &after_colon[open..];
    let end = value.find('"')?;
    Some(value[..end].to_string())
}

/// The three Lost Halls monuments, in the fixed display order used for the
/// combined Oryx's Sanctuary entry. Each carries its Rune item's display name
/// (for the sprite lookup).
const MONUMENTS: [(&str, &str); 3] = [
    ("Sword", "Sword Rune"),
    ("Shield", "Shield Rune"),
    ("Helmet", "Helmet Rune"),
];

/// A parsed "area unlock" broadcast.
#[derive(Debug)]
enum AreaUnlockMsg {
    /// A single-item pop (Wine Cellar Incantation or Vial of Pure Darkness) that
    /// produces an immediate Live Feed entry.
    Single {
        item_name: &'static str,
        /// Full entry title, e.g. "Wine Cellar unlocked".
        title: &'static str,
        short: &'static str,
        popper: String,
    },
    /// A Lost Halls Rune monument activation. These are accumulated (last popper
    /// per monument wins) and only surface as a combined entry on Oryx's
    /// Sanctuary entry.
    Monument {
        /// Index into [`MONUMENTS`] (0 = Sword, 1 = Shield, 2 = Helmet).
        index: usize,
        popper: String,
    },
}

/// Map a dungeon display name to its area-unlock Live Feed entry, or `None` for
/// dungeons that don't produce a pop.
fn dungeon_unlock_single(dungeon: &str, popper: String) -> Option<AreaUnlockMsg> {
    let (item_name, title, short) = match dungeon.trim() {
        "Wine Cellar" => ("Wine Cellar Incantation", "Wine Cellar unlocked", "inc"),
        "The Void" => ("Vial of Pure Darkness", "The Void unlocked", "vial"),
        _ => return None,
    };
    Some(AreaUnlockMsg::Single {
        item_name,
        title,
        short,
        popper,
    })
}

/// Resolve a monument index from a prefix like "The Shield Monument".
fn monument_index_from_prefix(prefix: &str) -> Option<usize> {
    let rune = prefix
        .trim()
        .strip_prefix("The ")?
        .strip_suffix(" Monument")?;
    MONUMENTS.iter().position(|(k, _)| *k == rune)
}

/// Recognize a plain-text area-unlock broadcast and extract the popper name.
/// Auto-activations are silent (no message), so a missing monument simply has no
/// popper. Returns `None` for anything else.
fn parse_area_unlock(text: &str) -> Option<AreaUnlockMsg> {
    let text = text.trim();
    // "<Dungeon> unlocked by <name>".
    if let Some((dungeon, name)) = text.split_once(" unlocked by ") {
        return dungeon_unlock_single(dungeon, name.trim().to_string());
    }
    // "The <Sword|Shield|Helmet> Monument has been activated by <name>".
    if let Some((prefix, name)) = text.split_once(" has been activated by ") {
        let index = monument_index_from_prefix(prefix)?;
        return Some(AreaUnlockMsg::Monument {
            index,
            popper: name.trim().to_string(),
        });
    }
    None
}

/// Recognize a server death-broadcast line and extract the dead player's name
/// (and killer). RotMG announces every death in an instance as a Text message
/// like "Yumiho died at level 20, killed by Plagued Honey"; this is the reliable
/// death signal for the combat tracker (position-based grave correlation
/// mis-assigns graves when several players die in a tight cluster). Parses the
/// name from the text; falls back to the packet `name` field when the line
/// begins with "died at level". Returns `(player, killer)`.
fn parse_remote_death(sender: &str, text: &str) -> Option<(String, String)> {
    let text = text.trim();
    // The name precedes " died at level "; when the text itself begins with
    // "died at level" the name lives in the packet's sender field instead.
    let (before, after) = if let Some(rest) = text.strip_prefix("died at level ") {
        ("", rest)
    } else {
        text.split_once(" died at level ")?
    };
    // "<level>, killed by <killer>" -- killer is best-effort context only.
    let killer = after
        .split_once(", killed by ")
        .map(|(_, k)| k.trim().trim_end_matches('.').trim().to_string())
        .unwrap_or_default();
    let name = if before.is_empty() {
        sender.trim().to_string()
    } else {
        before.trim().to_string()
    };
    if name.is_empty() {
        return None;
    }
    Some((name, killer))
}

/// - `{"k":"s.dungeon_unlocked_by","t":{"name":"Wine Cellar","player":"Jeff",}}`
/// - `{"k":"s.something_by_player","t":{"name":"The Shield Monument has been activated","player":"MsXepher",}}`
fn parse_area_unlock_json(message: &str) -> Option<AreaUnlockMsg> {
    let name = extract_json_string_field(message, "name")?;
    let player = extract_json_string_field(message, "player")?;
    // Player names can carry a `,<discriminator>` suffix (e.g. "Tiffinia,1cf5").
    let popper = player
        .split(',')
        .next()
        .unwrap_or(&player)
        .trim()
        .to_string();
    if popper.is_empty() {
        return None;
    }
    // Branch on the message key directly rather than extracting the very short
    // `"k"` field, which the lenient extractor could confuse with a stray `k`.
    if message.contains("s.dungeon_unlocked_by") {
        dungeon_unlock_single(&name, popper)
    } else if message.contains("s.something_by_player") {
        let prefix = name.trim().strip_suffix(" has been activated")?;
        let index = monument_index_from_prefix(prefix)?;
        Some(AreaUnlockMsg::Monument { index, popper })
    } else {
        None
    }
}

/// The full packet-processing model. Owns the game session, combat engine,
/// loot tracker, and account data, and emits [`UiUpdate`]s for the UI.
pub struct PacketProcessor {
    // --- Model (moved out of `RealmHoundApp`) ---
    reassembler: TcpReassembler,
    session: GameSession,
    router: PacketRouter,
    loot_tracker: LootTracker,
    /// Combat History engine: reconstructs boss fights from the packet stream.
    combat: CombatManager,
    /// Single source of truth for the persistent model, including the vault
    /// chests (`regular_vault`/`seasonal_vault`). Both full vault snapshots and
    /// incremental in-session vault moves mutate this directly so they are saved.
    account_data: AccountData,
    /// Explicit persistence binding for `account_data` periodic/final saves.
    account_repository: AccountDataRepository,
    #[cfg(test)]
    _test_account_root: Option<tempfile::TempDir>,
    settings: Arc<RwLock<Settings>>,

    /// Currently playing character id (from CreateSuccess). Owned here so the
    /// character cache (in `account_data`) stays the single source of truth.
    live_char_id: Option<i32>,
    /// Latest raw local-player stat values (id -> value), accumulated across the
    /// spawn `Update` (full snapshot) and `NewTick` deltas. Base character stats
    /// are derived from these (base = stat - boost) to keep the cached stats and
    /// maxed count current live. Cleared on character load and on death.
    live_stat_raw: std::collections::HashMap<u8, i32>,
    /// Close calls detected but not yet persisted onto their character's tally,
    /// keyed by char id. Retried every tick so a dip is never lost when the
    /// character is momentarily absent from the cache (nexus / reconnect refresh
    /// window) at the moment it is drained from the tracker.
    pending_close_calls_by_char: std::collections::HashMap<i32, u32>,
    /// Fight-card-derived lifetime close-call totals awaiting application, keyed
    /// by char id. Held (merged by max) and retried until the character is
    /// present in the cache, so the retroactive backfill survives a reconnect
    /// refresh window or a not-yet-loaded roster.
    pending_close_call_totals: std::collections::HashMap<i32, i64>,
    /// Authoritative lifetime award totals awaiting application. Missing
    /// characters remain queued across cache refresh windows.
    pending_stat_awards: std::collections::HashMap<i32, (i64, i64, i64)>,
    /// Dedup signature for dungeon callout alert sounds.
    last_dungeon_alert: Option<(i32, String, Vec<String>, Option<String>)>,
    /// Full pet-object status captured at spawn, keyed by the pet's world object
    /// id. A pet's inventory + identity ride only in its spawn `Update`; later
    /// NewTicks are position-only. Detection can latch a tick after the spawn
    /// (the ActivePetUpdate mapping or the map-load proximity latch arrives
    /// later), so we buffer every known pet's spawn status and replay the
    /// detected pet's once the mapping is known, so the initial inventory is not
    /// lost (e.g. a pet equipped in a prior session, or observed at map load).
    pet_object_buffer:
        std::collections::HashMap<i32, realmhound_core::protocol::data::ObjectStatusData>,
    /// Object id whose buffered spawn status has already been applied, so we
    /// enrich a detected pet only once instead of re-emitting slot updates every
    /// tick. Reset when the detected pet changes or is lost.
    enriched_pet_object_id: Option<i32>,

    // --- View bookkeeping ---
    /// Bumps on every character-cache mutation (drives the characters mirror).
    char_view_gen: u64,
    /// Bumps on every live-vault mutation (drives vault/Treasury resync).
    vault_view_gen: u64,
    /// Monotonic sequence for emitted updates.
    seq: u64,
    /// Scratch buffer of updates emitted for the current `process_available` call.
    updates: Vec<UiUpdate>,

    /// Optional raw-packet recorder. Activated at capture start when the
    /// `REALMHOUND_RECORD` env var is set (spike/dev tool for Combat History).
    recorder: Option<CaptureWriter>,

    /// When the player last entered a realm map. Oryx event-spawn taunts for
    /// events already alive are re-broadcast by the server as the client syncs
    /// into the realm; the official client hides these, so we suppress boss
    /// calls that arrive within [`REALM_JOIN_SETTLE`] of entering a realm.
    /// `None` outside a realm (nexus/dungeon) or after a disconnect.
    realm_entered_at: Option<std::time::Instant>,

    /// Realm-scoped latch for the Keyper crystal-tower notification. Set when a
    /// "towers appeared" cue fires; reset when The Keyper spawns (the towers
    /// were just destroyed, so the next tower appearance is a genuinely new
    /// wave) and when entering a *different* realm. Suppresses the repeated
    /// tower-entity spawns that re-enter `new_objects` as the player moves
    /// around a realm.
    keyper_towers_announced: bool,

    /// `realm_name` of the realm the Keyper latch currently applies to. Used so
    /// that leaving to a dungeon/nexus and returning to the *same* realm keeps
    /// the latch (no duplicate tower notification), while entering a different
    /// realm re-arms it.
    keyper_realm: Option<String>,

    /// Events buffered while the current connection is a tentative (unverified)
    /// main and a saved main account already exists. RotMG opens a fresh socket
    /// per map change, so the real main re-enters this tentative window every
    /// time it changes areas. Everything it emits before its identifying
    /// `Update` verifies is held here and replayed in order the moment it
    /// verifies (so no loot/combat/location/objects are lost), or discarded if
    /// the connection turns out to be a mule. Bounded by [`PENDING_EVENT_CAP`].
    pending_events: Vec<GameEvent>,

    /// Last human popper per Lost Halls monument (index 0 = Sword, 1 = Shield,
    /// 2 = Helmet), accumulated while in Lost Halls. Auto-activations are silent,
    /// so a monument with no message stays `None`. Surfaced as one combined Live
    /// Feed entry on Oryx's Sanctuary entry, then cleared; also cleared when
    /// leaving to any map other than Lost Halls.
    monument_poppers: [Option<String>; 3],

    /// Seasonal battle-pass mission tracker: combines HTTP
    /// definitions/progress with live packet-165 deltas and claim detection.
    mission_tracker: MissionTracker,

    /// The selected account this worker is bound to. Set once at worker spawn;
    /// async completions whose scope names a different account (or `None` here)
    /// are rejected before touching account-scoped state.
    selected_scope: Option<SelectedScope>,
    /// Access token captured from a connection's HELLO, held until that
    /// connection is verified so an unverified/foreign candidate never releases
    /// credentials. Cleared when the candidate is discarded.
    pending_credential: Option<PendingCredential>,
    /// Last applied generation for Realm-API account-data completions.
    last_account_gen: u64,
    /// Last applied generation for seasonal-mission completions.
    last_mission_gen: u64,
}

/// The selected account a worker is bound to, used to validate async completions.
#[derive(Debug, Clone)]
struct SelectedScope {
    account_key: AccountKey,
    account_id: AccountId,
}

/// An access token bound to the connection that presented it, pending
/// identity verification of that connection.
#[derive(Debug, Clone)]
struct PendingCredential {
    connection: ConnectionKey,
    token: String,
}

/// Boss-call taunts received within this window of entering a realm are treated
/// as the server's join-time state replay (not witnessed spawns) and dropped.
const REALM_JOIN_SETTLE: std::time::Duration = std::time::Duration::from_secs(2);

/// Safety cap on the tentative-connection event buffer. The identifying
/// `Update` normally arrives within the first handful of packets, so this is
/// only a guard against unbounded growth on a connection that never verifies
/// (e.g. a stuck mule); beyond it we stop buffering rather than leak memory.
const PENDING_EVENT_CAP: usize = 4096;

/// How long a tentative candidate may stay unverified before it is discarded.
/// The identifying `Update` normally arrives within a second; a candidate that
/// never identifies itself in this window is dropped so its buffered events are
/// released and the main slot is freed for the next connection.
const IDENTITY_TIMEOUT_MS: i64 = 60_000;

impl PacketProcessor {
    /// Build a processor from flat-layout paths, degrading writer failures to
    /// in-memory. Superseded by [`Self::new_selected`] for selected-profile
    /// startup (which opens writers strictly and propagates failures); retained
    /// for reference and non-selected callers.
    #[allow(dead_code)]
    pub fn new(
        settings: Arc<RwLock<Settings>>,
        account_data: AccountData,
        account_repository: AccountDataRepository,
        loot_path: PathBuf,
        combat_path: PathBuf,
        saved_account_id: Option<String>,
        detected_account_name: Option<String>,
    ) -> Self {
        let session = {
            let mut session = GameSession::new();
            session.connection.saved_account_id = saved_account_id;
            session.connection.detected_account_name = detected_account_name;
            session
        };

        // The loot writer creates the schema before the UI reader opens, and its
        // v13 backfill correlates against the paired combat history database.
        let mut loot_tracker = match LootTracker::with_database(&loot_path, Some(&combat_path)) {
            Ok(tracker) => {
                tracing::info!("[LOOT] Initialized loot tracker with database");
                tracker
            }
            Err(e) => {
                tracing::warn!(
                    "[LOOT] Failed to open database: {}, using in-memory only",
                    e
                );
                LootTracker::new()
            }
        };
        if let Ok(s) = settings.read() {
            loot_tracker.set_tracking_settings(&s.loot_tracking);
        }

        let mut combat = CombatManager::new();
        combat.open_database(&combat_path, Some(&loot_path));
        if let Ok(s) = settings.read() {
            combat.set_history_settings(s.combat_history.clone());
        }

        Self {
            reassembler: TcpReassembler::new(),
            session,
            router: PacketRouter::new(),
            loot_tracker,
            combat,
            account_data,
            account_repository,
            #[cfg(test)]
            _test_account_root: None,
            settings,
            live_char_id: None,
            live_stat_raw: std::collections::HashMap::new(),
            pending_close_calls_by_char: std::collections::HashMap::new(),
            pending_close_call_totals: std::collections::HashMap::new(),
            pending_stat_awards: std::collections::HashMap::new(),
            last_dungeon_alert: None,
            pet_object_buffer: std::collections::HashMap::new(),
            enriched_pet_object_id: None,
            char_view_gen: 0,
            vault_view_gen: 0,
            seq: 0,
            updates: Vec::new(),
            recorder: None,
            realm_entered_at: None,
            keyper_towers_announced: false,
            keyper_realm: None,
            pending_events: Vec::new(),
            monument_poppers: [None, None, None],
            mission_tracker: MissionTracker::new(),
            selected_scope: None,
            pending_credential: None,
            last_account_gen: 0,
            last_mission_gen: 0,
        }
    }

    /// Build a processor for a validated selected profile, opening the database
    /// writers before any UI reader. The combat writer initializes before the
    /// loot writer because the loot backfill consults the combat database. A
    /// writer failure propagates instead of degrading to in-memory, so a broken
    /// profile never runs silently without persistence.
    #[allow(clippy::too_many_arguments)]
    pub fn new_selected(
        settings: Arc<RwLock<Settings>>,
        account_data: AccountData,
        account_repository: AccountDataRepository,
        loot_path: PathBuf,
        combat_path: PathBuf,
        saved_account_id: Option<String>,
        detected_account_name: Option<String>,
    ) -> Result<Self, ProcessorInitError> {
        let session = {
            let mut session = GameSession::new();
            session.connection.saved_account_id = saved_account_id;
            session.connection.detected_account_name = detected_account_name;
            session
        };

        // Combat writer first: it creates/validates the combat database that the
        // loot writer's backfill correlates against.
        let mut combat = CombatManager::new();
        combat
            .open_database_strict(&combat_path, Some(&loot_path))
            .map_err(|source| ProcessorInitError::Database {
                label: "combat",
                detail: source.to_string(),
            })?;
        if let Ok(s) = settings.read() {
            combat.set_history_settings(s.combat_history.clone());
        }

        // Loot writer second: creates the schema before the UI reader opens.
        let mut loot_tracker =
            LootTracker::with_database(&loot_path, Some(&combat_path)).map_err(|source| {
                ProcessorInitError::Database {
                    label: "loot",
                    detail: source.to_string(),
                }
            })?;
        if let Ok(s) = settings.read() {
            loot_tracker.set_tracking_settings(&s.loot_tracking);
        }

        Ok(Self {
            reassembler: TcpReassembler::new(),
            session,
            router: PacketRouter::new(),
            loot_tracker,
            combat,
            account_data,
            account_repository,
            #[cfg(test)]
            _test_account_root: None,
            settings,
            live_char_id: None,
            live_stat_raw: std::collections::HashMap::new(),
            pending_close_calls_by_char: std::collections::HashMap::new(),
            pending_close_call_totals: std::collections::HashMap::new(),
            pending_stat_awards: std::collections::HashMap::new(),
            last_dungeon_alert: None,
            pet_object_buffer: std::collections::HashMap::new(),
            enriched_pet_object_id: None,
            char_view_gen: 0,
            vault_view_gen: 0,
            seq: 0,
            updates: Vec::new(),
            recorder: None,
            realm_entered_at: None,
            keyper_towers_announced: false,
            keyper_realm: None,
            pending_events: Vec::new(),
            monument_poppers: [None, None, None],
            mission_tracker: MissionTracker::new(),
            selected_scope: None,
            pending_credential: None,
            last_account_gen: 0,
            last_mission_gen: 0,
        })
    }

    /// Test-only constructor: in-memory loot tracker, default settings, no disk
    /// or DB side effects. Takes ownership of a pre-populated `account_data`.
    #[cfg(test)]
    fn new_for_test(account_data: AccountData) -> Self {
        let test_account_root = tempfile::tempdir().expect("create account-data test root");
        let account_repository =
            AccountDataRepository::for_profile(test_account_root.path().join("account_data.json"));
        Self {
            reassembler: TcpReassembler::new(),
            session: GameSession::new(),
            router: PacketRouter::new(),
            loot_tracker: LootTracker::new(),
            combat: CombatManager::new(),
            account_data,
            account_repository,
            _test_account_root: Some(test_account_root),
            settings: Arc::new(RwLock::new(Settings::default())),
            live_char_id: None,
            live_stat_raw: std::collections::HashMap::new(),
            pending_close_calls_by_char: std::collections::HashMap::new(),
            pending_close_call_totals: std::collections::HashMap::new(),
            pending_stat_awards: std::collections::HashMap::new(),
            last_dungeon_alert: None,
            pet_object_buffer: std::collections::HashMap::new(),
            enriched_pet_object_id: None,
            char_view_gen: 0,
            vault_view_gen: 0,
            seq: 0,
            updates: Vec::new(),
            recorder: None,
            realm_entered_at: None,
            keyper_towers_announced: false,
            keyper_realm: None,
            pending_events: Vec::new(),
            monument_poppers: [None, None, None],
            mission_tracker: MissionTracker::new(),
            selected_scope: None,
            pending_credential: None,
            last_account_gen: 0,
            last_mission_gen: 0,
        }
    }

    // -----------------------------------------------------------------------

    pub fn account_data(&self) -> &AccountData {
        &self.account_data
    }

    /// Build a `LiveVaultData` view from the canonical account vaults.
    ///
    /// The vault chests live inside [`AccountData`] (single source of truth); the
    /// UI panels still consume a `LiveVaultData`, so this materialises one on
    /// demand. Called only when the vault generation bumps, so the clone is rare.
    pub fn live_vault_data(&self) -> LiveVaultData {
        LiveVaultData {
            regular: self.account_data.regular_vault.clone(),
            seasonal: self.account_data.seasonal_vault.clone(),
        }
    }

    pub fn live_char_id(&self) -> Option<i32> {
        self.live_char_id
    }

    /// The socket of the verified main game-client connection, if one is
    /// currently tracked. Used by the UI to map the connection to its owning
    /// OS process (forge-fire relaunch safeguard). Only returned once the
    /// account is *verified* as the main, so a tentatively-accepted mule/alt
    /// Hello (cleared later on account mismatch) can never unlock the guard.
    pub fn main_connection(&self) -> Option<ConnectionKey> {
        if self.session.connection.is_account_verified() {
            self.session.connection.main_connection
        } else {
            None
        }
    }

    /// Build the latest coalesced view snapshot.
    pub fn view_state(&self) -> ViewState {
        let metrics = self.reassembler.metrics();
        ViewState {
            detected_account_name: self.session.connection.detected_account_name.clone(),
            saved_account_id: self.session.connection.saved_account_id.clone(),
            main_account_status: self.session.connection.main_account_status,
            account_verified: self.session.connection.is_account_verified(),
            ignored_count: self.reassembler.ignored_count(),
            char_view_gen: self.char_view_gen,
            vault_view_gen: self.vault_view_gen,
            account_generation: self.account_data.generation(),
            capture_health: metrics.health(),
            capture_gap_skips: metrics.gap_skips,
            capture_bytes_lost: metrics.gap_bytes_lost,
            capture_resyncs: metrics.resyncs,
            capture_packets_dropped: metrics.packets_dropped_unsynced,
            capture_unsync_ms: metrics.total_unsync_duration_ms(),
            capture_queue_drops: metrics.queue_drops,
            mission_view_gen: self.mission_tracker.generation(),
            loot_boost_secs: self.session.player.loot_boost_secs,
        }
    }

    /// Build the current seasonal mission tracklist snapshot for the UI.
    pub fn mission_view(&self) -> MissionView {
        self.mission_tracker.build_view()
    }

    /// Advance elapsed mission cooldowns before publishing or applying packets.
    pub fn expire_mission_cooldowns(&mut self) -> bool {
        self.mission_tracker.expire_cooldowns(now_ms() / 1000)
    }

    /// Bind this worker to its selected account. Async completions are validated
    /// against this scope; called once at worker spawn before the loop starts.
    pub fn set_selected_scope(&mut self, account_key: AccountKey, account_id: AccountId) {
        self.selected_scope = Some(SelectedScope {
            account_key,
            account_id,
        });
    }

    /// Whether an async completion's scope names the currently selected account.
    /// With no bound scope (non-selected / legacy callers) identity is enforced
    /// elsewhere, so this permits the completion.
    fn scope_matches_selected(&self, scope: &AccountOperationScope) -> bool {
        match &self.selected_scope {
            Some(sel) => {
                scope.account_key == sel.account_key && scope.expected_account_id == sel.account_id
            }
            None => true,
        }
    }

    /// Release a token captured from a now-verified connection, emitting it to
    /// the UI so refreshes may use it. Only releases when the token is still
    /// bound to the current main connection; no-op otherwise.
    fn release_pending_credential(&mut self) {
        let bound_to_main = matches!(
            (&self.pending_credential, self.session.connection.main_connection),
            (Some(cred), Some(main)) if cred.connection == main
        );
        if bound_to_main {
            if let Some(cred) = self.pending_credential.take() {
                self.emit(UiPayload::AccessTokenCaptured(cred.token));
            }
        }
    }

    /// Discard a tentative candidate that is unverified (not positively foreign):
    /// drop its buffered events and captured token and free the main slot so the
    /// next HELLO can retry. Unlike a mismatch this does not ignore the socket,
    /// so a slow-but-legitimate main can still recover on its next connection.
    fn discard_unverified_candidate(&mut self) {
        self.pending_events.clear();
        self.pending_credential = None;
        self.session.on_account_mismatch();
    }

    /// Discard the tentative candidate if it has stayed unverified past the
    /// identity timeout. Returns whether a candidate was discarded (so the caller
    /// can publish the resulting state change). Verified mains never time out.
    pub fn tick_identity(&mut self) -> bool {
        self.check_identity_timeout(now_ms())
    }

    fn check_identity_timeout(&mut self, now_ms: i64) -> bool {
        if self.session.connection.is_account_verified() {
            return false;
        }
        if let Some(since) = self.session.connection.candidate_since_ms {
            if now_ms.saturating_sub(since) >= IDENTITY_TIMEOUT_MS {
                tracing::info!("[ACCOUNT] Identity timeout; discarding unverified candidate.");
                self.discard_unverified_candidate();
                return true;
            }
        }
        false
    }

    // -----------------------------------------------------------------------
    // Lifecycle / control
    // -----------------------------------------------------------------------

    /// Reset session + reassembler state for a fresh capture.
    pub fn on_capture_start(&mut self) {
        self.session.reset_for_capture();
        self.reassembler.clear();
        self.start_recorder_if_enabled();
    }

    /// Start a raw-packet recorder when `REALMHOUND_RECORD` is set. The value may
    /// be a file path; otherwise a timestamped file under the captures dir is
    /// used. Spike/dev tool for the Combat History feature.
    fn start_recorder_if_enabled(&mut self) {
        let Some(val) = std::env::var_os("REALMHOUND_RECORD") else {
            return;
        };
        if val.is_empty() || val == "0" {
            return;
        }
        let path = if val == "1" {
            let name = format!(
                "capture_{}.rhcap",
                chrono::Utc::now().format("%Y-%m-%d_%H-%M-%S")
            );
            realmhound_core::capture::captures_dir().join(name)
        } else {
            let p = std::path::PathBuf::from(&val);
            if p.is_absolute() {
                p
            } else {
                realmhound_core::capture::captures_dir().join(p)
            }
        };
        match CaptureWriter::create(&path) {
            Ok(w) => {
                tracing::info!("[RECORD] Recording raw packets to {}", path.display());
                self.recorder = Some(w);
            }
            Err(e) => {
                tracing::warn!(
                    "[RECORD] Failed to open capture file {}: {}",
                    path.display(),
                    e
                );
            }
        }
    }

    /// Update capture-queue drop count (called by the worker each batch).
    pub fn update_queue_drops(&mut self, queue_drops: u64) {
        let metrics = self.reassembler.metrics_mut();
        if queue_drops > metrics.queue_drops {
            metrics.last_degraded_event = Some(std::time::Instant::now());
        }
        metrics.queue_drops = queue_drops;
    }

    /// Apply a UI-origin control message.
    pub fn apply_control(&mut self, msg: ControlMsg) {
        match msg {
            ControlMsg::ClearIgnored => {
                self.reassembler.clear_ignored();
            }
            ControlMsg::SetLootTrackingSettings(settings) => {
                self.loot_tracker.set_tracking_settings(&settings);
            }
            ControlMsg::SetCombatHistorySettings(settings) => {
                self.combat.set_history_settings(settings);
            }
            ControlMsg::SetCharacterLabel { char_id, label } => {
                self.account_data.characters.set_label(char_id, label);
                // Bump account generation (not just dirty) so the Treasury panel,
                // which resyncs its character cache on account_generation, refreshes.
                self.account_data.bump_generation();
                self.char_view_gen += 1;
            }
            ControlMsg::RemoveCharacter(char_id) => {
                self.account_data.characters.remove_character(char_id);
                self.account_data.bump_generation();
                self.char_view_gen += 1;
            }
            ControlMsg::MoveCharacter {
                char_id,
                to_index,
                seasonal,
            } => {
                self.account_data
                    .characters
                    .move_character(char_id, to_index, seasonal);
                self.account_data.bump_generation();
                self.char_view_gen += 1;
            }
            ControlMsg::ApplyApiAccountData(data, scope) => {
                // Reject stale or foreign-account completions before touching any
                // account-scoped state; only a strictly newer generation for the
                // selected account may apply.
                if !self.scope_matches_selected(&scope) || scope.generation <= self.last_account_gen
                {
                    tracing::warn!("[ACCOUNT] Rejected stale/non-selected ApplyApiAccountData.");
                    return;
                }
                self.last_account_gen = scope.generation;
                // Reject the whole payload if the character list belongs to a
                // different account (mule/alt); otherwise exaltations, vault and
                // account-wide stats would be overwritten with the wrong data.
                if !self
                    .account_data
                    .characters
                    .update_from_api(&data.characters, data.account_id.as_deref())
                {
                    tracing::warn!(
                        "[ACCOUNT] Rejected ApplyApiAccountData from a different account; \
                         exaltations and account stats left unchanged."
                    );
                    return;
                }
                self.account_data.characters.exaltation_stats = data.exaltation_stats;
                self.account_data.characters.account_accelerators = data.accelerators;
                self.account_data.max_num_chars = data.max_num_chars;
                self.account_data.next_char_slot_price = data.next_char_slot_price;
                self.account_data.owned_skins_count = data.owned_skins_count;
                if data.account_credits.is_some()
                    || data.account_fame.is_some()
                    || data.account_star.is_some()
                {
                    let update = realmhound_core::account_stats::AccountStatsUpdate {
                        stars: data.account_star,
                        account_fame: data.account_fame,
                        gold: data.account_credits,
                        ..Default::default()
                    };
                    self.account_data.update_account_stats(&update);
                }
                self.account_data.bump_generation();
                self.char_view_gen += 1;
            }
            ControlMsg::ApplyMissionData {
                defs,
                progress,
                scope,
            } => {
                // Reject stale or foreign-account completions before binding the
                // tracklist to the current main account.
                if !self.scope_matches_selected(&scope) || scope.generation <= self.last_mission_gen
                {
                    tracing::warn!("[MISSIONS] Rejected stale/non-selected ApplyMissionData.");
                    return;
                }
                self.last_mission_gen = scope.generation;
                // Bind to the current main account first so a stale/mule fetch
                // can't populate the main's tracklist.
                self.mission_tracker
                    .set_account(self.session.connection.detected_account_name.clone());
                if let Some(defs) = defs {
                    self.mission_tracker.apply_definitions(defs);
                }
                if let Some(progress) = progress {
                    self.mission_tracker.apply_player_missions(progress);
                }
            }
            // Capture-lifecycle messages are handled by the worker loop, not the
            // processor; ignore them here for exhaustiveness.
            ControlMsg::StartCapture(_) | ControlMsg::StopCapture | ControlMsg::Shutdown => {}
        }
    }

    /// Periodic save: persists `account_data` if dirty. Returns whether a save
    /// was attempted (so the caller can reset its timer).
    pub fn save_account_if_dirty(&mut self) -> bool {
        if self.account_data.is_dirty() {
            #[cfg(feature = "latency-diagnostics")]
            let started = std::time::Instant::now();
            if let Err(e) = self
                .account_data
                .save_and_clear_dirty(&self.account_repository)
            {
                tracing::warn!("[ACCOUNT] Periodic save failed: {}", e);
            }
            #[cfg(feature = "latency-diagnostics")]
            self.log_account_storage_latency("account_periodic_save", started.elapsed());
            true
        } else {
            false
        }
    }

    /// Save `account_data` on exit, surfacing the error so a relaunch can abort.
    pub fn save_account_on_exit(&mut self) -> std::io::Result<()> {
        #[cfg(feature = "latency-diagnostics")]
        let started = std::time::Instant::now();
        match self.account_data.save(&self.account_repository) {
            Ok(()) => {
                #[cfg(feature = "latency-diagnostics")]
                self.log_account_storage_latency("account_exit_save", started.elapsed());
                tracing::info!("[ACCOUNT] Saved account data on exit");
                Ok(())
            }
            Err(e) => {
                #[cfg(feature = "latency-diagnostics")]
                self.log_account_storage_latency("account_exit_save", started.elapsed());
                tracing::warn!("[ACCOUNT] Failed to save account data on exit: {}", e);
                Err(e)
            }
        }
    }

    #[cfg(feature = "latency-diagnostics")]
    fn log_account_storage_latency(&self, operation: &str, duration: std::time::Duration) {
        if duration >= std::time::Duration::from_millis(100) {
            tracing::warn!(
                "[LATENCY][STORAGE] operation={} duration_ms={} character_count={} \
                 vault_item_count={}",
                operation,
                duration.as_millis(),
                self.account_data.characters.characters.len(),
                self.account_data.regular_vault.vault_item_count()
                    + self.account_data.seasonal_vault.vault_item_count()
            );
        }
    }

    /// Fold deferred combat tallies onto the character cache: this-tick close
    /// calls, close-call backfill from persisted cards, and lifetime stat awards.
    fn fold_deferred_combat(&mut self) {
        // Buffer per character and retry so a close call is never lost while the
        // character is momentarily absent from the cache (nexus / reconnect).
        let close_calls = self.combat.take_pending_close_calls();
        if close_calls > 0 {
            if let Some(char_id) = self.live_char_id {
                *self.pending_close_calls_by_char.entry(char_id).or_insert(0) += close_calls;
            }
        }
        if !self.pending_close_calls_by_char.is_empty() {
            let mut applied = false;
            self.pending_close_calls_by_char.retain(|&char_id, count| {
                if self
                    .account_data
                    .characters
                    .add_close_calls(char_id, *count)
                {
                    applied = true;
                    false
                } else {
                    true
                }
            });
            if applied {
                self.account_data.mark_dirty();
                self.char_view_gen += 1;
            }
        }

        // Backfill from persisted fight cards (raise only); merge by max and hold
        // any whose character is not cached yet.
        for (char_id, total) in self.combat.take_pending_close_call_totals() {
            let slot = self.pending_close_call_totals.entry(char_id).or_insert(0);
            *slot = (*slot).max(total);
        }
        if !self.pending_close_call_totals.is_empty() {
            let mut changed = false;
            self.pending_close_call_totals.retain(|&char_id, total| {
                match self
                    .account_data
                    .characters
                    .reconcile_close_calls(char_id, *total)
                {
                    CloseCallSync::Raised => {
                        changed = true;
                        false
                    }
                    CloseCallSync::Unchanged => false,
                    CloseCallSync::Missing => true,
                }
            });
            if changed {
                self.account_data.mark_dirty();
                self.char_view_gen += 1;
            }
        }

        // Apply authoritative lifetime award totals (absolute, idempotent), and
        // retain characters that are temporarily absent from the cache.
        self.pending_stat_awards
            .extend(self.combat.take_pending_stat_awards());
        if !self.pending_stat_awards.is_empty() {
            let mut changed = false;
            self.pending_stat_awards
                .retain(|&char_id, (lone, last, most)| {
                    match self
                        .account_data
                        .characters
                        .set_stat_awards(char_id, *lone, *last, *most)
                    {
                        StatAwardSync::Updated => {
                            changed = true;
                            false
                        }
                        StatAwardSync::Unchanged => false,
                        StatAwardSync::Missing => true,
                    }
                });
            if changed {
                self.account_data.mark_dirty();
                self.char_view_gen += 1;
            }
        }
    }

    /// Terminal drain before shutdown: finalize in-flight fights, fold their
    /// tallies, and flush loot buffers. Finalize runs before the fold (reverse of
    /// the live-tick order) so a fight ending at shutdown still contributes.
    pub fn final_drain(&mut self) {
        let now = now_ms();
        self.combat.on_tick(now);
        self.fold_deferred_combat();

        let recent_kills: Vec<RecentBossKill> = self
            .combat
            .tracker()
            .recent_fights()
            .iter()
            .filter(|f| f.killed && f.boss_object_type > 0)
            .map(|f| RecentBossKill {
                map_seed: f.map_seed,
                object_type: f.boss_object_type,
                name: f.boss_name.clone(),
                started_at_ms: f.started_at,
                ended_at_ms: f.ended_at,
            })
            .collect();
        self.loot_tracker.set_recent_boss_kills(recent_kills);
        self.loot_tracker.flush_pending(now as u64);
    }

    /// Checkpoint and close the per-account databases, leaving no `-wal` tail.
    pub fn close_databases(&mut self) -> Result<(), DbCloseError> {
        self.combat
            .close()
            .map_err(|e| DbCloseError::Combat(e.to_string()))?;
        self.loot_tracker
            .close()
            .map_err(|e| DbCloseError::Loot(e.to_string()))?;
        Ok(())
    }

    /// Propagate the live player's observed alive fame into the character cache
    /// so the Characters panel updates as fame increases, not only after a full
    /// account refresh. No-op unless the fame actually rose, so the
    /// dirty flag and char view generation only bump on a real change.
    fn sync_live_fame(&mut self) {
        if let Some(char_id) = self.live_char_id {
            if let Some(fame) = self.loot_tracker.current_player_fame() {
                if self.account_data.characters.update_fame(char_id, fame) {
                    self.account_data.mark_dirty();
                    self.char_view_gen += 1;
                }
            }
        }
    }

    /// Merge the local player's stats from a status snapshot/delta into the raw
    /// accumulator, then derive base character stats (`base = stat - boost`) and
    /// push them into the cache so the maxed count and derived stats stay current
    /// live -- not only after an API refresh. No-op until the base HP stat has
    /// been observed (a full spawn snapshot) and the live character is known.
    fn merge_live_stats(&mut self, status: &realmhound_core::protocol::data::ObjectStatusData) {
        use realmhound_core::protocol::data::StatType;
        let Some(char_id) = self.live_char_id else {
            return;
        };
        let mut touched = false;
        for s in &status.stats {
            match StatType::from_id(s.stat_type_id) {
                StatType::MaxHP
                | StatType::MaxMP
                | StatType::Attack
                | StatType::Defense
                | StatType::Speed
                | StatType::Dexterity
                | StatType::Vitality
                | StatType::Wisdom
                | StatType::MaxHPBoost
                | StatType::MaxMPBoost
                | StatType::AttackBoost
                | StatType::DefenseBoost
                | StatType::SpeedBoost
                | StatType::DexterityBoost
                | StatType::VitalityBoost
                | StatType::WisdomBoost => {
                    self.live_stat_raw.insert(s.stat_type_id, s.stat_value);
                    touched = true;
                }
                _ => {}
            }
        }
        if !touched || !self.live_stat_raw.contains_key(&(StatType::MaxHP as u8)) {
            return;
        }
        let get = |id: StatType| self.live_stat_raw.get(&(id as u8)).copied().unwrap_or(0);
        let base = |stat: StatType, boost: StatType| (get(stat) - get(boost)).max(0);
        let stats = realmhound_core::vault::CachedStats {
            max_hp: base(StatType::MaxHP, StatType::MaxHPBoost),
            max_mp: base(StatType::MaxMP, StatType::MaxMPBoost),
            attack: base(StatType::Attack, StatType::AttackBoost),
            defense: base(StatType::Defense, StatType::DefenseBoost),
            speed: base(StatType::Speed, StatType::SpeedBoost),
            dexterity: base(StatType::Dexterity, StatType::DexterityBoost),
            vitality: base(StatType::Vitality, StatType::VitalityBoost),
            wisdom: base(StatType::Wisdom, StatType::WisdomBoost),
        };
        if self
            .account_data
            .characters
            .update_live_stats(char_id, &stats)
        {
            self.account_data.mark_dirty();
            self.char_view_gen += 1;
        }
    }

    // -----------------------------------------------------------------------
    // Pipeline
    // -----------------------------------------------------------------------

    /// Process a batch of raw captured packets, returning the ordered updates
    /// the UI should apply.
    pub fn process_available(&mut self, raw_packets: Vec<RawPacket>) -> Vec<UiUpdate> {
        realmhound_core::prof_function!();
        for raw_packet in raw_packets {
            if let Some(rec) = self.recorder.as_mut() {
                let _ = rec.write(&raw_packet);
            }
            // Parse TCP segment
            if let Some(segment) = parse_tcp_segment(&raw_packet) {
                // Track server IP from incoming packets (server port is 2050).
                // Only front-end/nexus IPs are present in the server-name table;
                // in-realm and dungeon game-world hosts use different unmapped
                // backend IPs in the same region. So an unmapped IP is almost
                // always a same-region backend host, not a different server: keep
                // the last known name rather than clearing it.
                if segment.src_port == 2050 {
                    let (seg_key, _) = ConnectionKey::from_packet(
                        segment.src_ip,
                        segment.src_port,
                        segment.dst_ip,
                        segment.dst_port,
                        2050,
                    );
                    // Only the verified main connection -- or true bootstrap on a
                    // brand-new install with no saved account yet -- may drive the
                    // toolbar server name. A returning user's server updates only
                    // once the connection verifies as the main, so a mule's server
                    // (or a segment arriving between the main's sockets) never
                    // overwrites it.
                    let is_main = self.session.connection.main_connection == Some(seg_key)
                        && self.session.connection.is_account_verified();
                    let bootstrap = self.session.connection.main_connection.is_none()
                        && self.session.connection.saved_account_id.is_none();
                    let new_ip = segment.src_ip;
                    if (is_main || bootstrap) && self.session.connection.server_ip != Some(new_ip) {
                        self.session.connection.server_ip = Some(new_ip);
                        if let Some(name) = ip_to_server_name(new_ip) {
                            tracing::info!("[SERVER] Connected to {} ({})", name, new_ip);
                            self.emit(UiPayload::SetServerName(name.to_string()));
                        } else {
                            tracing::info!("[SERVER] Unknown server IP: {} (int={}) - not in the server-name table", new_ip, u32::from_be_bytes(new_ip.octets()) as i32);
                        }
                    }
                }

                // Multi-client isolation: detect main connection disconnect
                if (segment.flags.fin || segment.flags.rst)
                    && self.session.connection.main_connection.is_some()
                {
                    let (seg_key, _) = ConnectionKey::from_packet(
                        segment.src_ip,
                        segment.src_port,
                        segment.dst_ip,
                        segment.dst_port,
                        2050,
                    );
                    if Some(seg_key) == self.session.connection.main_connection {
                        tracing::info!(
                            "[ACCOUNT] Main connection {} (FIN={}, RST={})",
                            if segment.flags.rst { "reset" } else { "closed" },
                            segment.flags.fin,
                            segment.flags.rst
                        );
                        self.session.on_disconnect();
                        self.combat.on_disconnect(now_ms());
                        self.forward_dungeon_freeze();
                        self.realm_entered_at = None;
                        self.pending_events.clear();
                        self.pending_credential = None;
                    }
                }

                // Process through reassembler
                let packets = self.reassembler.process_segment(&segment);

                for packet in packets {
                    // Parse packet for processing
                    let parsed = parse_packet_for_processing(&packet);

                    if let Some(ref parsed) = parsed {
                        // Pre-routing: Hello multi-client isolation
                        // Decision logic lives in GameSession; reassembler action stays here.
                        if let ParsedPacket::Hello(hello) = parsed {
                            let (conn_key, _) = ConnectionKey::from_packet(
                                packet.src_ip,
                                packet.src_port,
                                packet.dst_ip,
                                packet.dst_port,
                                2050,
                            );

                            match self.session.evaluate_hello(conn_key) {
                                HelloAction::Accept { needs_verification } => {
                                    tracing::info!(
                                        "[ACCOUNT] Tentative main connection: {} (verification={})",
                                        conn_key,
                                        needs_verification
                                    );
                                    // New tentative connection: drop anything
                                    // buffered for a prior connection so stale
                                    // events can't replay onto the wrong account.
                                    self.pending_events.clear();
                                    if needs_verification {
                                        self.session.connection.candidate_since_ms = Some(now_ms());
                                    }
                                    // Bind the token to this candidate; release
                                    // it only once the candidate is verified.
                                    if !hello.access_token.is_empty() {
                                        self.pending_credential = Some(PendingCredential {
                                            connection: conn_key,
                                            token: hello.access_token.clone(),
                                        });
                                    }
                                    if self.session.connection.is_account_verified() {
                                        self.release_pending_credential();
                                    }
                                }
                                HelloAction::Reject => {
                                    self.reassembler.ignore_connection(&conn_key);
                                    tracing::info!(
                                        "[ACCOUNT] Ignoring secondary connection: {}",
                                        conn_key
                                    );
                                    continue;
                                }
                                HelloAction::Reconnect => {
                                    // Same-socket re-HELLO: rebind the token and
                                    // release it only if this socket is verified.
                                    if !hello.access_token.is_empty() {
                                        self.pending_credential = Some(PendingCredential {
                                            connection: conn_key,
                                            token: hello.access_token.clone(),
                                        });
                                        if self.session.connection.is_account_verified() {
                                            self.release_pending_credential();
                                        }
                                    }
                                }
                            }
                        }

                        // --- Multi-client hard isolation (per-connection) ---
                        // A verified main lives on exactly one socket, and every
                        // global broadcast (key pops, area unlocks) arrives on
                        // that socket. Any packet from a *different* socket is a
                        // secondary client (mule/alt) -- including one that was
                        // already connected before capture began, whose Hello and
                        // CreateSuccess were never observed and whose account
                        // identity therefore can't be extracted from its Updates.
                        // The Hello-reject path can't catch such a connection
                        // until it happens to re-Hello, so drop its packets here
                        // to stop them leaking onto the main's UI.
                        let (pkt_conn, _) = ConnectionKey::from_packet(
                            packet.src_ip,
                            packet.src_port,
                            packet.dst_ip,
                            packet.dst_port,
                            2050,
                        );
                        if self.session.connection.is_foreign_connection(pkt_conn) {
                            continue;
                        }

                        // Seasonal mission packets aren't routed into
                        // GameEvents; feed them straight into the tracker. Only
                        // applied on a verified main so an unverified/tentative
                        // candidate (possibly a mule) can't mutate the main's
                        // account-scoped mission state before it identifies itself.
                        if self.session.connection.is_account_verified() {
                            match parsed {
                                ParsedPacket::Unknown165(p) => {
                                    let entries =
                                        realmhound_core::api::parse_prog_string(&p.unknown_string);
                                    for entry in entries {
                                        self.mission_tracker.apply_prog(&entry);
                                    }
                                }
                                ParsedPacket::ClaimMission(p) => {
                                    self.mission_tracker.record_claim_request(
                                        p.request_id,
                                        p.season_id,
                                        p.mission_positional_idx,
                                    );
                                }
                                ParsedPacket::Unknown164(p) => {
                                    self.mission_tracker
                                        .confirm_claim(p.unknown_byte1, p.unknown_byte2 != 0);
                                }
                                _ => {}
                            }
                        }

                        // Route through PacketRouter (all packet types)
                        match self.router.route(parsed, &mut self.session) {
                            RouteResult::Routed(events) => {
                                for event in events {
                                    self.dispatch_event(event);
                                }
                            }
                            RouteResult::Unmapped => {
                                // All known packet types are now routed.
                                // This branch only fires for truly unknown packets.
                            }
                        }
                    }
                }
            }
        }

        if let Some(rec) = self.recorder.as_mut() {
            let _ = rec.flush();
        }

        // Keep mission tracking bound to the current main account so a mule's
        // packets (already filtered above) or an account switch never bleed in.
        self.mission_tracker
            .set_account(self.session.connection.detected_account_name.clone());

        std::mem::take(&mut self.updates)
    }

    /// Emit a `UiUpdate` with the next sequence number.
    fn emit(&mut self, payload: UiPayload) {
        self.seq += 1;
        self.updates.push(UiUpdate {
            seq: self.seq,
            #[cfg(feature = "latency-diagnostics")]
            enqueued_at: None,
            payload,
        });
    }

    /// Emit the combined Lost Halls Rune-monument entry from the accumulated
    /// per-monument poppers, if at least one monument was activated by a real
    /// player (all-auto runs are silent and produce nothing). Ordered
    /// Sword, Shield, Helmet; the clipboard thanks each unique popper.
    fn flush_monument_entry(&mut self) {
        let show = self
            .settings
            .read()
            .map(|s| s.live_feed.show_area_unlocks)
            .unwrap_or(false);
        if !show {
            return;
        }
        let mgr = realmhound_core::assets::get_asset_manager();
        let mut contributions: Vec<(i32, String)> = Vec::new();
        // (rune type lowercased, popper) for each monument popped by a non-local player.
        let mut eligible: Vec<(String, String)> = Vec::new();
        for (idx, (rune_key, item_name)) in MONUMENTS.iter().enumerate() {
            if let Some(popper) = self.monument_poppers[idx].clone() {
                let item_id = mgr.object_id_for_name(item_name).unwrap_or(0);
                contributions.push((item_id, popper.clone()));
                if !self.is_local_player(&popper) {
                    eligible.push((rune_key.to_lowercase(), popper));
                }
            }
        }
        if contributions.is_empty() {
            return;
        }
        let callout = Self::monument_callout(&eligible);
        self.emit(UiPayload::AreaUnlock {
            title: "Sanctuary runes".to_string(),
            contributions,
            callout,
        });
    }

    /// Case-insensitive match of a name against the local player's detected name.
    fn is_local_player(&self, name: &str) -> bool {
        self.session
            .connection
            .detected_account_name
            .as_deref()
            .map(|me| me.eq_ignore_ascii_case(name.trim()))
            .unwrap_or(false)
    }

    /// Build the Sanctuary runes thanks callout from the non-local contributors:
    /// empty when only the local player popped, singular
    /// "Thanks <Name> for the <type> rune!" for a lone rune from one player, else
    /// "Thanks <Names> for the runes!".
    fn monument_callout(eligible: &[(String, String)]) -> String {
        if eligible.is_empty() {
            return String::new();
        }
        let mut unique: Vec<&str> = Vec::new();
        for (_, name) in eligible {
            if !unique.iter().any(|n| n.eq_ignore_ascii_case(name)) {
                unique.push(name);
            }
        }
        if unique.len() == 1 {
            if eligible.len() == 1 {
                return format!("Thanks {} for the {} rune!", eligible[0].1, eligible[0].0);
            }
            return format!("Thanks {} for the runes!", unique[0]);
        }
        format!("Thanks {} for the runes!", unique.join(", "))
    }

    /// Build the enchantment-notification context for a batch of loot drops:
    /// the current settings snapshot plus the enchant catalog, resolved once so
    /// a burst of drops doesn't re-clone the catalog per drop. Returns `None`
    /// when no category is enabled.
    fn enchant_sound_context(
        &self,
    ) -> Option<(
        realmhound_core::settings::EnchantmentSounds,
        Vec<realmhound_core::assets::EnchantCatalogEntry>,
    )> {
        let settings = self.settings.read().ok()?.sound.enchantments.clone();
        if !crate::enchant_sound::any_enabled(&settings) {
            return None;
        }
        let catalog = realmhound_core::assets::get_asset_manager().enchant_catalog();
        Some((settings, catalog))
    }

    /// Play enchantment notification sounds for a new loot drop, if any
    /// of its item enchants match a configured tiered / unique-awakened entry.
    fn play_enchant_sounds(
        &mut self,
        drop: &realmhound_core::loot::ProcessedLootDrop,
        settings: &realmhound_core::settings::EnchantmentSounds,
        catalog: &[realmhound_core::assets::EnchantCatalogEntry],
    ) {
        let ids: Vec<i32> = drop
            .items
            .iter()
            .flat_map(|it| it.enchant_ids.iter().copied())
            .collect();
        if ids.is_empty() {
            return;
        }
        for trigger in crate::enchant_sound::resolve(settings, &ids, catalog) {
            self.emit(UiPayload::Audio(AudioCommand::PlayEvent {
                sound: crate::enchant_sound::ENCHANT_SOUND,
                custom_key: trigger.custom_key,
                volume: trigger.volume,
            }));
        }
    }

    /// Apply a live active-pet change to the character cache and vault panel.
    ///
    /// Remaps the current character's pet instance id so live pet-inventory
    /// updates land on the newly equipped pet without waiting for an API
    /// refresh. Only applied on the verified main account to avoid a
    /// mule/alt connection persisting pet state.
    fn apply_active_pet(&mut self, instance_id: i32) {
        let verified = self.session.connection.is_account_verified();
        if instance_id <= 0 || !verified {
            return;
        }
        let Some(char_id) = self.session.player.char_id else {
            return;
        };
        let changed = self
            .account_data
            .characters
            .set_active_pet(char_id, instance_id);
        if changed {
            self.account_data.bump_generation();
            self.char_view_gen += 1;
            self.emit(UiPayload::SetVaultActivePet {
                char_id,
                instance_id,
            });
            // The instance mapping just changed, so re-apply the detected pet's
            // buffered spawn status onto the freshly mapped instance: a pet's
            // full inventory + identity ride only in its spawn `Update`, which
            // may be processed before this ActivePetUpdate established the
            // mapping (pets equipped in a prior session emit no live deltas).
            self.enriched_pet_object_id = None;
            self.enrich_detected_pet();
        }
    }

    /// Once a pet is detected (its world object id is known), replay the full
    /// spawn status buffered for that object so its inventory + identity land
    /// even though detection latched a tick after the spawn `Update` that
    /// carried them. Applied at most once per detected pet.
    fn enrich_detected_pet(&mut self) {
        let Some(pet_id) = self.session.player.pet_object_id else {
            self.enriched_pet_object_id = None;
            return;
        };
        if self.enriched_pet_object_id == Some(pet_id) {
            return;
        }
        if let Some(status) = self.pet_object_buffer.get(&pet_id).cloned() {
            self.apply_pet_object(&status);
            self.enriched_pet_object_id = Some(pet_id);
        }
    }

    /// Apply a pet object's stats (identity + inventory) to the character cache
    /// and vault panel. Works for both the initial `Update` (which carries the
    /// freshly equipped pet's full identity + inventory) and later `NewTick`
    /// deltas. The Treasury and Vault > Materials views render from
    /// `account_data.characters`, so updates must land there; the vault panel
    /// keeps a separate API copy fed via `UpdateVaultPetSlot`.
    fn apply_pet_object(&mut self, status: &realmhound_core::protocol::data::ObjectStatusData) {
        let updates = extract_pet_inventory_updates(status);
        let identity = extract_pet_identity(status);
        if updates.is_empty() && identity.is_empty() {
            return;
        }
        let Some(char_id) = self.session.player.char_id else {
            return;
        };
        let Some((pet_instance_id, seasonal)) = self.account_data.characters.pet_info(char_id)
        else {
            return;
        };
        let mut changed = false;
        // Enrich identity first so the character card can render the newly
        // equipped pet's sprite/name before an API refresh.
        if self
            .account_data
            .characters
            .apply_pet_identity(pet_instance_id, seasonal, &identity)
        {
            changed = true;
        }
        for u in &updates {
            self.emit(UiPayload::UpdateVaultPetSlot {
                pet_instance_id,
                seasonal,
                slot: u.slot,
                item_id: u.item_id,
                stack_count: u.stack_count,
            });
            if self.account_data.characters.update_pet_inventory_slot(
                pet_instance_id,
                seasonal,
                u.slot,
                u.item_id,
                u.stack_count,
            ) {
                changed = true;
            }
        }
        if changed {
            self.account_data.bump_generation();
            self.char_view_gen += 1;
        }
    }

    /// Dispatch a domain event: re-broadcast to the session-independent panels,
    /// then apply model-side effects and emit derived updates.
    /// Emit the Live Feed / sound side effects for a decoded area-unlock
    /// broadcast. Single pops surface immediately; monument activations are
    /// accumulated (last human popper wins) and surfaced together on Oryx's
    /// Sanctuary entry. Shared by the text and ServerMessage-notification paths.
    fn apply_area_unlock(&mut self, msg: AreaUnlockMsg) {
        match msg {
            AreaUnlockMsg::Single {
                item_name,
                title,
                short,
                popper,
            } if !popper.is_empty() => {
                let show = self
                    .settings
                    .read()
                    .map(|s| s.live_feed.show_area_unlocks)
                    .unwrap_or(false);
                if show {
                    let item_id = realmhound_core::assets::get_asset_manager()
                        .object_id_for_name(item_name)
                        .unwrap_or(0);
                    let callout = if self.is_local_player(&popper) {
                        String::new()
                    } else {
                        format!("Thanks {} for the {}!", popper, short)
                    };
                    self.emit(UiPayload::AreaUnlock {
                        title: title.to_string(),
                        contributions: vec![(item_id, popper)],
                        callout,
                    });
                }
            }
            // Monument activation: remember the last human popper for this
            // monument. Surfaces later as a combined Oryx's Sanctuary entry.
            AreaUnlockMsg::Monument { index, popper } if !popper.is_empty() => {
                self.monument_poppers[index] = Some(popper);
            }
            _ => {}
        }
    }

    /// Runs multi-client account verification for an `Update` packet and applies
    /// all identity side effects (ignore the connection + reset session + clear
    /// buffered location + wipe party on mismatch; persist settings on first
    /// run; commit the display name only once confirmed as the main). Returns
    /// the verdict so callers can gate downstream processing. Shared by the
    /// gated (tentative-connection) path and the normal `UpdateReceived` arm.
    fn run_account_verification(
        &mut self,
        update: &realmhound_core::protocol::packets::UpdatePacket,
    ) -> AccountVerifyResult {
        let Some(player_id) = self.session.player.object_id else {
            return AccountVerifyResult::NoAction;
        };
        let Some(obj) = update.find_object(player_id) else {
            return AccountVerifyResult::NoAction;
        };
        let identity = extract_account_identity(&obj.status);
        if let Some(name) = identity.account_name.as_ref() {
            tracing::info!("[ACCOUNT] Detected player name: {}", name);
        }
        let verify = self.session.verify_account_identity(
            identity.account_id.as_deref(),
            identity.account_name.as_deref(),
        );
        match &verify {
            AccountVerifyResult::Mismatch {
                detected_id,
                saved_id,
            } => {
                tracing::warn!(
                    "[ACCOUNT] Account mismatch! detected={}, saved={}. Ignoring connection.",
                    detected_id,
                    saved_id
                );
                if let Some(conn_key) = self.session.connection.main_connection.take() {
                    self.reassembler.ignore_connection(&conn_key);
                }
                self.session.on_account_mismatch();
                // A mule may have buffered events while tentative; drop them so
                // none can replay onto the main's UI.
                self.pending_events.clear();
                // Drop any token captured from this foreign candidate so it can
                // never be committed as the selected account's credential.
                self.pending_credential = None;
                // A rejected alt/mule may have tentatively populated the party
                // panel before its identity was known; wipe it so a mule's party
                // never lingers on the main's UI.
                self.emit(UiPayload::PartyCleared);
            }
            AccountVerifyResult::Verified => {
                tracing::info!(
                    "[ACCOUNT] Account verified: {}",
                    identity.account_id.as_deref().unwrap_or("?")
                );
                self.release_pending_credential();
            }
            AccountVerifyResult::FirstRun { account_id, .. } => {
                // The selected profile is the authoritative identity, so a
                // first-run capture never rebinds `settings.account`; it is
                // migration/compat input only. The session already carries the
                // selected id, so this branch is effectively unreachable at
                // runtime and simply logs.
                tracing::info!("[ACCOUNT] First-run verification for {account_id}");
                self.release_pending_credential();
            }
            AccountVerifyResult::NoAction => {}
        }
        verify
    }

    fn dispatch_event(&mut self, event: GameEvent) {
        // --- Multi-client isolation gate ---
        // Once a main account is known, a freshly-accepted connection is
        // tentative until its `Update` verifies it as that main. RotMG opens a
        // new socket per map change, so the real main re-enters this tentative
        // window on every area switch (and a mule is repeatedly re-accepted
        // tentatively too). While tentative we must not leak events to the UI or
        // model, but we still run account verification (to accept or reject the
        // connection) and buffer everything else. When the connection verifies
        // as the main we replay the whole buffer in order -- so none of the
        // main's own loot/combat/location/objects are lost -- then process the
        // verifying `Update`. If it turns out to be a mule, the buffer is
        // discarded and nothing ever reaches the UI.
        let gated = self.session.connection.saved_account_id.is_some()
            && !self.session.connection.is_account_verified();
        if gated {
            let verify = match &event {
                GameEvent::UpdateReceived(update, _) => Some(self.run_account_verification(update)),
                _ => None,
            };
            match verify {
                Some(AccountVerifyResult::Verified)
                | Some(AccountVerifyResult::FirstRun { .. }) => {
                    // Confirmed as the main: flush the buffer in arrival order
                    // through the (now ungated) dispatch, then process this
                    // verifying Update so its objects/loot/combat aren't lost.
                    let buffered = std::mem::take(&mut self.pending_events);
                    for ev in buffered {
                        self.dispatch_event(ev);
                    }
                    self.dispatch_event(event);
                }
                Some(AccountVerifyResult::Mismatch { .. }) => {
                    // Rejected mule: the helper already cleared the buffer.
                }
                _ => {
                    // A non-Update event, or an Update before the player is yet
                    // identifiable (NoAction): keep buffering until we know who
                    // this connection belongs to. If the buffer fills before the
                    // candidate identifies itself, discard the whole candidate
                    // (never a partial replay) and free the slot for a retry.
                    if self.pending_events.len() < PENDING_EVENT_CAP {
                        self.pending_events.push(event);
                    } else {
                        tracing::warn!(
                            "[ACCOUNT] Candidate buffer overflow; discarding unverified candidate."
                        );
                        self.discard_unverified_candidate();
                    }
                }
            }
            return;
        }

        // --- Phase 1: re-broadcast to transient panels ---
        // The App applies this to quest / party / live_feed / vault via their
        // existing `handle_event`. Chat and characters are resolved here.
        self.emit(UiPayload::Broadcast(event.clone()));

        // --- Phase 2: model-side orchestration ---
        match event {
            GameEvent::QuestsReceived(_)
            | GameEvent::RealmScoreChanged { .. }
            | GameEvent::PartyMemberJoined(_)
            | GameEvent::PartyJoinRequestResponse(_)
            | GameEvent::PartyActionReceived { .. }
            | GameEvent::PortalUsed { .. }
            | GameEvent::QuestRedeemAttempted { .. }
            | GameEvent::QuestRedeemResult { .. }
            | GameEvent::ReconnectReceived { .. } => {}

            GameEvent::ActivePetChanged { instance_id } => {
                self.apply_active_pet(instance_id);
            }
            GameEvent::PartyListReceived(ref party_info) => {
                let local_name = self.session.connection.detected_account_name.clone();
                let leader_name = party_info
                    .party_players
                    .iter()
                    .find(|p| p.id == party_info.leader_id)
                    .map(|p| p.name.as_str());
                let is_leader = match (local_name.as_deref(), leader_name) {
                    (Some(me), Some(leader)) => me.eq_ignore_ascii_case(leader),
                    _ => false,
                };
                self.emit(UiPayload::PartyLocalLeaderStatus(is_leader));
                self.emit(UiPayload::PartyLocalPlayerName(local_name));
            }

            // Character cache events (previously handled by CharactersPanel).
            GameEvent::CharacterDied {
                char_id,
                ref killed_by,
                total_fame,
                gravestone_type,
            } => {
                self.account_data.characters.mark_dead(
                    char_id,
                    killed_by,
                    total_fame,
                    Some(gravestone_type),
                );
                // Feed the local death to combat so the finalizing fight can mark
                // the local participant as dead with the right gravestone sprite.
                self.combat
                    .on_local_death(char_id, gravestone_type, now_ms());
                if self.live_char_id == Some(char_id) {
                    self.live_char_id = None;
                    self.live_stat_raw.clear();
                }
                self.account_data.mark_dirty();
                self.char_view_gen += 1;
            }
            GameEvent::CharacterXmlReceived { ref xml } => {
                // Only ingest the account character list once the connection is
                // confirmed to be the saved main account, so mule/alt accounts
                // never populate the character pages.
                if self.session.connection.is_account_verified() {
                    match realmhound_core::api::parse_char_list(xml) {
                        Ok(characters) => {
                            tracing::info!(
                                "[CHARACTERS] Parsed {} characters from packet",
                                characters.len()
                            );
                            for character in &characters {
                                self.account_data
                                    .characters
                                    .update_single_character(character);
                            }
                            self.account_data.mark_dirty();
                            self.char_view_gen += 1;
                        }
                        Err(e) => {
                            tracing::error!("[CHARACTERS] Failed to parse character XML: {}", e);
                        }
                    }
                }
            }
            GameEvent::SeasonalStatusReceived {
                char_id,
                is_seasonal,
            } => {
                self.account_data
                    .characters
                    .update_seasonal_status(char_id, is_seasonal);
                self.account_data.mark_dirty();
                self.char_view_gen += 1;
            }
            GameEvent::CrucibleStatusReceived { char_id, is_active } => {
                self.account_data
                    .characters
                    .update_crucible_status(char_id, is_active);
                self.account_data.mark_dirty();
                self.char_view_gen += 1;
            }
            GameEvent::AccountAcceleratorActivated { object_type } => {
                let now_unix = chrono::Utc::now().timestamp();
                if self
                    .account_data
                    .characters
                    .activate_estimated_accelerator(object_type, now_unix)
                {
                    self.account_data.mark_dirty();
                    self.char_view_gen += 1;
                }
            }
            GameEvent::CrucibleDefinitionsReceived(ref defs) => {
                if self.account_data.characters.set_crucible_defs(defs.clone()) {
                    self.account_data.mark_dirty();
                    self.char_view_gen += 1;
                }
            }
            GameEvent::ExaltationUpdated {
                class_type,
                ref exaltation,
            } => {
                // Gated on a verified main account so a mule/alt loading an
                // unexalted character into nexus can't overwrite the main's
                // exaltation progress.
                if self.session.connection.is_account_verified() {
                    let changed = self
                        .account_data
                        .characters
                        .exaltation_stats
                        .get(&class_type)
                        .map(|existing| existing != exaltation)
                        .unwrap_or(true);
                    if changed {
                        self.account_data
                            .characters
                            .exaltation_stats
                            .insert(class_type, exaltation.clone());
                        self.account_data.mark_dirty();
                        self.char_view_gen += 1;
                    }
                }
            }

            // Dust updates: persist to account data (bumps generation + saves).
            GameEvent::DustUpdated {
                ref amounts,
                is_seasonal,
            } => {
                self.account_data.update_dust(
                    amounts.clone(),
                    is_seasonal,
                    &self.account_repository,
                );
            }

            // Account-level Widget Bar stats (stars, fame, gold, forge fire, materials).
            GameEvent::AccountStatsUpdated(ref update) => {
                self.account_data.update_account_stats(update);
            }

            GameEvent::EntityHit { target_id } => {
                self.loot_tracker.on_entity_hit(target_id);
            }
            GameEvent::LocalPlayerHit {
                target_id,
                bullet_id,
                shooter_id,
                main_id,
            } => {
                self.combat
                    .on_local_hit(target_id, bullet_id, shooter_id, main_id, now_ms());
            }
            GameEvent::LocalPlayerShot {
                bullet_id,
                weapon_id,
                projectile_id,
                client_time,
                shot_x,
                shot_y,
                angle,
            } => {
                self.combat.on_local_shot(
                    weapon_id,
                    projectile_id,
                    bullet_id,
                    client_time,
                    shot_x,
                    shot_y,
                    angle,
                    now_ms(),
                );
            }
            GameEvent::AllyOrSummonShot {
                owner_id,
                summoner_id,
                bullet_id,
                bullet_count,
                damage,
                container_type,
                bullet_type,
            } => {
                self.combat.on_ally_shot(
                    owner_id,
                    summoner_id,
                    bullet_id,
                    bullet_count,
                    damage,
                    container_type,
                    bullet_type,
                );
            }
            GameEvent::EnemyShot {
                owner_id,
                bullet_id,
                bullet_type,
                damage,
                num_shots,
            } => {
                self.combat
                    .on_enemy_shot(owner_id, bullet_id, bullet_type, damage, num_shots);
            }
            GameEvent::LocalPlayerWasHit {
                bullet_id,
                enemy_id,
            } => {
                self.combat.on_player_hit(bullet_id, enemy_id, now_ms());
            }
            GameEvent::LocalItemUsed { item_id, x, y } => {
                self.combat.on_use_item(item_id, x, y, now_ms());
            }
            GameEvent::DamageDealt {
                target_id,
                attacker_id,
                amount,
            } => {
                self.combat
                    .on_damage(target_id, attacker_id, amount as i64, now_ms());
            }
            GameEvent::PlayerLoaded {
                char_id,
                object_id,
                ref new_character,
                ref pc_stats,
            } => {
                // If brand-new character, add to the character cache. Gated on a
                // verified main account so a mule/alt character loading into
                // nexus is not added to the character pages.
                if let Some(ref pending) = new_character {
                    if self.session.connection.is_account_verified() {
                        self.account_data.characters.add_new_character(
                            char_id,
                            pending.class_id,
                            pending.skin_id as i32,
                            pending.is_seasonal,
                        );
                        self.account_data.mark_dirty();
                        self.char_view_gen += 1;
                    }
                }

                // Notify loot tracker
                self.loot_tracker.on_create_success(char_id, object_id);
                self.combat.on_player_loaded(object_id, char_id);

                // Mark as live character
                self.live_char_id = Some(char_id);
                // Fresh character: drop any prior character's accumulated stats so
                // base = stat - boost is derived from this character's snapshot.
                self.live_stat_raw.clear();
                self.account_data.characters.mark_live(char_id);
                self.char_view_gen += 1;

                // Update PCStats
                if !pc_stats.is_empty() {
                    if self
                        .account_data
                        .characters
                        .apply_pc_stats(char_id, pc_stats)
                    {
                        tracing::debug!(
                            "[CHARACTERS] Updated PCStats for char_id={} (len={})",
                            char_id,
                            pc_stats.len()
                        );
                        self.account_data.mark_dirty();
                        self.char_view_gen += 1;
                    }
                }
            }
            GameEvent::MapChanged {
                ref display_name,
                ref realm_name,
                fp,
                is_dungeon,
                max_realm_score,
                ref dungeon_modifiers,
                ref dungeon_grade,
                ..
            } => {
                // Location update + encounter clearing handled by LiveFeedPanel (broadcast).
                // Remaining: notify loot tracker of new map.
                self.loot_tracker.on_map_info(display_name, fp);
                self.combat.on_map_change(display_name, fp, now_ms());

                // Lost Halls Rune monuments: surface a single combined entry when
                // the player enters Oryx's Sanctuary, listing only the monuments a
                // real player activated (auto-activations are silent). Clear the
                // tracker on Sanctuary entry and whenever leaving Lost Halls so a
                // failed/other run never leaks stale poppers.
                let map = display_name.trim();
                if map == "Oryx's Sanctuary" {
                    self.flush_monument_entry();
                    self.monument_poppers = [None, None, None];
                } else if map != "Lost Halls" {
                    self.monument_poppers = [None, None, None];
                }

                // Track realm entry so join-time taunt replays can be suppressed.
                // Realms report a realm score; nexus/dungeons do not.
                self.realm_entered_at = if max_realm_score > 0 {
                    Some(std::time::Instant::now())
                } else {
                    None
                };

                // New realm/map: reset the Keyper tower-wave latch only when
                // this is a *different* realm than the one the latch applies to.
                // Leaving to a dungeon/nexus (max_realm_score == 0) keeps both
                // the latch and its realm, so returning to the same realm with a
                // still-active tower wave doesn't re-notify.
                if max_realm_score > 0 {
                    let realm = Some(realm_name.clone());
                    if self.keyper_realm != realm {
                        self.keyper_realm = realm;
                        self.keyper_towers_announced = false;
                    }
                }

                // Optional alerts: play a dedicated sound once when entering a
                // dungeon whose modifier set drives a special callout outline.
                // Golden (Dimitus) and Red (dangerous) outlines
                // each get their own sound. Repeated MapInfo packets are deduped.
                if Self::update_dungeon_alert(
                    &mut self.last_dungeon_alert,
                    is_dungeon,
                    fp,
                    display_name,
                    dungeon_modifiers,
                    dungeon_grade,
                ) {
                    use realmhound_core::dungeon_modifiers::{outline_kind, OutlineKind};
                    let (dimitus, bad_mod) = self
                        .settings
                        .read()
                        .map(|s| (s.sound.dimitus_dungeon, s.sound.bad_mod_warning))
                        .unwrap_or((false, false));
                    match outline_kind(dungeon_modifiers) {
                        OutlineKind::Golden if dimitus => {
                            self.emit(UiPayload::Audio(AudioCommand::Play(
                                SoundType::DimitusAlert,
                            )));
                        }
                        OutlineKind::Red if bad_mod => {
                            self.emit(UiPayload::Audio(AudioCommand::Play(
                                SoundType::BadModWarning,
                            )));
                        }
                        _ => {}
                    }
                }
            }
            GameEvent::VaultContentReceived {
                ref packet,
                vault_type,
            } => {
                // Update the canonical account vault (single source of truth).
                self.account_data
                    .update_vault_from_packet(packet, vault_type);
                // Drives vault + Treasury live-vault resync.
                self.vault_view_gen += 1;
            }
            GameEvent::TextReceived(ref text) => {
                // Detect realm-close / lag-warning before from_text_packet filters them out.
                if text.name.contains("Oryx the Mad God") {
                    let raw = &text.text;
                    if raw.contains("s.oryx_closed_realm") {
                        self.emit(UiPayload::RealmClosed);
                    } else if raw.contains("s.oryx_minions_failed") {
                        self.emit(UiPayload::OryxLagWarning);
                    }
                    // Discovery capture: Oryx's per-encounter death / "still alive"
                    // taunts arrive under stringlist keys the spawn parser ignores
                    // (only `.new.` is understood). Log the non-spawn ones so their
                    // key format can be identified for the dedup-reset-on-kill work.
                    if !raw.contains(".new.") {
                        crate::event_log::capture_oryx_taunt(&text.name, raw);
                    }
                }

                // Special "area unlock" broadcasts (Wine Cellar Incantation, Lost
                // Halls Rune monuments, Vial of Pure Darkness). Kept as a fallback
                // for any that arrive as plain text; the primary carrier is the
                // ServerMessage notification path (`AreaUnlockBroadcast`).
                if let Some(msg) = parse_area_unlock(&text.text) {
                    self.apply_area_unlock(msg);
                }

                // Server death broadcast ("<name> died at level N, killed by ...").
                // Authoritative remote-death signal for the combat tracker.
                if let Some((victim, killer)) = parse_remote_death(&text.name, &text.text) {
                    tracing::info!(
                        "[COMBAT] Remote death broadcast: player='{}' killed_by='{}'",
                        victim,
                        killer
                    );
                    self.combat.on_remote_death(&victim, now_ms());
                }

                // Boss taunts drive the Marble Colossus survival-phase split off
                // its authoritative second-coming taunt (the object id ties the
                // taunt to the boss's fight, so no fuzzy matching is needed).
                self.combat
                    .on_boss_text(text.object_id, &text.text, now_ms());

                // Keyper seasonal event: the realm-wide "#The Keyper" taunts are
                // the authoritative spawn/tower cues. Suppress join-time replays
                // like ordinary boss calls.
                if let Some(cue) =
                    realmhound_core::keyper::classify_keyper_taunt(&text.name, &text.text)
                {
                    let in_join_settle = self
                        .realm_entered_at
                        .map(|t| t.elapsed() < REALM_JOIN_SETTLE)
                        .unwrap_or(false);
                    if !in_join_settle {
                        self.on_keyper_cue(cue);
                    }
                }

                // Chat message + boss call (resolved here using our own session).
                if let Some(mut chat_msg) = ChatMessage::from_text_packet(text) {
                    if let Some(boss_id) = chat_msg.boss_id {
                        // Drop join-time state replays: when the server syncs us
                        // into a realm it re-broadcasts taunts for events already
                        // alive. The official client hides these, so suppress boss
                        // calls that land within the settle window of realm entry.
                        let in_join_settle = self
                            .realm_entered_at
                            .map(|t| t.elapsed() < REALM_JOIN_SETTLE)
                            .unwrap_or(false);
                        if !in_join_settle {
                            self.emit(UiPayload::BossCall {
                                boss_id,
                                text: chat_msg.text.clone(),
                            });
                        }
                    } else {
                        let me = self.session.connection.detected_account_name.as_deref();
                        let is_self = me
                            .map(|m| chat_msg.sender.eq_ignore_ascii_case(m))
                            .unwrap_or(false);
                        if chat_msg.chat_type == ChatType::Whisper && is_self {
                            chat_msg.is_outgoing = true;
                        }
                        // Social notification sounds (RealmShark parity): ping on
                        // inbound whisper / party / guild messages, never on our
                        // own. Each is gated by its per-sound toggle.
                        if !is_self {
                            let social = match chat_msg.chat_type {
                                ChatType::Whisper => Some(SoundType::Pm),
                                ChatType::Party => Some(SoundType::Party),
                                ChatType::Guild => Some(SoundType::Guild),
                                _ => None,
                            };
                            if let Some(sound) = social {
                                let enabled = self
                                    .settings
                                    .read()
                                    .map(|s| sound.is_enabled(&s.sound))
                                    .unwrap_or(false);
                                if enabled {
                                    self.emit(UiPayload::Audio(AudioCommand::Play(sound)));
                                }
                            }
                        }
                        self.emit(UiPayload::Chat(chat_msg));
                    }
                }

                // Party leave messages.
                if let Some(player_name) = text.text.strip_suffix(" has left the party") {
                    self.emit(UiPayload::PartyMemberLeft(player_name.to_string()));
                }

                // Notify loot tracker (for delayed drop triggers).
                self.loot_tracker.on_text_packet(&text.name, &text.text);
            }
            GameEvent::PortalOpened {
                ref message,
                picture_type,
            } => {
                // Key-pop: the PortalOpened notification carries the opener's
                // name in a `"player":"<name>"` field. Realm/other portal opens
                // omit it, so its presence identifies a genuine key pop.
                if let Some(raw) = extract_json_string_field(message, "player") {
                    let opener = raw.split(',').next().unwrap_or("").trim().to_string();
                    if !opener.is_empty() {
                        let raw_name = realmhound_core::assets::get_asset_manager()
                            .object_name(picture_type)
                            .unwrap_or_default();
                        // The difficulty filter buckets the dungeon by its grave
                        // rating; unknown dungeons pass through.
                        let tier = realmhound_core::assets::key_pop_tier(&raw_name);
                        // Portal object names carry a trailing " Portal"; drop it
                        // for a clean Live Feed label.
                        let dungeon_name = raw_name
                            .strip_suffix(" Portal")
                            .unwrap_or(&raw_name)
                            .to_string();
                        let (play, show) = self
                            .settings
                            .read()
                            .map(|s| {
                                let sound_allowed = s.sound.keypop_tiers.allows(tier);
                                let feed_allowed = s.live_feed.key_pop_tiers.allows(tier);
                                (
                                    s.sound.keypop && sound_allowed,
                                    s.live_feed.show_key_pops && feed_allowed,
                                )
                            })
                            .unwrap_or((false, false));
                        if play {
                            self.emit(UiPayload::Audio(AudioCommand::Play(
                                SoundType::key_pop_for_tier(tier),
                            )));
                        }
                        if show {
                            let mgr = realmhound_core::assets::get_asset_manager();
                            let key_id = mgr
                                .key_id_for_dungeon(&dungeon_name)
                                .unwrap_or(picture_type);
                            let thankable = !self.is_local_player(&opener);
                            self.emit(UiPayload::KeyPop {
                                opener,
                                key_id,
                                dungeon_name,
                                thankable,
                            });
                        }
                    }
                }
            }
            GameEvent::AreaUnlockBroadcast { ref message } => {
                if let Some(msg) = parse_area_unlock_json(message) {
                    self.apply_area_unlock(msg);
                }
            }
            GameEvent::TradeRequested { .. } => {
                let play = self.settings.read().map(|s| s.sound.trade).unwrap_or(false);
                if play {
                    self.emit(UiPayload::Audio(AudioCommand::Play(SoundType::Trade)));
                }
            }
            GameEvent::HelloReceived { .. } => {
                // The access token is bound to its connection candidate at the
                // pre-routing HELLO gate and released only after verification;
                // there is nothing to emit here.
            }
            GameEvent::UpdateReceived(ref update, time_ms) => {
                // Player stats + loot tracker
                if let Some(player_id) = self.session.player.object_id {
                    if let Some(obj) = update.find_object(player_id) {
                        // Ensure character exists in cache (for characters not created this session)
                        if let Some(char_id) = self.session.player.char_id {
                            // Only mutate/dirty when the character is genuinely new;
                            // matches the original behavior where add_new_character was a
                            // no-op for existing characters (no dirty/resync). Gated on a
                            // verified main account so mule/alt characters entering nexus
                            // are never added to the character pages.
                            if self.session.connection.is_account_verified()
                                && self
                                    .account_data
                                    .characters
                                    .find_character(char_id)
                                    .is_none()
                            {
                                let is_seasonal = obj.is_seasonal().unwrap_or(false);
                                let class_id = obj.object_type;
                                self.account_data.characters.add_new_character(
                                    char_id,
                                    class_id,
                                    0, // skin will be updated from NewTick
                                    is_seasonal,
                                );
                                self.account_data.mark_dirty();
                                self.char_view_gen += 1;
                            }
                        }

                        self.loot_tracker
                            .on_player_object_type(obj.object_type as i32);
                        self.loot_tracker
                            .on_player_stats(player_id, &obj.status, time_ms);
                        // Spawn snapshot carries all base stats + boosts: seed the
                        // live stats accumulator so the cache's maxed count and
                        // derived stats are correct immediately, not only after an
                        // API refresh.
                        self.merge_live_stats(&obj.status);

                        // Multi-client isolation: verify account identity. All
                        // mismatch/first-run/name side effects live in the helper
                        // so the gated dispatch path can reuse them verbatim.
                        let verify = self.run_account_verification(update);
                        let account_mismatch =
                            matches!(verify, AccountVerifyResult::Mismatch { .. });

                        // The Update packet carries the player's full status, so it
                        // is the reliable source for the current skin/dyes when a
                        // character was skinned before capture started.
                        // Skipped on account mismatch so a rejected alt connection
                        // never overwrites the main character's appearance.
                        if !account_mismatch {
                            if let Some(char_id) = self.session.player.char_id {
                                let appearance = parse_player_appearance(&obj.status);
                                if self.account_data.characters.apply_appearance(
                                    char_id,
                                    appearance.skin,
                                    appearance.tex1,
                                    appearance.tex2,
                                ) {
                                    self.account_data.mark_dirty();
                                    self.char_view_gen += 1;
                                }
                            }
                        }
                    }
                }

                // Process new objects for loot tracking + encounters
                for obj in &update.new_objects {
                    self.loot_tracker.on_object_spawn(
                        obj.object_id(),
                        obj.object_type as i32,
                        &obj.status,
                        time_ms,
                    );
                    self.combat.on_object_spawn(
                        obj.object_id(),
                        obj.object_type as i32,
                        &obj.status,
                        time_ms as i64,
                    );
                }

                // A pet's full inventory + identity ride only in its spawn
                // `Update`; later NewTicks are position-only. Detection can
                // latch a tick after the spawn (the ActivePetUpdate mapping or
                // the map-load proximity latch), so buffer every known pet's
                // spawn status, reconcile the active pet, then replay the
                // detected pet's buffered status so its inventory lands.
                for obj in &update.new_objects {
                    if self
                        .session
                        .player
                        .known_pet_object_ids
                        .contains(&obj.object_id())
                    {
                        self.pet_object_buffer
                            .insert(obj.object_id(), obj.status.clone());
                    }
                }
                if let Some(active_pet_id) = self.session.player.active_pet_instance_id {
                    self.apply_active_pet(active_pet_id);
                }
                self.enrich_detected_pet();

                // Forward encounter spawns to live feed
                for encounter in extract_encounter_spawns(&update.new_objects) {
                    self.emit(UiPayload::AddEncounter {
                        object_id: encounter.object_id,
                        object_type: encounter.sprite_id,
                        name: encounter.name,
                    });
                }

                // Keyper crystal-tower wave: the first wave (~33% realm score)
                // has no chat taunt, so detect it from the tower entities. The
                // realm-scoped latch (reset on Keyper spawn / map change)
                // collapses the repeated tower spawns into one notification per
                // wave. Suppress join-time replays like other realm cues.
                if !self.keyper_towers_announced
                    && update
                        .new_objects
                        .iter()
                        .any(|o| realmhound_core::keyper::is_keyper_tower(o.object_type as i32))
                {
                    let in_join_settle = self
                        .realm_entered_at
                        .map(|t| t.elapsed() < REALM_JOIN_SETTLE)
                        .unwrap_or(false);
                    if !in_join_settle {
                        self.on_keyper_cue(realmhound_core::keyper::KeyperCue::TowersAppeared);
                    }
                }

                // Process removed objects
                for &object_id in &update.drops {
                    self.loot_tracker.on_entity_dropped(object_id, time_ms);
                    self.loot_tracker.on_entity_removed(object_id);
                    self.combat.on_object_removed(object_id, time_ms as i64);
                    self.emit(UiPayload::RemoveEncounter(object_id));
                    self.pet_object_buffer.remove(&object_id);
                }
            }
            GameEvent::NewTickReceived(ref new_tick, time_ms) => {
                // Update character inventory from parsed stat data
                if let Some(player_id) = self.session.player.object_id {
                    if let Some(status) = new_tick.find_status(player_id) {
                        if let Some(char_id) = self.session.player.char_id {
                            if let Some(inv) = parse_player_inventory(status) {
                                if self.account_data.characters.apply_newtick_inventory(
                                    char_id,
                                    inv.equipment,
                                    inv.inventory,
                                    inv.backpack,
                                    inv.backpack_ext,
                                    inv.belt,
                                ) {
                                    self.account_data.mark_dirty();
                                    self.char_view_gen += 1;
                                }
                            }
                            // Skin/dyes can arrive without inventory stats, so sync
                            // them independently of the inventory parse.
                            let appearance = parse_player_appearance(status);
                            if self.account_data.characters.apply_appearance(
                                char_id,
                                appearance.skin,
                                appearance.tex1,
                                appearance.tex2,
                            ) {
                                self.account_data.mark_dirty();
                                self.char_view_gen += 1;
                            }
                        }
                    }
                }

                // Update pet inventory from NewTick
                // Reconcile the buffered active-pet instance id first: an
                // ActivePetUpdate that arrived before the character id was known
                // (early in map load) is applied here so live updates map to the
                // correct pet even if the initial event was dropped.
                if let Some(active_pet_id) = self.session.player.active_pet_instance_id {
                    self.apply_active_pet(active_pet_id);
                }
                if let Some(pet_id) = self.session.player.pet_object_id {
                    if let Some(status) = new_tick.find_status(pet_id) {
                        self.apply_pet_object(status);
                    }
                }
                // If the pet was just detected this tick (map-load proximity
                // latch), replay its buffered spawn status so its inventory +
                // identity populate without waiting for an API refresh.
                self.enrich_detected_pet();

                // Track vault chest inventory updates from NewTick
                let vault_type = if self.session.player.is_seasonal == Some(true) {
                    VaultType::Seasonal
                } else {
                    VaultType::Regular
                };

                let mut vault_updated = false;

                let detect_page_from_changes =
                    |items: &[LiveVaultItem], changes: &[(usize, i32)]| -> Option<usize> {
                        if changes.is_empty() {
                            return None;
                        }
                        let num_pages = (items.len() + 7) / 8;
                        for page in 0..num_pages {
                            let start = page * 8;
                            let mut all_match = true;
                            for &(slot, item_id) in changes {
                                let idx = start + slot;
                                let stored_id = items.get(idx).map(|i| i.item_id).unwrap_or(-1);
                                if stored_id != item_id {
                                    all_match = false;
                                    break;
                                }
                            }
                            if all_match {
                                return Some(page);
                            }
                        }
                        None
                    };

                // Check vault chest for updates
                if let Some(vault_id) = self.session.vault.chest_object_id {
                    if let Some(status) = new_tick.find_status(vault_id) {
                        let changes = extract_inventory_changes(status);
                        if !changes.is_empty() {
                            let storage = self.account_data.get_vault(vault_type);
                            if let Some(page) =
                                detect_page_from_changes(&storage.vault_items, &changes)
                            {
                                if self.session.vault.active_vault_page != Some(page) {
                                    self.session.vault.active_vault_page = Some(page);
                                }
                            } else if let Some(page) = self.session.vault.active_vault_page {
                                let storage = self.account_data.get_vault_mut(vault_type);
                                for (slot, item_id) in changes {
                                    let abs_idx = page * 8 + slot;
                                    if storage.update_vault_slot(abs_idx, item_id) {
                                        vault_updated = true;
                                    }
                                }
                            }
                        }
                    }
                }

                // Check material chest for updates
                if let Some(material_id) = self.session.vault.material_chest_object_id {
                    if let Some(status) = new_tick.find_status(material_id) {
                        let changes = extract_inventory_changes(status);
                        if !changes.is_empty() {
                            let storage = self.account_data.get_vault(vault_type);
                            if let Some(page) =
                                detect_page_from_changes(&storage.material_items, &changes)
                            {
                                if self.session.vault.active_material_page != Some(page) {
                                    self.session.vault.active_material_page = Some(page);
                                }
                            } else if let Some(page) = self.session.vault.active_material_page {
                                let storage = self.account_data.get_vault_mut(vault_type);
                                for (slot, item_id) in changes {
                                    let abs_idx = page * 8 + slot;
                                    if storage.update_material_slot(abs_idx, item_id) {
                                        vault_updated = true;
                                    }
                                }
                            }
                        }
                    }
                }

                // Check gift chest for updates
                if let Some(gift_id) = self.session.vault.gift_chest_object_id {
                    if let Some(status) = new_tick.find_status(gift_id) {
                        let changes = extract_inventory_changes(status);
                        if !changes.is_empty() {
                            let storage = self.account_data.get_vault(vault_type);
                            if let Some(page) =
                                detect_page_from_changes(&storage.gift_items, &changes)
                            {
                                if self.session.vault.active_gift_page != Some(page) {
                                    self.session.vault.active_gift_page = Some(page);
                                }
                            } else if let Some(page) = self.session.vault.active_gift_page {
                                let storage = self.account_data.get_vault_mut(vault_type);
                                for (slot, item_id) in changes {
                                    let abs_idx = page * 8 + slot;
                                    if storage.update_gift_slot(abs_idx, item_id) {
                                        vault_updated = true;
                                    }
                                }
                            }
                        }
                    }
                }

                // Check potion storage for updates
                if let Some(potion_id) = self.session.vault.potion_storage_object_id {
                    if let Some(status) = new_tick.find_status(potion_id) {
                        let changes = extract_inventory_changes(status);
                        if !changes.is_empty() {
                            let storage = self.account_data.get_vault(vault_type);
                            if let Some(page) =
                                detect_page_from_changes(&storage.potion_items, &changes)
                            {
                                if self.session.vault.active_potion_page != Some(page) {
                                    self.session.vault.active_potion_page = Some(page);
                                }
                            } else if let Some(page) = self.session.vault.active_potion_page {
                                let storage = self.account_data.get_vault_mut(vault_type);
                                for (slot, item_id) in changes {
                                    let abs_idx = page * 8 + slot;
                                    if storage.update_potion_slot(abs_idx, item_id) {
                                        vault_updated = true;
                                    }
                                }
                            }
                        }
                    }
                }

                if vault_updated {
                    self.vault_view_gen += 1;
                    // In-session vault moves mutate the canonical account vault.
                    // Bump the account generation (marks dirty + refreshes the
                    // published account snapshot) so the periodic save persists
                    // the move AND account_data consumers (e.g. QuestPanel item
                    // counts) stay consistent with the live vault view.
                    self.account_data.bump_generation();
                }

                // Update player stats from NewTick
                if let Some(player_id) = self.session.player.object_id {
                    if let Some(status) = new_tick.find_status(player_id) {
                        self.loot_tracker
                            .on_player_stats(player_id, status, time_ms);
                        // Fold NewTick stat deltas (e.g. a stat maxed mid-run)
                        // into the live stats accumulator.
                        self.merge_live_stats(status);
                    }
                }
                // Keep the live character's alive fame current from the observed
                // CurrFame, rather than only after a full account refresh.
                self.sync_live_fame();

                // Update entity positions/stats for loot tracking
                for status in &new_tick.statuses {
                    self.loot_tracker
                        .on_object_update(status.object_id, status, time_ms);
                    self.combat
                        .on_object_status(status.object_id, status, time_ms as i64);
                }

                // Fold deferred combat tallies onto the character cache.
                self.fold_deferred_combat();

                // Process pending bags and write to database. Feed the loot
                // tracker the combat tracker's recently killed bosses first so
                // Unknown bags can fall back to fight-correlation.
                let recent_kills: Vec<RecentBossKill> = self
                    .combat
                    .tracker()
                    .recent_fights()
                    .iter()
                    .filter(|f| f.killed && f.boss_object_type > 0)
                    .map(|f| RecentBossKill {
                        map_seed: f.map_seed,
                        object_type: f.boss_object_type,
                        name: f.boss_name.clone(),
                        started_at_ms: f.started_at,
                        ended_at_ms: f.ended_at,
                    })
                    .collect();
                self.loot_tracker.set_recent_boss_kills(recent_kills);
                #[cfg(feature = "latency-diagnostics")]
                let loot_started = std::time::Instant::now();
                let new_drops = self.loot_tracker.on_tick(time_ms);
                #[cfg(feature = "latency-diagnostics")]
                {
                    let duration = loot_started.elapsed();
                    if duration >= std::time::Duration::from_millis(100) {
                        tracing::warn!(
                            "[LATENCY][STORAGE] operation=loot_on_tick duration_ms={} \
                             drop_count={} item_count={}",
                            duration.as_millis(),
                            new_drops.len(),
                            new_drops.iter().map(|drop| drop.items.len()).sum::<usize>()
                        );
                    }
                }

                // Finalize any killed / timed-out fights.
                self.combat.on_tick(time_ms as i64);

                // Push new loot drops to live feed and play sounds
                let enchant_ctx = self.enchant_sound_context();
                for drop in &new_drops {
                    // A core-boss bag latches its realm-event card to Completed
                    // even when the core (e.g. Towering Perfection) was never seen
                    // dying and only its segments were damaged.
                    self.combat
                        .on_boss_loot(drop.mob_type, drop.player.map_seed);
                    self.emit(UiPayload::PushLoot(drop.clone()));
                    self.emit(UiPayload::Audio(AudioCommand::PlayForBag(drop.bag_type)));
                    if let Some((ref settings, ref catalog)) = enchant_ctx {
                        self.play_enchant_sounds(drop, settings, catalog);
                    }
                }

                // Invalidate autocomplete cache when new drops added
                if !new_drops.is_empty() {
                    self.emit(UiPayload::LootBatchAdded);
                }
            }
            GameEvent::InvSwapReceived(ref inv_swap) => {
                let vault_type = if self.session.player.is_seasonal == Some(true) {
                    VaultType::Seasonal
                } else {
                    VaultType::Regular
                };

                let mut vault_updated = false;
                let from = &inv_swap.slot_from;
                let to = &inv_swap.slot_to;

                let chest_id = self.session.vault.chest_object_id;
                let material_id = self.session.vault.material_chest_object_id;
                let gift_id = self.session.vault.gift_chest_object_id;
                let potion_id = self.session.vault.potion_storage_object_id;
                let char_id = self.session.player.char_id;

                let get_item = |storage: &realmhound_core::vault::LiveVaultStorage,
                                obj_id: i32,
                                slot_id: i32|
                 -> Option<LiveVaultItem> {
                    let idx = slot_id as usize;
                    if Some(obj_id) == chest_id {
                        storage.get_vault_item(idx)
                    } else if Some(obj_id) == material_id {
                        storage.get_material_item(idx)
                    } else if Some(obj_id) == gift_id {
                        storage.get_gift_item(idx)
                    } else if Some(obj_id) == potion_id {
                        storage.get_potion_item(idx)
                    } else {
                        None
                    }
                };

                let set_item = |storage: &mut realmhound_core::vault::LiveVaultStorage,
                                obj_id: i32,
                                slot_id: i32,
                                item: LiveVaultItem|
                 -> bool {
                    let idx = slot_id as usize;
                    if Some(obj_id) == chest_id {
                        storage.set_vault_item(idx, item)
                    } else if Some(obj_id) == material_id {
                        storage.set_material_item(idx, item)
                    } else if Some(obj_id) == gift_id {
                        storage.set_gift_item(idx, item)
                    } else if Some(obj_id) == potion_id {
                        storage.set_potion_item(idx, item)
                    } else {
                        false
                    }
                };

                // Read enchant ids from the canonical character cache (must reflect
                // inventory applied earlier in this same batch).
                let to_enchants = char_id
                    .and_then(|cid| self.account_data.characters.item_at_slot(cid, to.slot_id))
                    .map(|item| item.enchant_ids)
                    .unwrap_or_default();
                let from_enchants = char_id
                    .and_then(|cid| self.account_data.characters.item_at_slot(cid, from.slot_id))
                    .map(|item| item.enchant_ids)
                    .unwrap_or_default();

                let storage = self.account_data.get_vault_mut(vault_type);

                let from_item = get_item(storage, from.object_id, from.slot_id);
                let to_item = get_item(storage, to.object_id, to.slot_id);

                if let Some(to_item_data) = to_item {
                    if set_item(storage, from.object_id, from.slot_id, to_item_data) {
                        vault_updated = true;
                    }
                } else if [chest_id, material_id, gift_id, potion_id]
                    .contains(&Some(from.object_id))
                {
                    let new_item = LiveVaultItem::with_enchants(to.item_type, to_enchants);
                    if set_item(storage, from.object_id, from.slot_id, new_item) {
                        vault_updated = true;
                    }
                }

                if let Some(from_item_data) = from_item {
                    if set_item(storage, to.object_id, to.slot_id, from_item_data) {
                        vault_updated = true;
                    }
                } else if [chest_id, material_id, gift_id, potion_id].contains(&Some(to.object_id))
                {
                    let new_item = LiveVaultItem::with_enchants(from.item_type, from_enchants);
                    if set_item(storage, to.object_id, to.slot_id, new_item) {
                        vault_updated = true;
                    }
                }

                if vault_updated {
                    self.vault_view_gen += 1;
                    // In-session vault moves mutate the canonical account vault.
                    // Bump the account generation (marks dirty + refreshes the
                    // published account snapshot) so the periodic save persists
                    // the move AND account_data consumers (e.g. QuestPanel item
                    // counts) stay consistent with the live vault view.
                    self.account_data.bump_generation();
                }
            }
        }

        // Settle the Live Feed dungeon timer once a run has ended. The freeze is
        // targeted by map seed so it lands on the correct row even though the
        // Broadcast for a new dungeon is emitted before this run's flush.
        self.forward_dungeon_freeze();
    }

    /// Drain a pending dungeon-run freeze from the CombatManager and forward it
    /// to the Live Feed so its run timer settles to the committed per-run value.
    fn forward_dungeon_freeze(&mut self) {
        if let Some((map_seed, elapsed_ms)) = self.combat.take_dungeon_freeze() {
            self.emit(UiPayload::DungeonTimerFrozen {
                map_seed,
                elapsed_ms,
            });
        }
    }

    /// Handle a resolved Keyper-event cue: push its Live Feed entry (via the
    /// generic `KeyperEvent` payload) and maintain the tower-wave latch. A
    /// Keyper spawn means the towers were just destroyed, so it resets the latch
    /// to arm the next tower wave.
    fn on_keyper_cue(&mut self, cue: realmhound_core::keyper::KeyperCue) {
        use realmhound_core::keyper::KeyperCue;
        match cue {
            KeyperCue::KeyperSpawned => {
                self.keyper_towers_announced = false;
                self.emit(UiPayload::KeyperEvent { towers: false });
            }
            KeyperCue::TowersAppeared => {
                // Already announced this wave (taunt + nearby tower entities can
                // both arrive): notify once, until the next Keyper spawn / realm
                // change re-arms the latch.
                if self.keyper_towers_announced {
                    return;
                }
                self.keyper_towers_announced = true;
                self.emit(UiPayload::KeyperEvent { towers: true });
            }
        }
    }

    /// Decide whether a dungeon alert should fire for this map and update the
    /// stored signature to dedup repeated MapInfo packets. Fires for any
    /// distinct dungeon entry; the caller maps the modifier set to a specific
    /// alert sound via [`outline_kind`](realmhound_core::dungeon_modifiers::outline_kind).
    ///
    /// Returns `true` exactly once per distinct dungeon entry.
    fn update_dungeon_alert(
        last_sig: &mut Option<(i32, String, Vec<String>, Option<String>)>,
        is_dungeon: bool,
        fp: i32,
        display_name: &str,
        modifiers: &[String],
        grade: &Option<String>,
    ) -> bool {
        if !is_dungeon {
            *last_sig = None;
            return false;
        }
        let sig = (
            fp,
            display_name.to_string(),
            modifiers.to_vec(),
            grade.clone(),
        );
        if last_sig.as_ref() == Some(&sig) {
            return false;
        }
        *last_sig = Some(sig);
        true
    }
}

/// Parse a captured packet into a typed `ParsedPacket`, logging unparsed
/// incoming packets at trace level so nothing is silently dropped.
fn parse_packet_for_processing(packet: &Packet) -> Option<ParsedPacket> {
    let packet_type = packet.packet_type();
    if packet_type != realmhound_core::protocol::PacketType::Unknown
        && packet.incoming != packet_type.is_incoming()
    {
        tracing::warn!(
            "[DIRECTION] id={} ({}) direction mismatch: observed={} expected={} src={}:{} dst={}:{} len={}",
            packet.header.raw_id,
            packet.type_name(),
            if packet.incoming { "server->client" } else { "client->server" },
            if packet_type.is_incoming() { "server->client" } else { "client->server" },
            packet.src_ip,
            packet.src_port,
            packet.dst_ip,
            packet.dst_port,
            packet.payload.len(),
        );
    }

    let result = parse_packet_with_status(packet.packet_type(), &packet.payload);

    if packet.incoming {
        match &result.packet {
            Some(_) if result.fully_parsed => {
                tracing::trace!(
                    "[PARSE] id={} ({}) fully parsed (len={})",
                    packet.header.raw_id,
                    packet.type_name(),
                    packet.payload.len(),
                );
            }
            Some(_) => {
                let unread = &packet.payload[result.bytes_consumed..];
                tracing::warn!(
                    "[PARSE] id={} ({}) partially parsed (len={}, consumed={}, unread={})",
                    packet.header.raw_id,
                    packet.type_name(),
                    packet.payload.len(),
                    result.bytes_consumed,
                    unread.len(),
                );
            }
            None => {
                tracing::debug!(
                    "[PARSE] id={} ({}) parse failed (len={}, consumed={})",
                    packet.header.raw_id,
                    packet.type_name(),
                    packet.payload.len(),
                    result.bytes_consumed,
                );
            }
        }
    }

    result.packet
}

#[cfg(test)]
mod dungeon_alert_tests {
    use super::PacketProcessor;
    use realmhound_core::dungeon_modifiers::{outline_kind, OutlineKind};

    fn modifiers(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn fires_once_for_repeated_mapinfo() {
        let mut sig = None;
        let mods = modifiers(&["DIMITUS", "WEAKBOSS_3"]);
        assert!(PacketProcessor::update_dungeon_alert(
            &mut sig,
            true,
            1,
            "Snake Pit",
            &mods,
            &None
        ));
        assert!(!PacketProcessor::update_dungeon_alert(
            &mut sig,
            true,
            1,
            "Snake Pit",
            &mods,
            &None
        ));
    }

    #[test]
    fn fires_for_any_dungeon_entry() {
        // Sound selection is delegated to outline_kind; the dedup helper fires
        // for any dungeon entry, including plain (blue) ones.
        let mut sig = None;
        let mods = modifiers(&["WEAKBOSS_3"]);
        assert!(PacketProcessor::update_dungeon_alert(
            &mut sig,
            true,
            1,
            "Snake Pit",
            &mods,
            &None
        ));
    }

    #[test]
    fn does_not_fire_outside_dungeon() {
        let mut sig = None;
        let mods = modifiers(&["DIMITUS"]);
        assert!(!PacketProcessor::update_dungeon_alert(
            &mut sig, false, 0, "Nexus", &mods, &None
        ));
    }

    #[test]
    fn refires_after_leaving_and_reentering() {
        let mut sig = None;
        let mods = modifiers(&["DIMITUS"]);
        assert!(PacketProcessor::update_dungeon_alert(
            &mut sig,
            true,
            1,
            "Snake Pit",
            &mods,
            &None
        ));
        assert!(!PacketProcessor::update_dungeon_alert(
            &mut sig,
            false,
            0,
            "Nexus",
            &[],
            &None
        ));
        assert!(PacketProcessor::update_dungeon_alert(
            &mut sig,
            true,
            1,
            "Snake Pit",
            &mods,
            &None
        ));
    }

    #[test]
    fn refires_for_new_instance_same_signature() {
        // Two distinct dungeon instances with identical name/modifiers/grade but
        // different map seeds (fp) each fire.
        let mut sig = None;
        let mods = modifiers(&["DIMITUS"]);
        assert!(PacketProcessor::update_dungeon_alert(
            &mut sig,
            true,
            1,
            "Snake Pit",
            &mods,
            &None
        ));
        assert!(PacketProcessor::update_dungeon_alert(
            &mut sig,
            true,
            2,
            "Snake Pit",
            &mods,
            &None
        ));
    }

    #[test]
    fn fires_for_different_dungeon() {
        let mut sig = None;
        let mods = modifiers(&["DIMITUS"]);
        assert!(PacketProcessor::update_dungeon_alert(
            &mut sig,
            true,
            1,
            "Snake Pit",
            &mods,
            &None
        ));
        assert!(PacketProcessor::update_dungeon_alert(
            &mut sig,
            true,
            1,
            "Spider Den",
            &mods,
            &None
        ));
    }

    #[test]
    fn outline_kind_drives_sound_choice() {
        let _assets = crate::test_support::modifier_assets();
        // Dimitus -> golden (Dimitus alert), dangerous -> red (bad-mod warning),
        // anything else -> blue (no alert).
        assert_eq!(outline_kind(&modifiers(&["DIMITUS"])), OutlineKind::Golden);
        assert_eq!(outline_kind(&modifiers(&["EXPOSED_1"])), OutlineKind::Red);
        assert_eq!(outline_kind(&modifiers(&["WEAKBOSS_3"])), OutlineKind::Blue);
    }
}

#[cfg(test)]
mod vault_persistence_tests {
    use super::PacketProcessor;
    use realmhound_core::protocol::packets::{InvSwapPacket, SlotObjectData};
    use realmhound_core::vault::{AccountData, LiveVaultItem};
    use realmhound_core::GameEvent;

    /// An incremental in-session vault move (via `InvSwapReceived`) must land in
    /// the canonical `account_data` vault and survive a save -> reload round-trip.
    #[test]
    fn incremental_vault_move_survives_save_reload() {
        // Seed a regular vault page (8 slots): item 1000 in slot 0, slot 1 empty.
        let mut account_data = AccountData::new();
        account_data.regular_vault.vault_items = vec![LiveVaultItem::default(); 8];
        account_data.regular_vault.vault_items[0] = LiveVaultItem::new(1000);

        let mut processor = PacketProcessor::new_for_test(account_data);

        // Live in a non-seasonal char with the vault chest open (object id 100).
        processor.session.player.is_seasonal = Some(false);
        processor.session.vault.chest_object_id = Some(100);

        // Move the item from vault slot 0 to the empty vault slot 1.
        let event = GameEvent::InvSwapReceived(InvSwapPacket {
            time: 0,
            player_x: 0.0,
            player_y: 0.0,
            slot_from: SlotObjectData {
                object_id: 100,
                slot_id: 0,
                item_type: 1000,
            },
            slot_to: SlotObjectData {
                object_id: 100,
                slot_id: 1,
                item_type: -1,
            },
        });
        processor.dispatch_event(event);

        // The canonical account vault reflects the move and is marked dirty.
        let vault = &processor.account_data.regular_vault;
        assert_eq!(
            vault.vault_items[0].item_id, -1,
            "source slot should be empty"
        );
        assert_eq!(
            vault.vault_items[1].item_id, 1000,
            "item should have moved to slot 1"
        );
        assert!(
            processor.account_data.is_dirty(),
            "vault move must mark account_data dirty"
        );

        // Save -> reload round-trip (serde, mirroring the on-disk JSON format).
        let json = serde_json::to_string(&processor.account_data).expect("serialize");
        let reloaded: AccountData = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(
            reloaded.regular_vault.vault_items[1].item_id, 1000,
            "moved item must persist across save/reload"
        );
        assert_eq!(reloaded.regular_vault.vault_items[0].item_id, -1);
    }

    #[cfg(test)]
    mod combat_award_retry_tests {
        use super::PacketProcessor;
        use realmhound_core::vault::{AccountData, CachedCharacter};

        #[test]
        fn missing_stat_awards_apply_after_character_appears() {
            let mut processor = PacketProcessor::new_for_test(AccountData::new());
            processor.pending_stat_awards.insert(77, (2, 3, 4));

            processor.fold_deferred_combat();
            assert_eq!(processor.pending_stat_awards.get(&77), Some(&(2, 3, 4)));

            processor
                .account_data
                .characters
                .characters
                .push(CachedCharacter {
                    char_id: 77,
                    ..Default::default()
                });
            processor.fold_deferred_combat();

            assert!(processor.pending_stat_awards.is_empty());
            let character = &processor.account_data.characters.characters[0];
            assert_eq!(
                (
                    character.lone_fighter,
                    character.last_hero_standing,
                    character.most_damage_taken,
                ),
                (2, 3, 4)
            );
        }
    }
}

#[cfg(test)]
mod live_pet_tests {
    use super::PacketProcessor;
    use realmhound_core::protocol::data::{ObjectStatusData, StatData, StatType, WorldPosData};
    use realmhound_core::stream::ConnectionKey;
    use realmhound_core::vault::AccountData;
    use realmhound_core::GameEvent;
    use std::net::Ipv4Addr;

    fn stat(stat_type: StatType, value: i32) -> StatData {
        StatData {
            stat_type_id: 0,
            stat_type,
            stat_value: value,
            stat_value_two: 0,
            string_stat_value: None,
        }
    }

    fn pet_status(object_id: i32) -> ObjectStatusData {
        ObjectStatusData {
            object_id,
            pos: WorldPosData { x: 0.0, y: 0.0 },
            stats: vec![
                stat(StatType::PetType, 4321),
                stat(StatType::Inventory0, 999),
            ],
        }
    }

    /// A pet equipped in a prior session emits no live inventory deltas: its full
    /// inventory arrives only in the spawn `Update`, which is processed before the
    /// `ActivePetUpdate` establishes the instance mapping. The buffered spawn
    /// status must be replayed when the mapping is learned, so the pet is not
    /// hidden (`inventory_slots == 0`) in the Pet Inventories aggregate.
    #[test]
    fn active_pet_after_spawn_backfills_buffered_inventory() {
        let mut account_data = AccountData::new();
        account_data.characters.add_new_character(1232, 0, 0, false);

        let mut processor = PacketProcessor::new_for_test(account_data);

        // Verified main account (required to apply live pet state).
        processor.session.connection.main_connection = Some(ConnectionKey::new(
            Ipv4Addr::new(127, 0, 0, 1),
            1000,
            Ipv4Addr::new(127, 0, 0, 1),
            2050,
        ));
        processor.session.connection.pending_account_verification = false;
        processor.session.player.char_id = Some(1232);
        processor.session.player.pet_object_id = Some(500);

        // Spawn Update processed before the mapping is known: buffer only.
        processor.pet_object_buffer.insert(500, pet_status(500));

        // The active-pet packet arrives later, establishing the instance mapping.
        processor.dispatch_event(GameEvent::ActivePetChanged { instance_id: 77 });

        let pet = processor
            .account_data
            .characters
            .get_pet(77, false)
            .expect("placeholder pet created in regular pool");
        assert!(
            pet.inventory_slots > 0,
            "buffered spawn inventory must backfill so the pet is not hidden"
        );
        assert_eq!(
            pet.pet_type, 4321,
            "identity should backfill from spawn status"
        );
    }
}

#[cfg(test)]
mod join_gate_tests {
    use super::{PacketProcessor, REALM_JOIN_SETTLE};
    use crate::processing::contract::UiPayload;
    use realmhound_core::assets::get_asset_manager;
    use realmhound_core::protocol::TextPacket;
    use realmhound_core::vault::AccountData;
    use realmhound_core::GameEvent;

    fn map_changed(max_realm_score: i32) -> GameEvent {
        GameEvent::MapChanged {
            name: "Realm".to_string(),
            display_name: "Realm of the Mad God".to_string(),
            realm_name: "NexusPortal.Mesa".to_string(),
            fp: 1,
            current_realm_score: 0,
            max_realm_score,
            allows_api: true,
            is_dungeon: false,
            dungeon_modifiers: Vec::new(),
            dungeon_grade: None,
        }
    }

    /// A Beer God spawn taunt (resolves to a boss_id via the embedded assets).
    fn skull_shrine_taunt() -> GameEvent {
        GameEvent::TextReceived(TextPacket {
            name: "Oryx the Mad God".to_string(),
            object_id: 100,
            num_stars: 0,
            bubble_time: 0,
            recipient: String::new(),
            text: r#"Beer God{"k":"stringlist.Beer_Encounter_Spawner.new.0"}"#.to_string(),
            clean_text: "Beer God".to_string(),
            is_supporter: false,
            star_background: 0,
        })
    }

    fn boss_calls(processor: &PacketProcessor) -> usize {
        processor
            .updates
            .iter()
            .filter(|u| matches!(u.payload, UiPayload::BossCall { .. }))
            .count()
    }

    /// Load the real extracted game assets so boss taunts resolve to a boss_id.
    /// Returns `false` (test should skip) when assets aren't available.
    fn assets_ready() -> Option<std::sync::MutexGuard<'static, ()>> {
        let guard = crate::test_support::asset_manager_guard();
        let mgr = get_asset_manager();
        if let Some(dir) = realmhound_core::assets::find_assets_dir() {
            mgr.set_assets_dir(&dir);
        }
        if !mgr.try_load() || mgr.object_id_for_name("Beer God").is_none() {
            eprintln!("skipping: game assets not available");
            return None;
        }
        Some(guard)
    }

    #[test]
    fn entering_realm_arms_and_nexus_disarms_the_join_gate() {
        let mut processor = PacketProcessor::new_for_test(AccountData::new());
        processor.dispatch_event(map_changed(100));
        assert!(
            processor.realm_entered_at.is_some(),
            "realm entry arms the gate"
        );
        processor.dispatch_event(map_changed(0));
        assert!(
            processor.realm_entered_at.is_none(),
            "nexus/dungeon disarms the gate"
        );
    }

    #[test]
    fn boss_taunt_within_settle_window_is_suppressed() {
        let Some(_assets) = assets_ready() else {
            return;
        };
        let mut processor = PacketProcessor::new_for_test(AccountData::new());
        processor.dispatch_event(map_changed(100));
        processor.dispatch_event(skull_shrine_taunt());
        assert_eq!(
            boss_calls(&processor),
            0,
            "join-time replay must be suppressed"
        );
    }

    #[test]
    fn boss_taunt_after_settle_window_notifies() {
        let Some(_assets) = assets_ready() else {
            return;
        };
        let mut processor = PacketProcessor::new_for_test(AccountData::new());
        processor.dispatch_event(map_changed(100));
        // Pretend we entered the realm well before the settle window.
        processor.realm_entered_at = Some(
            std::time::Instant::now() - (REALM_JOIN_SETTLE + std::time::Duration::from_secs(1)),
        );
        processor.dispatch_event(skull_shrine_taunt());
        assert_eq!(
            boss_calls(&processor),
            1,
            "genuine spawn after join must notify"
        );
    }

    #[test]
    fn boss_taunt_outside_a_realm_notifies() {
        let Some(_assets) = assets_ready() else {
            return;
        };
        let mut processor = PacketProcessor::new_for_test(AccountData::new());
        // No realm entry recorded (e.g. dungeon): the gate is inactive.
        processor.dispatch_event(map_changed(0));
        processor.dispatch_event(skull_shrine_taunt());
        assert_eq!(boss_calls(&processor), 1);
    }
}

#[cfg(test)]
mod keyper_tests {
    use super::{PacketProcessor, REALM_JOIN_SETTLE};
    use crate::processing::contract::UiPayload;
    use realmhound_core::protocol::TextPacket;
    use realmhound_core::vault::AccountData;
    use realmhound_core::GameEvent;

    fn map_changed(max_realm_score: i32) -> GameEvent {
        map_changed_realm(max_realm_score, "NexusPortal.Mesa")
    }

    fn map_changed_realm(max_realm_score: i32, realm: &str) -> GameEvent {
        GameEvent::MapChanged {
            name: "Realm".to_string(),
            display_name: "Realm of the Mad God".to_string(),
            realm_name: realm.to_string(),
            fp: 1,
            current_realm_score: 0,
            max_realm_score,
            allows_api: true,
            is_dungeon: false,
            dungeon_modifiers: Vec::new(),
            dungeon_grade: None,
        }
    }

    fn keyper_taunt(text: &str) -> GameEvent {
        GameEvent::TextReceived(TextPacket {
            name: "#The Keyper".to_string(),
            object_id: 19246,
            num_stars: 0,
            bubble_time: 0,
            recipient: String::new(),
            text: text.to_string(),
            clean_text: text.to_string(),
            is_supporter: false,
            star_background: 0,
        })
    }

    /// Count emitted Keyper events by kind (towers vs Keyper spawn).
    fn keyper_events(processor: &PacketProcessor) -> (usize, usize) {
        let mut towers = 0;
        let mut spawn = 0;
        for u in &processor.updates {
            if let UiPayload::KeyperEvent { towers: t } = u.payload {
                if t {
                    towers += 1;
                } else {
                    spawn += 1;
                }
            }
        }
        (towers, spawn)
    }

    /// Enter a realm and move past the join-settle window so cues notify.
    fn enter_realm_settled(processor: &mut PacketProcessor) {
        processor.dispatch_event(map_changed(100));
        processor.realm_entered_at = Some(
            std::time::Instant::now() - (REALM_JOIN_SETTLE + std::time::Duration::from_secs(1)),
        );
    }

    #[test]
    fn keyper_spawn_taunt_notifies() {
        let mut processor = PacketProcessor::new_for_test(AccountData::new());
        enter_realm_settled(&mut processor);
        processor.dispatch_event(keyper_taunt(
            "Hands off those crystals! I need them to scrounge up more keys!",
        ));
        assert_eq!(keyper_events(&processor), (0, 1));
    }

    #[test]
    fn tower_respawn_taunt_notifies_once_until_keyper_spawns() {
        let mut processor = PacketProcessor::new_for_test(AccountData::new());
        enter_realm_settled(&mut processor);
        let taunt = "Ah, there we go! Let\u{2019}s see those lowlifes try to take down my crystals this time!";
        processor.dispatch_event(keyper_taunt(taunt));
        // Re-broadcast of the same tower cue is latched away.
        processor.dispatch_event(keyper_taunt(taunt));
        assert_eq!(keyper_events(&processor), (1, 0));

        // The Keyper spawning re-arms the tower latch for the next wave.
        processor.dispatch_event(keyper_taunt("Wha- Again? REALLY?!"));
        processor.dispatch_event(keyper_taunt(taunt));
        assert_eq!(keyper_events(&processor), (2, 1));
    }

    #[test]
    fn keyper_taunt_within_join_settle_is_suppressed() {
        let mut processor = PacketProcessor::new_for_test(AccountData::new());
        processor.dispatch_event(map_changed(100)); // just entered: gate armed
        processor.dispatch_event(keyper_taunt("Wha- Again? REALLY?!"));
        assert_eq!(keyper_events(&processor), (0, 0));
    }

    #[test]
    fn in_fight_banter_does_not_notify() {
        let mut processor = PacketProcessor::new_for_test(AccountData::new());
        enter_realm_settled(&mut processor);
        processor.dispatch_event(keyper_taunt("Ack, get away from me!"));
        assert_eq!(keyper_events(&processor), (0, 0));
    }

    #[test]
    fn different_realm_resets_tower_latch() {
        let mut processor = PacketProcessor::new_for_test(AccountData::new());
        enter_realm_settled(&mut processor);
        let taunt = "Ah, there we go! see my crystals";
        processor.dispatch_event(keyper_taunt(taunt));
        // Entering a genuinely different realm re-arms the latch.
        processor.dispatch_event(map_changed_realm(100, "NexusPortal.Other"));
        processor.realm_entered_at = Some(
            std::time::Instant::now() - (REALM_JOIN_SETTLE + std::time::Duration::from_secs(1)),
        );
        processor.dispatch_event(keyper_taunt(taunt));
        assert_eq!(keyper_events(&processor), (2, 0));
    }

    #[test]
    fn returning_to_same_realm_keeps_tower_latch() {
        let mut processor = PacketProcessor::new_for_test(AccountData::new());
        enter_realm_settled(&mut processor);
        let taunt = "Ah, there we go! see my crystals";
        processor.dispatch_event(keyper_taunt(taunt));
        // Duck out to a dungeon (max_realm_score == 0) then return to the SAME
        // realm: the still-active tower wave must not re-notify.
        processor.dispatch_event(map_changed_realm(0, "Snake Pit"));
        processor.dispatch_event(map_changed_realm(100, "NexusPortal.Mesa"));
        processor.realm_entered_at = Some(
            std::time::Instant::now() - (REALM_JOIN_SETTLE + std::time::Duration::from_secs(1)),
        );
        processor.dispatch_event(keyper_taunt(taunt));
        assert_eq!(keyper_events(&processor), (1, 0));
    }
}

#[cfg(test)]
mod key_pop_tests {
    use super::{extract_json_string_field, PacketProcessor};
    use crate::processing::contract::UiPayload;
    use realmhound_core::vault::AccountData;
    use realmhound_core::GameEvent;

    #[test]
    fn extracts_player_field_from_notification_message() {
        let msg = r#"{"player":"Alice","dungeon":"Snake Pit"}"#;
        assert_eq!(
            extract_json_string_field(msg, "player").as_deref(),
            Some("Alice")
        );
    }

    #[test]
    fn extracts_player_field_with_whitespace_after_colon() {
        let msg = r#"{"player": "Bob"}"#;
        assert_eq!(
            extract_json_string_field(msg, "player").as_deref(),
            Some("Bob")
        );
    }

    #[test]
    fn returns_none_when_field_absent() {
        let msg = r#"{"dungeon":"Snake Pit"}"#;
        assert!(extract_json_string_field(msg, "player").is_none());
    }

    fn key_pops(processor: &PacketProcessor) -> usize {
        processor
            .updates
            .iter()
            .filter(|u| matches!(u.payload, UiPayload::KeyPop { .. }))
            .count()
    }

    #[test]
    fn portal_open_with_player_emits_key_pop() {
        let mut processor = PacketProcessor::new_for_test(AccountData::new());
        processor.dispatch_event(GameEvent::PortalOpened {
            message: r#"{"player":"Carol"}"#.to_string(),
            picture_type: 0,
        });
        assert_eq!(
            key_pops(&processor),
            1,
            "a player-tagged portal open is a key pop"
        );
    }

    #[test]
    fn portal_open_without_player_is_ignored() {
        let mut processor = PacketProcessor::new_for_test(AccountData::new());
        processor.dispatch_event(GameEvent::PortalOpened {
            message: r#"{"dungeon":"Realm"}"#.to_string(),
            picture_type: 0,
        });
        assert_eq!(
            key_pops(&processor),
            0,
            "realm/other portal opens are not key pops"
        );
    }

    #[test]
    fn disabled_tier_suppresses_key_pop() {
        // Snake Pit (2.5) is an Adept dungeon; disabling that tier must drop
        // both the sound and the feed entry. picture_type 0 resolves to no
        // name, so drive classification through a known object id if assets are
        // present; otherwise this still exercises the None-tier (always allow)
        // path. We assert via the filter helper directly to stay asset-free.
        use realmhound_core::assets::{key_pop_tier, KeyPopTier};
        use realmhound_core::settings::KeyPopTierFilter;

        assert_eq!(key_pop_tier("Snake Pit Portal"), Some(KeyPopTier::Adept));

        let mut filter = KeyPopTierFilter::default();
        assert!(
            !filter.allows(Some(KeyPopTier::Adept)),
            "adept off by default"
        );
        assert!(
            filter.allows(Some(KeyPopTier::Expert)),
            "expert on by default"
        );
        filter.adept = true;
        assert!(filter.allows(Some(KeyPopTier::Adept)), "adept enabled");
        filter.adept = false;
        assert!(
            !filter.allows(Some(KeyPopTier::Adept)),
            "disabled tier is filtered"
        );
        assert!(filter.allows(None), "unknown dungeons always allowed");
    }
}

#[cfg(test)]
mod area_unlock_tests {
    use super::{parse_area_unlock, parse_area_unlock_json, AreaUnlockMsg, PacketProcessor};

    #[test]
    fn parses_wine_cellar_single() {
        match parse_area_unlock("Wine Cellar unlocked by Pillar") {
            Some(AreaUnlockMsg::Single {
                short,
                popper,
                title,
                ..
            }) => {
                assert_eq!(short, "inc");
                assert_eq!(popper, "Pillar");
                assert_eq!(title, "Wine Cellar unlocked");
            }
            other => panic!("expected Single, got {other:?}"),
        }
    }

    #[test]
    fn parses_void_single() {
        match parse_area_unlock("The Void unlocked by Lovens") {
            Some(AreaUnlockMsg::Single { short, popper, .. }) => {
                assert_eq!(short, "vial");
                assert_eq!(popper, "Lovens");
            }
            other => panic!("expected Single, got {other:?}"),
        }
    }

    #[test]
    fn parses_each_monument() {
        for (name, idx) in [("Sword", 0usize), ("Shield", 1), ("Helmet", 2)] {
            let text = format!("The {name} Monument has been activated by Alice");
            match parse_area_unlock(&text) {
                Some(AreaUnlockMsg::Monument { index, popper }) => {
                    assert_eq!(index, idx, "{name} maps to index {idx}");
                    assert_eq!(popper, "Alice");
                }
                other => panic!("expected Monument, got {other:?}"),
            }
        }
    }

    #[test]
    fn ignores_unrelated_text() {
        assert!(parse_area_unlock("Alice has joined your guild").is_none());
        assert!(parse_area_unlock("The Shatters Monument is not a thing").is_none());
    }

    #[test]
    fn monument_callout_forms() {
        let s = |a: &str, b: &str| (a.to_string(), b.to_string());

        // No eligible contributors (only the local player popped) => no callout.
        assert_eq!(PacketProcessor::monument_callout(&[]), "");

        // Exactly one rune from one player => singular, lowercased rune type.
        assert_eq!(
            PacketProcessor::monument_callout(&[s("sword", "Greenmun")]),
            "Thanks Greenmun for the sword rune!"
        );

        // One player, multiple runes => plural.
        assert_eq!(
            PacketProcessor::monument_callout(&[s("sword", "Greenmun"), s("shield", "Greenmun")]),
            "Thanks Greenmun for the runes!"
        );

        // Multiple distinct players => plural, de-duplicated, order preserved.
        assert_eq!(
            PacketProcessor::monument_callout(&[
                s("sword", "Alice"),
                s("shield", "Bob"),
                s("helmet", "Alice"),
            ]),
            "Thanks Alice, Bob for the runes!"
        );
    }

    // --- ServerMessage notification path (the real carrier) ---

    #[test]
    fn parses_wine_cellar_notification() {
        let msg =
            r#"{"k":"s.dungeon_unlocked_by","t":{"name":"Wine Cellar","player":"Jeffryjhon",}}"#;
        match parse_area_unlock_json(msg) {
            Some(AreaUnlockMsg::Single {
                short,
                popper,
                title,
                ..
            }) => {
                assert_eq!(short, "inc");
                assert_eq!(popper, "Jeffryjhon");
                assert_eq!(title, "Wine Cellar unlocked");
            }
            other => panic!("expected Single, got {other:?}"),
        }
    }

    #[test]
    fn parses_void_notification() {
        let msg = r#"{"k":"s.dungeon_unlocked_by","t":{"name":"The Void","player":"Lovens",}}"#;
        match parse_area_unlock_json(msg) {
            Some(AreaUnlockMsg::Single { short, popper, .. }) => {
                assert_eq!(short, "vial");
                assert_eq!(popper, "Lovens");
            }
            other => panic!("expected Single, got {other:?}"),
        }
    }

    #[test]
    fn parses_each_monument_notification() {
        for (rune, idx) in [("Sword", 0usize), ("Shield", 1), ("Helmet", 2)] {
            let msg = format!(
                r#"{{"k":"s.something_by_player","t":{{"name":"The {rune} Monument has been activated","player":"MsXepher",}}}}"#
            );
            match parse_area_unlock_json(&msg) {
                Some(AreaUnlockMsg::Monument { index, popper }) => {
                    assert_eq!(index, idx, "{rune} maps to index {idx}");
                    assert_eq!(popper, "MsXepher");
                }
                other => panic!("expected Monument, got {other:?}"),
            }
        }
    }

    #[test]
    fn strips_player_discriminator_from_notification() {
        let msg = r#"{"k":"s.something_by_player","t":{"name":"The Sword Monument has been activated","player":"Tiffinia,1cf5",}}"#;
        match parse_area_unlock_json(msg) {
            Some(AreaUnlockMsg::Monument { index, popper }) => {
                assert_eq!(index, 0);
                assert_eq!(
                    popper, "Tiffinia",
                    "the ,<discriminator> suffix is stripped"
                );
            }
            other => panic!("expected Monument, got {other:?}"),
        }
    }

    #[test]
    fn ignores_unrelated_notification() {
        // A ServerMessage we don't care about must not be treated as an unlock.
        let msg = r#"{"k":"s.gift_chest","t":{"name":"giftChestOccupied","player":"Bob",}}"#;
        assert!(parse_area_unlock_json(msg).is_none());
        // Unknown dungeon name yields no pop.
        let msg = r#"{"k":"s.dungeon_unlocked_by","t":{"name":"Snake Pit","player":"Bob",}}"#;
        assert!(parse_area_unlock_json(msg).is_none());
    }
}

#[cfg(test)]
mod remote_death_tests {
    use super::parse_remote_death;

    #[test]
    fn parses_environmental_death() {
        let (name, killer) =
            parse_remote_death("", "Yumiho died at level 20, killed by Plagued Honey").unwrap();
        assert_eq!(name, "Yumiho");
        assert_eq!(killer, "Plagued Honey");
    }

    #[test]
    fn parses_boss_death_with_trailing_period() {
        let (name, killer) =
            parse_remote_death("", "Zannmp died at level 20, killed by Killer Bee Queen.").unwrap();
        assert_eq!(name, "Zannmp");
        assert_eq!(killer, "Killer Bee Queen");
    }

    #[test]
    fn falls_back_to_sender_when_text_starts_with_died() {
        let (name, _killer) =
            parse_remote_death("Bob", "died at level 5, killed by a Spider").unwrap();
        assert_eq!(name, "Bob");
    }

    #[test]
    fn ignores_unrelated_text() {
        assert!(parse_remote_death("Alice", "Alice: hello there").is_none());
        assert!(parse_remote_death("", "Gained +109 Red Dust!").is_none());
    }
}

#[cfg(test)]
mod isolation_gate_tests {
    use super::{PacketProcessor, PendingCredential, IDENTITY_TIMEOUT_MS, PENDING_EVENT_CAP};
    use crate::processing::contract::{AccountOperationScope, ControlMsg, UiPayload};
    use realmhound_core::account::{AccountId, AccountKey};
    use realmhound_core::protocol::data::{
        ObjectData, ObjectStatusData, StatData, StatType, WorldPosData,
    };
    use realmhound_core::protocol::packets::UpdatePacket;
    use realmhound_core::session::MainAccountStatus;
    use realmhound_core::stream::ConnectionKey;
    use realmhound_core::vault::AccountData;
    use realmhound_core::GameEvent;
    use std::net::Ipv4Addr;

    fn string_stat(stat_type: StatType, value: &str) -> StatData {
        StatData {
            stat_type_id: 0,
            stat_type,
            stat_value: 0,
            stat_value_two: 0,
            string_stat_value: Some(value.to_string()),
        }
    }

    /// An `Update` whose player object carries the given account id + name.
    fn identity_update(player_id: i32, account_id: &str, name: &str) -> GameEvent {
        let obj = ObjectData {
            object_type: 0,
            status: ObjectStatusData {
                object_id: player_id,
                pos: WorldPosData { x: 0.0, y: 0.0 },
                stats: vec![
                    string_stat(StatType::AccountId, account_id),
                    string_stat(StatType::Name, name),
                ],
            },
        };
        GameEvent::UpdateReceived(
            UpdatePacket {
                pos: WorldPosData { x: 0.0, y: 0.0 },
                level_type: 0,
                tiles: Vec::new(),
                new_objects: vec![obj],
                drops: Vec::new(),
            },
            0,
        )
    }

    fn map_changed(name: &str) -> GameEvent {
        GameEvent::MapChanged {
            name: name.to_string(),
            display_name: name.to_string(),
            realm_name: String::new(),
            fp: 1,
            current_realm_score: 0,
            max_realm_score: 0,
            allows_api: true,
            is_dungeon: false,
            dungeon_modifiers: Vec::new(),
            dungeon_grade: None,
        }
    }

    fn map_broadcasts(p: &PacketProcessor) -> usize {
        p.updates
            .iter()
            .filter(|u| {
                matches!(
                    &u.payload,
                    UiPayload::Broadcast(GameEvent::MapChanged { .. })
                )
            })
            .count()
    }

    fn update_broadcasts(p: &PacketProcessor) -> usize {
        p.updates
            .iter()
            .filter(|u| {
                matches!(
                    &u.payload,
                    UiPayload::Broadcast(GameEvent::UpdateReceived(..))
                )
            })
            .count()
    }

    /// Puts the processor into the "known main, but current connection is
    /// tentative (unverified)" state -- exactly the window a mule's socket sits
    /// in each time it changes maps.
    fn tentative_with_saved_main(account_id: &str) -> PacketProcessor {
        let mut p = PacketProcessor::new_for_test(AccountData::new());
        p.session.connection.saved_account_id = Some(account_id.to_string());
        p.session.connection.main_connection = Some(ConnectionKey::new(
            Ipv4Addr::new(127, 0, 0, 1),
            1000,
            Ipv4Addr::new(127, 0, 0, 1),
            2050,
        ));
        p.session.connection.pending_account_verification = true;
        p.session.player.object_id = Some(1);
        p
    }

    /// A tentative connection's location must be buffered, not leaked to the UI.
    #[test]
    fn tentative_map_change_is_buffered_not_broadcast() {
        let mut p = tentative_with_saved_main("MAIN123");
        p.dispatch_event(map_changed("Mule Vault"));
        assert_eq!(
            map_broadcasts(&p),
            0,
            "mule location must not reach the toolbar"
        );
        assert!(
            !p.pending_events.is_empty(),
            "location is buffered pending verification"
        );
    }

    /// When the tentative connection verifies as the main, its buffered location
    /// replays so the real main's toolbar updates, and its name is committed.
    #[test]
    fn verifying_main_replays_buffered_location_and_name() {
        let mut p = tentative_with_saved_main("MAIN123");
        p.dispatch_event(map_changed("Main Nexus"));
        assert_eq!(map_broadcasts(&p), 0);

        p.dispatch_event(identity_update(1, "MAIN123", "MainToon"));

        assert!(p.session.connection.is_account_verified(), "main verified");
        assert_eq!(
            map_broadcasts(&p),
            1,
            "buffered main location replays to the toolbar"
        );
        assert_eq!(
            update_broadcasts(&p),
            1,
            "the verifying Update is re-dispatched so the main's own objects/loot are not lost"
        );
        assert!(p.pending_events.is_empty(), "buffer consumed on verify");
        assert_eq!(
            p.session.connection.detected_account_name.as_deref(),
            Some("MainToon"),
            "verified main's name is committed"
        );
    }

    /// A mismatching (mule) connection discards the buffered location and never
    /// leaks its name -- no toolbar or widget-bar flicker.
    #[test]
    fn mismatching_mule_discards_buffer_and_name() {
        let mut p = tentative_with_saved_main("MAIN123");
        p.dispatch_event(map_changed("Mule Vault"));

        p.dispatch_event(identity_update(1, "MULE999", "MuleToon"));

        assert_eq!(map_broadcasts(&p), 0, "mule location never broadcasts");
        assert_eq!(
            update_broadcasts(&p),
            0,
            "mule Update is never re-dispatched"
        );
        assert!(
            p.pending_events.is_empty(),
            "buffered mule location is discarded"
        );
        assert_eq!(
            p.session.connection.detected_account_name, None,
            "mule name never leaks to the widget bar"
        );
        assert!(
            p.session.connection.main_connection.is_none(),
            "mule connection is dropped as main"
        );
    }

    // --- Candidate lifecycle, identity, and scoped async completions ---

    fn main_conn() -> ConnectionKey {
        ConnectionKey::new(
            Ipv4Addr::new(127, 0, 0, 1),
            1000,
            Ipv4Addr::new(127, 0, 0, 1),
            2050,
        )
    }

    fn access_tokens(p: &PacketProcessor) -> Vec<String> {
        p.updates
            .iter()
            .filter_map(|u| match &u.payload {
                UiPayload::AccessTokenCaptured(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    fn party_cleared_count(p: &PacketProcessor) -> usize {
        p.updates
            .iter()
            .filter(|u| matches!(u.payload, UiPayload::PartyCleared))
            .count()
    }

    fn scope(key: AccountKey, id: &AccountId, generation: u64) -> AccountOperationScope {
        AccountOperationScope {
            account_key: key,
            expected_account_id: id.clone(),
            generation,
        }
    }

    /// A verifying main replays its buffered events in arrival order.
    #[test]
    fn candidate_buffer_replays_in_order_on_verify() {
        let mut p = tentative_with_saved_main("MAIN123");
        p.dispatch_event(map_changed("First"));
        p.dispatch_event(map_changed("Second"));
        p.dispatch_event(map_changed("Third"));
        assert_eq!(p.pending_events.len(), 3);

        p.dispatch_event(identity_update(1, "MAIN123", "MainToon"));

        let names: Vec<String> = p
            .updates
            .iter()
            .filter_map(|u| match &u.payload {
                UiPayload::Broadcast(GameEvent::MapChanged { name, .. }) => Some(name.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["First", "Second", "Third"]);
        assert!(p.pending_events.is_empty());
    }

    /// Overflow drops the whole candidate and frees the slot without ignoring the
    /// socket, so a slow-but-legitimate main can still recover on its next HELLO.
    #[test]
    fn candidate_overflow_discards_whole_candidate_and_frees_slot() {
        let mut p = tentative_with_saved_main("MAIN123");
        p.pending_credential = Some(PendingCredential {
            connection: main_conn(),
            token: "tok".into(),
        });
        for _ in 0..PENDING_EVENT_CAP {
            p.dispatch_event(map_changed("Buffered"));
        }
        assert_eq!(p.pending_events.len(), PENDING_EVENT_CAP);

        p.dispatch_event(map_changed("Overflow"));

        assert!(
            p.pending_events.is_empty(),
            "whole candidate dropped, no partial replay"
        );
        assert!(p.pending_credential.is_none(), "captured token dropped");
        assert!(
            p.session.connection.main_connection.is_none(),
            "main slot freed for retry"
        );
        assert_eq!(
            p.session.connection.main_account_status,
            MainAccountStatus::WaitingForConnection
        );
        assert_eq!(
            party_cleared_count(&p),
            0,
            "overflow is not a positive mismatch"
        );
        assert!(access_tokens(&p).is_empty(), "no token leaked");
    }

    /// A candidate that never identifies itself is discarded after the timeout.
    #[test]
    fn identity_timeout_discards_unverified_candidate() {
        let mut p = tentative_with_saved_main("MAIN123");
        p.session.connection.candidate_since_ms = Some(1_000);
        p.pending_credential = Some(PendingCredential {
            connection: main_conn(),
            token: "tok".into(),
        });
        p.dispatch_event(map_changed("Buffered"));

        assert!(
            !p.check_identity_timeout(1_000 + IDENTITY_TIMEOUT_MS - 1),
            "not yet expired"
        );
        assert_eq!(p.pending_events.len(), 1);

        assert!(
            p.check_identity_timeout(1_000 + IDENTITY_TIMEOUT_MS),
            "expired"
        );
        assert!(p.pending_events.is_empty());
        assert!(p.pending_credential.is_none());
        assert!(p.session.connection.main_connection.is_none());
    }

    /// A verified main is never timed out, even with a stale candidate timer.
    #[test]
    fn verified_main_never_times_out() {
        let mut p = tentative_with_saved_main("MAIN123");
        p.dispatch_event(identity_update(1, "MAIN123", "MainToon"));
        assert!(p.session.connection.is_account_verified());

        p.session.connection.candidate_since_ms = Some(0);
        assert!(!p.check_identity_timeout(i64::MAX / 2));
        assert!(p.session.connection.is_account_verified());
    }

    /// A token captured from a candidate that turns out to be a mule is dropped
    /// and never released to the UI.
    #[test]
    fn captured_token_is_dropped_when_candidate_mismatches() {
        let mut p = tentative_with_saved_main("MAIN123");
        p.pending_credential = Some(PendingCredential {
            connection: main_conn(),
            token: "mule-tok".into(),
        });

        p.dispatch_event(identity_update(1, "MULE999", "MuleToon"));

        assert!(
            p.pending_credential.is_none(),
            "foreign candidate's token dropped"
        );
        assert!(
            access_tokens(&p).is_empty(),
            "mule token never released to the UI"
        );
    }

    /// A captured token is only released once its candidate verifies as the main.
    #[test]
    fn captured_token_is_released_only_after_verification() {
        let mut p = tentative_with_saved_main("MAIN123");
        p.pending_credential = Some(PendingCredential {
            connection: main_conn(),
            token: "main-tok".into(),
        });

        p.dispatch_event(map_changed("Nexus"));
        assert!(
            access_tokens(&p).is_empty(),
            "token withheld while unverified"
        );

        p.dispatch_event(identity_update(1, "MAIN123", "MainToon"));
        assert_eq!(access_tokens(&p), vec!["main-tok".to_string()]);
        assert!(p.pending_credential.is_none());
    }

    /// After a restart selecting a new main, an old (now-foreign) account that is
    /// still connected is treated as a mismatch and never mutates state.
    #[test]
    fn previously_selected_account_is_ignored_after_selection_change() {
        let mut p = tentative_with_saved_main("NEWMAIN");
        p.dispatch_event(map_changed("Old Account Nexus"));

        p.dispatch_event(identity_update(1, "OLDMAIN", "OldToon"));

        assert!(
            p.session.connection.main_connection.is_none(),
            "old account rejected as main"
        );
        assert_eq!(p.session.connection.detected_account_name, None);
        assert!(p.pending_events.is_empty());
    }

    /// Out-of-order same-account mission completions apply newest-first only.
    #[test]
    fn mission_completions_apply_in_generation_order() {
        let key = AccountKey::generate();
        let id = AccountId::new("MAIN123").unwrap();
        let mut p = PacketProcessor::new_for_test(AccountData::new());
        p.set_selected_scope(key, id.clone());

        p.apply_control(ControlMsg::ApplyMissionData {
            defs: None,
            progress: None,
            scope: scope(key, &id, 2),
        });
        assert_eq!(p.last_mission_gen, 2);

        p.apply_control(ControlMsg::ApplyMissionData {
            defs: None,
            progress: None,
            scope: scope(key, &id, 1),
        });
        assert_eq!(p.last_mission_gen, 2, "older generation rejected");

        p.apply_control(ControlMsg::ApplyMissionData {
            defs: None,
            progress: None,
            scope: scope(key, &id, 3),
        });
        assert_eq!(p.last_mission_gen, 3, "newer generation applied");
    }

    /// A completion whose scope names a different account (key or server id) is
    /// rejected without touching account-scoped state.
    #[test]
    fn non_selected_account_completion_is_rejected() {
        let key = AccountKey::generate();
        let id = AccountId::new("MAIN123").unwrap();
        let mut p = PacketProcessor::new_for_test(AccountData::new());
        p.set_selected_scope(key, id.clone());

        let other_key = AccountKey::generate();
        p.apply_control(ControlMsg::ApplyMissionData {
            defs: None,
            progress: None,
            scope: scope(other_key, &id, 5),
        });
        assert_eq!(p.last_mission_gen, 0, "foreign account key rejected");

        let other_id = AccountId::new("MULE999").unwrap();
        p.apply_control(ControlMsg::ApplyMissionData {
            defs: None,
            progress: None,
            scope: scope(key, &other_id, 5),
        });
        assert_eq!(p.last_mission_gen, 0, "foreign server id rejected");
    }

    /// A stale API account-data completion (older generation) is rejected.
    #[test]
    fn stale_api_account_completion_is_rejected() {
        let key = AccountKey::generate();
        let id = AccountId::new("MAIN123").unwrap();
        let mut p = PacketProcessor::new_for_test(AccountData::new());
        p.set_selected_scope(key, id.clone());

        let data = realmhound_core::api::AccountData {
            account_id: Some("MAIN123".into()),
            ..Default::default()
        };
        p.apply_control(ControlMsg::ApplyApiAccountData(
            data.clone(),
            scope(key, &id, 2),
        ));
        assert_eq!(p.last_account_gen, 2);

        p.apply_control(ControlMsg::ApplyApiAccountData(data, scope(key, &id, 1)));
        assert_eq!(p.last_account_gen, 2, "older API completion rejected");
    }
}

#[cfg(test)]
mod new_selected_tests {
    use std::sync::{Arc, RwLock};

    use realmhound_core::combat::CombatDatabase;
    use realmhound_core::loot::LootDatabase;
    use realmhound_core::vault::{AccountData, AccountDataRepository};
    use realmhound_core::Settings;

    use super::{PacketProcessor, ProcessorInitError};

    fn settings() -> Arc<RwLock<Settings>> {
        Arc::new(RwLock::new(Settings::default()))
    }

    #[test]
    fn opens_writers_before_readers_and_persists() {
        let temp = tempfile::tempdir().unwrap();
        let loot_path = temp.path().join("databases").join("loot_history.db");
        let combat_path = temp.path().join("databases").join("combat_history.db");
        let repo = AccountDataRepository::for_profile(temp.path().join("account_data.json"));

        let processor = PacketProcessor::new_selected(
            settings(),
            AccountData::new(),
            repo,
            loot_path.clone(),
            combat_path.clone(),
            Some("ACCT-1".to_string()),
            None,
        )
        .expect("writers open for a valid profile");

        // The writers created both databases; a reader can now open each, proving
        // writer-before-reader ordering holds.
        assert!(combat_path.exists());
        assert!(loot_path.exists());
        assert!(CombatDatabase::open_reader_at(&combat_path).is_ok());
        assert!(LootDatabase::open_reader_at(&loot_path).is_ok());
        drop(processor);
    }

    #[test]
    fn propagates_combat_writer_failure_without_in_memory_fallback() {
        let temp = tempfile::tempdir().unwrap();
        // Make the combat database path unopenable by planting a directory there.
        let combat_path = temp.path().join("combat_history.db");
        std::fs::create_dir_all(&combat_path).unwrap();
        let loot_path = temp.path().join("loot_history.db");
        let repo = AccountDataRepository::for_profile(temp.path().join("account_data.json"));

        let error = match PacketProcessor::new_selected(
            settings(),
            AccountData::new(),
            repo,
            loot_path,
            combat_path,
            None,
            None,
        ) {
            Ok(_) => panic!("a broken combat writer must not degrade to in-memory"),
            Err(error) => error,
        };
        let ProcessorInitError::Database { label, .. } = error;
        assert_eq!(label, "combat");
    }
}

#[cfg(test)]
mod shutdown_tests {
    use std::sync::{Arc, RwLock};

    use realmhound_core::combat::CombatDatabase;
    use realmhound_core::vault::{AccountData, AccountDataRepository};
    use realmhound_core::Settings;

    use super::PacketProcessor;
    use crate::processing::contract::{DbCloseError, ShutdownReport};

    fn settings() -> Arc<RwLock<Settings>> {
        Arc::new(RwLock::new(Settings::default()))
    }

    fn paths(root: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
        (
            root.join("databases").join("loot_history.db"),
            root.join("databases").join("combat_history.db"),
        )
    }

    #[test]
    fn final_drain_then_save_persists_account_data() {
        let temp = tempfile::tempdir().unwrap();
        let (loot_path, combat_path) = paths(temp.path());
        let account_json = temp.path().join("account_data.json");
        let repo = AccountDataRepository::for_profile(account_json.clone());

        let mut processor = PacketProcessor::new_selected(
            settings(),
            AccountData::new(),
            repo,
            loot_path,
            combat_path,
            None,
            None,
        )
        .unwrap();

        // Drain without a live tick; the save must land on disk before relaunch.
        processor.final_drain();
        assert!(processor.save_account_on_exit().is_ok());
        assert!(account_json.exists());
    }

    #[test]
    fn account_save_failure_is_surfaced() {
        let temp = tempfile::tempdir().unwrap();
        let (loot_path, combat_path) = paths(temp.path());
        // Plant a FILE where the parent dir must be, so create_dir_all fails.
        let blocker = temp.path().join("blocker");
        std::fs::write(&blocker, b"x").unwrap();
        let account_json = blocker.join("account_data.json");
        let repo = AccountDataRepository::for_profile(account_json);

        let mut processor = PacketProcessor::new_selected(
            settings(),
            AccountData::new(),
            repo,
            loot_path,
            combat_path,
            None,
            None,
        )
        .unwrap();

        processor.final_drain();
        let persistence = processor.save_account_on_exit();
        assert!(persistence.is_err());

        // A failed save yields a non-success report, blocking relaunch.
        let report = ShutdownReport::from_results(persistence, Ok(()));
        assert!(matches!(report, ShutdownReport::PersistenceFailed(_)));
        assert!(!report.is_success());
    }

    #[test]
    fn close_databases_truncates_wal_and_reader_still_opens() {
        let temp = tempfile::tempdir().unwrap();
        let (loot_path, combat_path) = paths(temp.path());
        let repo = AccountDataRepository::for_profile(temp.path().join("account_data.json"));

        let mut processor = PacketProcessor::new_selected(
            settings(),
            AccountData::new(),
            repo,
            loot_path,
            combat_path.clone(),
            None,
            None,
        )
        .unwrap();

        assert!(processor.close_databases().is_ok());

        // After a TRUNCATE checkpoint + close, the -wal tail is gone or empty.
        let wal = combat_path.with_file_name("combat_history.db-wal");
        if wal.exists() {
            assert_eq!(std::fs::metadata(&wal).unwrap().len(), 0);
        }
        assert!(CombatDatabase::open_reader_at(&combat_path).is_ok());
    }

    #[test]
    fn db_close_failure_is_surfaced() {
        // A manufactured close error maps to a non-success report; `from_results`
        // is the real epilogue path (no test-only production seam).
        let report = ShutdownReport::from_results(
            Ok(()),
            Err(DbCloseError::Combat("disk I/O error".to_string())),
        );
        assert!(matches!(report, ShutdownReport::DatabaseCloseFailed(_)));
        assert!(!report.is_success());
    }
}

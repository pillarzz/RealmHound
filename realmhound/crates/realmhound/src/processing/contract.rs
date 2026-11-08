//! Contract types shared between the packet `PacketProcessor` and the UI.
//!
//! In Session 1 these types are exchanged synchronously (the UI drives the
//! processor from `update()` and applies the returned updates immediately).
//! Session 2 moves the processor onto a worker thread and wires these same
//! types through channels, so the shapes are designed to be fully self-contained
//! (the UI never re-reads live model state to interpret an update).
//!
//! Some fields/variants (e.g. `UiUpdate::seq`, `ViewState` connection fields,
//! `ControlMsg::ClearIgnored`/`SetLootTrackingSettings`) are part of the
//! Session 2 contract and are not yet consumed in the single-threaded path.
#![allow(dead_code)]

use realmhound_core::{
    account::{AccountId, AccountKey},
    api::AccountData as ApiAccountData,
    api::{ClientSeasons, PlayerMissions},
    capture::PacketReceiver,
    loot::{LootBagType, ProcessedLootDrop},
    protocol::QuestFetchResponsePacket,
    session::MainAccountStatus,
    settings::{CombatHistorySettings, LootTrackingSettings},
    stream::CaptureHealth,
    vault::{AccountData, LiveVaultData},
    GameEvent,
};

use crate::panels::chat::ChatMessage;
use crate::processing::mission_tracker::MissionView;
use crate::sound::{EventSound, SoundType};
use chrono::{DateTime, Utc};
use std::sync::Arc;
#[cfg(feature = "latency-diagnostics")]
use std::time::Instant;

// ---------------------------------------------------------------------------
// UiUpdate (worker -> UI, single ordered stream, monotonic `seq`)
// ---------------------------------------------------------------------------

/// A single, fully-resolved update emitted by the processor for the UI.
///
/// `seq` is a monotonically increasing sequence number establishing a total
/// order over the stream. In Session 1 the updates are returned (and applied)
/// in order, so `seq` is informational; Session 2's threaded channel relies on
/// it for ordering.
pub struct UiUpdate {
    /// Monotonic sequence number.
    pub seq: u64,
    /// Monotonic timestamp recorded at worker-to-UI queue insertion.
    #[cfg(feature = "latency-diagnostics")]
    pub enqueued_at: Option<Instant>,
    /// The update payload.
    pub payload: UiPayload,
}

/// The payload of a [`UiUpdate`].
pub enum UiPayload {
    /// Re-broadcast a domain event. No panel consumes it any more; every panel
    /// that matters is driven by the snapshot. Kept as the replay signal the
    /// processor's buffering tests assert on.
    Broadcast(GameEvent),

    /// Automatic clipboard work, consumed by the worker before UI delivery.
    DungeonCallout {
        display_name: String,
        modifiers: Vec<String>,
        grade: Option<String>,
        origin: Option<DungeonOrigin>,
    },

    /// A resolved chat message to append (whisper direction already tagged).
    Chat(ChatMessage),
    /// An encounter/boss callout for the live feed.
    BossCall { boss_id: i32, text: String },

    /// A Keyper seasonal-event cue for the live feed. `towers` = true when a
    /// crystal-tower wave appeared, false when The Keyper boss spawned. Carries
    /// its own Live Feed entry + sound (independent of the realm-event system).
    KeyperEvent { towers: bool },

    /// A processed loot drop (already recorded to the DB by the processor).
    PushLoot(ProcessedLootDrop),
    /// At least one loot drop was recorded this batch (refresh autocomplete).
    LootBatchAdded,

    /// A dungeon key was popped nearby (public portal open). Carries the opener
    /// name, the dungeon key's object id (for the sprite), and the resolved
    /// dungeon name. Drives the key-open Live Feed entry.
    KeyPop {
        opener: String,
        key_id: i32,
        dungeon_name: String,
        thankable: bool,
    },

    /// A special "area unlock" to show in the Live Feed. Covers single-item pops
    /// (Wine Cellar Incantation, Vial of Pure Darkness) and the combined Lost
    /// Halls Rune monuments (shown on Oryx's Sanctuary entry). `contributions`
    /// pairs each popped item's object id (for the sprite) with its popper name;
    /// `callout` is the ready-to-copy "Thanks ..." clipboard string.
    AreaUnlock {
        title: String,
        contributions: Vec<(i32, String)>,
        callout: String,
    },

    /// A pet inventory slot changed (forward to the vault panel's API copy).
    UpdateVaultPetSlot {
        pet_instance_id: i32,
        seasonal: bool,
        slot: usize,
        item_id: i32,
        stack_count: u8,
    },

    /// The active pet changed (forward to the vault panel's API copy so its
    /// character keeps the correct pet instance id for live inventory updates).
    SetVaultActivePet { char_id: i32, instance_id: i32 },

    /// An access token was captured from a Hello packet.
    AccessTokenCaptured(String),

    /// A sound should be played (App owns the `SoundEngine` this session).
    Audio(AudioCommand),

    /// Oryx "MY MINIONS HAVE FAILED ME" detected while in dungeon.
    OryxLagWarning,

    /// A stored Combat History card changed without a new fight being recorded
    /// (a loot bag completing an escaped fight or an encounter run), which the
    /// panel's fight-count poll cannot see. The open view re-reads its summaries.
    CombatHistoryChanged,
}

// ---------------------------------------------------------------------------
// AudioCommand (bounded; owned by the audio thread in Session 2)
// ---------------------------------------------------------------------------

/// A command for the audio sink.
///
/// Session 1 plays these on the UI thread via the App-owned `SoundEngine`.
#[derive(Debug, Clone)]
pub enum AudioCommand {
    /// Play a specific sound.
    Play(SoundType),
    /// Play the configured notification sound for a loot bag tier.
    PlayForBag(LootBagType),
    /// Play the shiny-loot notification sound for the shiny item's own bag tier
    /// (see [`realmhound_core::loot::shiny_bag_tier`]).
    PlayForBagShiny(LootBagType),
    /// Play a realm-event notification sound at a specific volume (already the
    /// per-event level; the audio thread scales it by master). `custom_key`
    /// selects a per-boss custom override from `SoundSettings::custom_sounds`.
    PlayEvent {
        sound: EventSound,
        custom_key: String,
        /// Distinguishes discovered objects sharing one configured sound.
        /// None retains coalescing by the custom-sound key alone.
        instance_id: Option<i32>,
        volume: f32,
    },
    /// Play one custom-chat alert sound. `custom_key` is the alert's
    /// `custom_chat` / `custom_chat_{id}` key in `SoundSettings::custom_sounds`.
    PlayCustomChat { custom_key: String, volume: f32 },
    /// Set the output volume (Session 2 audio thread).
    SetVolume(f32),
}

// ---------------------------------------------------------------------------
// ControlMsg (UI -> worker)
// ---------------------------------------------------------------------------

/// Identifies the selected account an async completion was requested for, plus a
/// per-operation-kind monotonic generation. The worker applies a completion only
/// when the scope names the currently selected account (key + normalized server
/// id) and its generation is strictly newer than the last applied one, so stale
/// or foreign-account results can never mutate account-scoped state.
#[derive(Debug, Clone)]
pub struct AccountOperationScope {
    pub account_key: AccountKey,
    pub expected_account_id: AccountId,
    pub generation: u64,
}

/// A control message from the UI to the processor.
///
/// Not `Debug` because [`ControlMsg::StartCapture`] carries a non-`Debug`
/// [`PacketReceiver`]. Sent over a single ordered channel so capture
/// start/stop is ordered relative to model edits.
pub enum ControlMsg {
    /// Hand the worker a fresh capture source and reset session/reassembler.
    StartCapture(PacketReceiver),
    /// Drop the current capture source (stop draining packets).
    StopCapture,
    /// The first UI frame has completed; automatic history maintenance may run.
    StartHistoricalMaintenance,
    /// Whether the UI currently wants repaint wakes (false while minimized).
    SetLiveDemand(bool),
    /// Flush persistent state and exit the worker loop (app shutdown).
    Shutdown,
    /// Clear the reassembler's ignored-connection set.
    ClearIgnored,
    /// Start/stop recording raw packets to a `.rhcap` file (Debug settings).
    SetCaptureRecording(bool),
    /// Update loot-tracking capture settings.
    SetLootTrackingSettings(LootTrackingSettings),
    /// Update Combat History tracking settings (which boss groups are recorded).
    SetCombatHistorySettings(CombatHistorySettings),
    /// Set a custom label for a character (UI-origin edit).
    SetCharacterLabel { char_id: i32, label: String },
    /// Remove a (dead) character (UI-origin edit).
    RemoveCharacter(i32),
    /// Reorder a character within its tab (UI-origin drag).
    MoveCharacter {
        char_id: i32,
        to_index: usize,
        seasonal: bool,
    },
    /// Apply account data fetched from the Realm API (merges + bumps generation).
    ApplyApiAccountData(ApiAccountData, AccountOperationScope),
    /// Apply seasonal mission data fetched from the appspot HTTP endpoints
    /// (`getClientSeasons` definitions and/or `getPlayerMissions` progress).
    ApplyMissionData {
        defs: Option<ClientSeasons>,
        progress: Option<PlayerMissions>,
        scope: AccountOperationScope,
    },
    /// Replace the worker's watchlist snapshot after a UI edit.
    SetWatchlist(realmhound_core::watchlist::Watchlist),
}
// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Shutdown reporting (worker -> UI, graceful relaunch)
// ---------------------------------------------------------------------------

/// Failure closing a per-account SQLite database on shutdown. Captured as a
/// string so this layer needs no direct SQLite dependency.
#[derive(Debug, Clone)]
pub enum DbCloseError {
    /// The combat history database failed to checkpoint/close.
    Combat(String),
    /// The loot history database failed to checkpoint/close.
    Loot(String),
}

impl std::fmt::Display for DbCloseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbCloseError::Combat(e) => write!(f, "combat database close failed: {e}"),
            DbCloseError::Loot(e) => write!(f, "loot database close failed: {e}"),
        }
    }
}

impl std::error::Error for DbCloseError {}

/// Outcome of the worker's graceful shutdown, reported to the UI so a relaunch
/// only proceeds when persistence and database close both succeeded.
#[derive(Debug)]
pub enum ShutdownReport {
    /// Account data flushed and databases closed cleanly.
    Success,
    /// Saving account data failed; state on disk may be stale.
    PersistenceFailed(std::io::Error),
    /// A database failed to close cleanly.
    DatabaseCloseFailed(DbCloseError),
    /// The worker did not acknowledge shutdown within the timeout.
    TimedOut,
}

impl ShutdownReport {
    /// Whether shutdown fully succeeded (the only state that permits relaunch).
    pub fn is_success(&self) -> bool {
        matches!(self, ShutdownReport::Success)
    }

    /// Build a report from the epilogue's two fallible steps. Persistence is
    /// reported first: a failed save outranks a failed close.
    pub fn from_results(
        persistence: std::io::Result<()>,
        db_close: Result<(), DbCloseError>,
    ) -> Self {
        if let Err(e) = persistence {
            return ShutdownReport::PersistenceFailed(e);
        }
        if let Err(e) = db_close {
            return ShutdownReport::DatabaseCloseFailed(e);
        }
        ShutdownReport::Success
    }
}

/// Decide whether a relaunch may proceed. Every persistence step (worker
/// shutdown, settings, registry) must succeed; returns the reason of the first failure.
pub fn relaunch_preflight(
    shutdown: &ShutdownReport,
    settings_saved: Result<(), String>,
    registry_saved: Result<(), String>,
) -> Result<(), String> {
    if !shutdown.is_success() {
        return Err(format!("worker shutdown did not succeed: {shutdown:?}"));
    }
    settings_saved.map_err(|e| format!("settings save failed: {e}"))?;
    registry_saved.map_err(|e| format!("registry write failed: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod relaunch_report_tests {
    use super::{relaunch_preflight, DbCloseError, ShutdownReport};

    fn io_err() -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::Other, "boom")
    }

    #[test]
    fn from_results_reports_success_only_when_both_ok() {
        assert!(ShutdownReport::from_results(Ok(()), Ok(())).is_success());
    }

    #[test]
    fn from_results_prioritizes_persistence_failure() {
        let report =
            ShutdownReport::from_results(Err(io_err()), Err(DbCloseError::Loot("x".to_string())));
        assert!(matches!(report, ShutdownReport::PersistenceFailed(_)));
    }

    #[test]
    fn from_results_surfaces_db_close_failure() {
        let report =
            ShutdownReport::from_results(Ok(()), Err(DbCloseError::Combat("x".to_string())));
        assert!(matches!(report, ShutdownReport::DatabaseCloseFailed(_)));
    }

    #[test]
    fn preflight_allows_relaunch_when_everything_succeeds() {
        assert!(relaunch_preflight(&ShutdownReport::Success, Ok(()), Ok(())).is_ok());
    }

    #[test]
    fn preflight_blocks_relaunch_on_failed_shutdown() {
        assert!(relaunch_preflight(&ShutdownReport::TimedOut, Ok(()), Ok(())).is_err());
        assert!(
            relaunch_preflight(&ShutdownReport::PersistenceFailed(io_err()), Ok(()), Ok(()))
                .is_err()
        );
    }

    #[test]
    fn preflight_blocks_relaunch_on_registry_write_failure() {
        let result = relaunch_preflight(
            &ShutdownReport::Success,
            Ok(()),
            Err("registry disk full".to_string()),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("registry"));
    }

    #[test]
    fn preflight_blocks_relaunch_on_settings_failure() {
        let result = relaunch_preflight(
            &ShutdownReport::Success,
            Err("settings locked".to_string()),
            Ok(()),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("settings"));
    }
}

// ---------------------------------------------------------------------------
// ViewState (coalesced render snapshot)
// ---------------------------------------------------------------------------

/// Coalesced, latest-value snapshot of model state the UI reads each frame.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewState {
    /// Detected account name (live), if any.
    pub detected_account_name: Option<String>,
    /// Saved main account id, if any.
    pub saved_account_id: Option<String>,
    /// Main account connection status.
    pub main_account_status: MainAccountStatus,
    /// Whether the main account is verified (accepted and no longer pending
    /// account-id verification). A tentatively-accepted connection -- e.g. a
    /// mule whose Hello arrived before the main's -- is Connected but not yet
    /// verified.
    pub account_verified: bool,
    /// Number of ignored TCP connections (secondary clients).
    pub ignored_count: usize,
    /// Generation of the character view (bumps on any character-cache change).
    pub char_view_gen: u64,
    /// Generation of the vault view (bumps on any live-vault change).
    pub vault_view_gen: u64,
    /// `AccountData` generation (drives totals character-cache resync).
    pub account_generation: u64,
    /// Capture pipeline health indicator.
    pub capture_health: CaptureHealth,
    /// Gap-skip count (packet loss recovery events).
    pub capture_gap_skips: u64,
    /// Total bytes lost to gap-skips.
    pub capture_bytes_lost: u64,
    /// Number of cipher resyncs triggered.
    pub capture_resyncs: u64,
    /// Packets discarded while cipher was unsynced.
    pub capture_packets_dropped: u64,
    /// Cumulative time spent with cipher unsynced (ms).
    pub capture_unsync_ms: u64,
    /// Queue overflow drops (capture thread faster than consumer).
    pub capture_queue_drops: u64,
    /// Generation of the mission tracker (bumps on any mission-state change).
    pub mission_view_gen: u64,
    /// Generation of the released access token (bumps on a changed token).
    pub token_gen: u64,
    /// Generation of worker-owned party membership.
    pub party_rev: u64,
    /// Generation of worker-owned current encounters.
    pub encounter_rev: u64,
    /// Generation of worker-owned quest state.
    pub quest_rev: u64,
    /// Generation of the worker-owned recoverable Live Feed window.
    pub live_event_rev: u64,
    /// Bumps when the worker-side Live Feed context changes.
    pub feed_rev: u64,
    /// Remaining loot-drop-boost seconds for the live player, if a boost is
    /// active. Server-driven (already paused by the game in safe areas), shown
    /// on the live character card.
    pub loot_boost_secs: Option<u32>,
    /// Whether the current map pauses the boost timers: a safe area (the Nexus,
    /// Vault, Pet Yard, Guild Hall, Bazaar, Chess, Tinkerer Cave), or no map seen
    /// yet. The card must not count the loot boost down while this is set.
    pub map_pauses_boost_timers: bool,
}

impl ViewState {
    /// Initial snapshot for a freshly created session.
    pub fn new(detected_account_name: Option<String>, saved_account_id: Option<String>) -> Self {
        Self {
            detected_account_name,
            saved_account_id,
            main_account_status: MainAccountStatus::WaitingForConnection,
            account_verified: false,
            ignored_count: 0,
            char_view_gen: 0,
            vault_view_gen: 0,
            account_generation: 0,
            capture_health: CaptureHealth::Healthy,
            capture_gap_skips: 0,
            capture_bytes_lost: 0,
            capture_resyncs: 0,
            capture_packets_dropped: 0,
            capture_unsync_ms: 0,
            capture_queue_drops: 0,
            mission_view_gen: 0,
            token_gen: 0,
            party_rev: 0,
            encounter_rev: 0,
            quest_rev: 0,
            live_event_rev: 0,
            feed_rev: 0,
            loot_boost_secs: None,
            // Nothing heard yet, so nothing may be counted down.
            map_pauses_boost_timers: true,
        }
    }
}

// ---------------------------------------------------------------------------
// ModelSnapshot (worker -> UI shared latest-value model snapshot)
// ---------------------------------------------------------------------------

/// Worker-owned party membership. Watchlist status and world sightings stay in
/// the UI panel; this is the authoritative member list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PartyView {
    /// Party id from the party list packet.
    pub party_id: i32,
    /// Party description shown in the header.
    pub party_name: String,
    /// Maximum party size.
    pub max_size: u8,
    /// Current members.
    pub members: Vec<PartyMemberView>,
    /// Whether the local player is the party leader.
    pub is_local_leader: bool,
    /// The local (main) player's name, if known.
    pub local_player_name: Option<String>,
    /// Bumps on every membership change.
    pub rev: u64,
}

/// One party member as owned by the worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartyMemberView {
    /// Internal player id.
    pub player_id: i16,
    /// Player name.
    pub name: String,
    /// Whether this member is the party leader.
    pub is_leader: bool,
    /// Player class id from the last world update.
    pub class_id: Option<i32>,
    /// Skin, texture and guild from the last world update.
    pub skin_id: i32,
    /// Texture 1.
    pub tex1: u32,
    /// Texture 2.
    pub tex2: u32,
    /// Guild name, if any.
    pub guild: Option<String>,
}

/// An encounter currently visible in the world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncounterView {
    /// Instance object id.
    pub object_id: i32,
    /// Sprite/object type id.
    pub object_type: u16,
    /// Display name.
    pub name: String,
}

/// Worker-owned set of currently visible encounters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EncounterSet {
    /// Current encounters.
    pub encounters: Vec<EncounterView>,
    /// Bumps on every set change.
    pub rev: u64,
}

/// One recoverable Live Feed event emitted by the worker.
#[derive(Debug, Clone)]
pub enum LiveEventView {
    BossCall {
        boss_id: i32,
        name: String,
        server_name: Option<String>,
        realm_name: Option<String>,
    },
    KeyPop {
        opener: String,
        key_id: i32,
        dungeon_name: String,
        thankable: bool,
    },
    AreaUnlock {
        title: String,
        contributions: Vec<(i32, String)>,
        callout: String,
    },
    Warning {
        text: String,
        severity: crate::panels::live_feed::WarningSeverity,
        copyable: bool,
    },
    Dm {
        sender: String,
        text: String,
    },
    Loot(ProcessedLootDrop),
    Dungeon {
        fp: i32,
        display_name: String,
        modifiers: Vec<String>,
        grade: Option<String>,
        origin: DungeonOrigin,
    },
    /// The player left the dungeon, ordered with the entry it invalidates.
    DungeonLeft,
    WhoRoster {
        names: Vec<String>,
    },
    DustFull {
        is_seasonal: bool,
        dust_type: realmhound_core::dust::DustType,
    },
    KickAlert {
        name: String,
    },
    JoinRequestAlert {
        name: String,
    },
}

/// One authoritative Live Feed context update produced by the worker.
///
/// These used to travel as UI payloads, which let a stale queued event undo a
/// newer snapshot. They now live in the snapshot and are replayed in order.
#[derive(Debug, Clone)]
pub enum FeedContextEvent {
    /// Map transition with the realm context it was observed in.
    Location {
        display_name: String,
        realm_name: String,
        current_realm_score: i32,
        max_realm_score: i32,
        is_dungeon: bool,
    },
    /// Realm score tick.
    RealmScore(i32),
    /// Realm close detected; starts the castle countdown.
    RealmClosed,
    /// Crystal/Jailer/Totalia lifecycle update.
    Discovery {
        scope: u64,
        event: super::realm_discovery::DiscoveryEvent,
    },
    /// Connected server display name (from the server-IP mapping).
    ServerName(String),
    /// `/server` reply: authoritative server + realm header override.
    ServerCommand {
        server: String,
        realm: Option<String>,
    },
    /// Forge dust amounts changed.
    DustUpdated {
        amounts: realmhound_core::dust::DustAmounts,
        is_seasonal: bool,
    },
    /// Seasonal status of the current character.
    Seasonal(bool),
    /// A portal object appeared on the current map (anchor for callout timing).
    PortalSpawned { object_id: i32 },
    /// The local player used a portal object.
    PortalUsed { object_id: i32 },
    /// A dungeon run's timer settled, keyed by map seed.
    DungeonTimerFrozen { map_seed: i32, elapsed_ms: i64 },
}

/// A sequenced, timestamped context update.
#[derive(Debug, Clone)]
pub struct FeedContextEntry {
    /// Monotonic id used by the UI to apply only new updates.
    pub seq: u64,
    /// When the worker observed it.
    pub at: chrono::DateTime<chrono::Utc>,
    /// The update.
    pub event: FeedContextEvent,
}

/// Authoritative current feed context, kept outside the bounded event history.
#[derive(Debug, Clone, Default)]
pub struct FeedCurrent {
    /// Connected server display name.
    pub server_name: Option<String>,
    /// Current map context from the last transition.
    pub location: Option<FeedLocation>,
    pub realm_name: Option<String>,
    /// Latest score and maximum, retained as stale inside the same realm's dungeon.
    pub realm_score: Option<(i32, i32)>,
    pub is_score_stale: bool,
    /// Whether the realm has closed.
    pub realm_closed: bool,
    pub castle_deadline: Option<std::time::Instant>,
    /// Current character's seasonal flag.
    pub is_seasonal: Option<bool>,
    /// Latest dust amounts.
    pub dust: Option<(realmhound_core::dust::DustAmounts, bool)>,
    /// Current discovery lifecycle per object.
    pub discoveries: Vec<DiscoveryState>,
    pub discovery_scope: u64,
    pub discovery_reset_at: Option<std::time::Instant>,
    /// Origin flags for the next callout, so an evicted transition cannot
    /// leave them stale.
    pub map_is_realm: bool,
    pub left_from_realm: bool,
    pub map_is_hub: bool,
    pub left_from_hub: bool,
    pub hub_host_hint: Option<std::time::Instant>,
    pub dungeon_origin: Option<DungeonOrigin>,
    pub dungeon_entered_at: Option<std::time::Instant>,
    pub left_from_dungeon: bool,
}

/// One tracked discovery object's durable state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryState {
    pub scope: u64,
    pub object_id: Option<i32>,
    pub object_type: i32,
    pub timestamp: chrono::DateTime<chrono::Local>,
    pub present: bool,
    pub defeated: bool,
    pub packet_seen: bool,
    pub absent_since: Option<std::time::Instant>,
    pub last_message: Option<std::time::Instant>,
    pub defeated_at: Option<std::time::Instant>,
}

/// Resolved inputs shared by automatic and displayed dungeon callouts.
#[derive(Debug, Clone, Default)]
pub struct DungeonOrigin {
    pub map_seed: i32,
    pub entered_at: Option<std::time::Instant>,
    pub server_name: Option<String>,
    pub realm_name: Option<String>,
    pub realm_score_percent: Option<i32>,
    pub entered_from_nexus: bool,
    pub realm_closed: bool,
    pub portal_anchor: Option<std::time::Instant>,
}

/// Map context carried by [`FeedCurrent`].
#[derive(Debug, Clone)]
pub struct FeedLocation {
    pub display_name: String,
    pub realm_name: String,
    pub current_realm_score: i32,
    pub max_realm_score: i32,
    pub is_dungeon: bool,
}

#[derive(Debug, Clone, Default)]
pub struct FeedContext {
    /// Current state, always available even when the history is evicted.
    pub current: FeedCurrent,
    /// Updates oldest to newest (bounded).
    pub events: Vec<FeedContextEntry>,
    /// Highest sequence dropped from the front.
    pub dropped_before_seq: u64,
    /// Modifier sets learned per dungeon (single owner: the worker).
    pub learned_mod_sets: std::collections::HashMap<String, Vec<Vec<String>>>,
    /// Bumps on every append or learning change.
    pub rev: u64,
}

/// A sequenced Live Feed event.
#[derive(Debug, Clone)]
pub struct LiveEventEntry {
    /// Monotonic event id used by the UI to recover only missed entries.
    pub seq: u64,
    /// When the worker observed the event (the UI must not re-date it).
    pub at: chrono::DateTime<chrono::Utc>,
    /// Event payload.
    pub event: LiveEventView,
}

/// Bounded worker-owned window of recent recoverable Live Feed events.
#[derive(Debug, Clone, Default)]
pub struct LiveEventWindow {
    /// Events oldest to newest (bounded).
    pub events: Vec<LiveEventEntry>,
    /// Highest sequence dropped from the front, so a gap is detectable.
    pub dropped_before_seq: u64,
    /// Bumps on every append.
    pub rev: u64,
}

/// Worker-owned quest state: the last fetched quests plus local completion.
#[derive(Debug, Clone)]
pub struct QuestView {
    /// Latest quest fetch response.
    pub packet: QuestFetchResponsePacket,
    /// Bumps on fetch and redeem completion.
    pub rev: u64,
}

/// A released access token with its original capture time and account binding.
/// Memory only: never persisted.
#[derive(Debug, Clone)]
pub struct TokenState {
    /// The token value.
    pub token: String,
    /// When the token was captured from the client's HELLO.
    pub captured_at: DateTime<Utc>,
    /// The verified account the token belongs to.
    pub account_id: AccountId,
}

/// The immutable identity of the one selected account for the process lifetime.
///
/// Sourced from the `AccountContext` at worker construction and never changed
/// afterwards. Packet-observed identity (which lives in [`ViewState`]) is
/// advisory display state and can never overwrite this authoritative value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedIdentity {
    account_key: AccountKey,
    account_id: AccountId,
}

impl SelectedIdentity {
    /// Bind the immutable selected identity.
    pub fn new(account_key: AccountKey, account_id: AccountId) -> Self {
        Self {
            account_key,
            account_id,
        }
    }

    /// The selected local account key.
    pub fn account_key(&self) -> AccountKey {
        self.account_key
    }

    /// The verified server account identity.
    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }
}

/// Immutable model state published by the worker as an `Arc`.
///
/// Publishing builds the next snapshot outside the lock and swaps the pointer;
/// unchanged domains reuse their `Arc`, so the lock only protects the pointer.
#[derive(Debug, Clone)]
pub struct ModelSnapshot {
    /// The immutable selected-account identity. Set once at construction and
    /// preserved across every publication; packet-observed identity in `view`
    /// never overwrites it.
    pub selected_identity: SelectedIdentity,
    /// Coalesced scalar view state (connection fields + generations).
    pub view: ViewState,
    /// Latest account data (characters + vaults). Single source of truth.
    pub account_data: Arc<AccountData>,
    /// Latest live vault data.
    pub live_vault_data: Arc<LiveVaultData>,
    /// Currently playing character id.
    pub live_char_id: Option<i32>,
    /// Socket of the verified main game-client connection, if tracked. Lets the
    /// UI map the connection to its owning OS process for the forge-fire
    /// relaunch safeguard.
    pub main_connection: Option<realmhound_core::stream::ConnectionKey>,
    /// Latest seasonal mission tracklist snapshot.
    pub mission_view: Arc<MissionView>,
    /// Released access token with capture time and account binding (memory only).
    pub token: Option<Arc<TokenState>>,
    /// Worker-owned party membership.
    pub party: Arc<PartyView>,
    /// Worker-owned current encounters.
    pub encounters: Arc<EncounterSet>,
    /// Worker-owned quest state.
    pub quests: Option<Arc<QuestView>>,
    /// Worker-owned recent recoverable Live Feed window.
    pub live_events: Arc<LiveEventWindow>,
    /// Worker-owned Live Feed context (location, score, discovery, anchors).
    pub feed: Arc<FeedContext>,
}

impl ModelSnapshot {
    /// Build the initial snapshot from the freshly constructed model.
    pub fn new(
        selected_identity: SelectedIdentity,
        view: ViewState,
        account_data: AccountData,
        live_vault_data: LiveVaultData,
        live_char_id: Option<i32>,
    ) -> Self {
        Self {
            selected_identity,
            view,
            account_data: Arc::new(account_data),
            live_vault_data: Arc::new(live_vault_data),
            live_char_id,
            main_connection: None,
            mission_view: Arc::new(MissionView::default()),
            token: None,
            party: Arc::new(PartyView::default()),
            encounters: Arc::new(EncounterSet::default()),
            quests: None,
            live_events: Arc::new(LiveEventWindow::default()),
            feed: Arc::new(FeedContext::default()),
        }
    }

    /// Build the next immutable snapshot, reusing unchanged domain `Arc`s.
    /// `None` means "unchanged", so the previous `Arc` is cloned cheaply.
    pub fn next(
        &self,
        view: ViewState,
        live_char_id: Option<i32>,
        main_connection: Option<realmhound_core::stream::ConnectionKey>,
        account_data: Option<Arc<AccountData>>,
        live_vault_data: Option<Arc<LiveVaultData>>,
        mission_view: Option<Arc<MissionView>>,
        token: Option<Arc<TokenState>>,
        party: Option<Arc<PartyView>>,
        encounters: Option<Arc<EncounterSet>>,
        quests: Option<Arc<QuestView>>,
        live_events: Option<Arc<LiveEventWindow>>,
        feed: Option<Arc<FeedContext>>,
    ) -> Self {
        Self {
            selected_identity: self.selected_identity.clone(),
            view,
            account_data: account_data.unwrap_or_else(|| self.account_data.clone()),
            live_vault_data: live_vault_data.unwrap_or_else(|| self.live_vault_data.clone()),
            live_char_id,
            main_connection,
            mission_view: mission_view.unwrap_or_else(|| self.mission_view.clone()),
            token: token.or_else(|| self.token.clone()),
            party: party.unwrap_or_else(|| self.party.clone()),
            encounters: encounters.unwrap_or_else(|| self.encounters.clone()),
            quests: quests.or_else(|| self.quests.clone()),
            live_events: live_events.unwrap_or_else(|| self.live_events.clone()),
            feed: feed.unwrap_or_else(|| self.feed.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use realmhound_core::vault::LiveVaultData;

    fn identity(account_id: &str) -> SelectedIdentity {
        SelectedIdentity::new(AccountKey::generate(), AccountId::new(account_id).unwrap())
    }

    #[test]
    fn next_snapshot_reuses_unchanged_domains() {
        let snap = ModelSnapshot::new(
            identity("ACCT-SNAPSHOT"),
            ViewState::new(None, None),
            AccountData::new(),
            LiveVaultData::default(),
            None,
        );
        let old_account = snap.account_data.clone();
        let old_vault = snap.live_vault_data.clone();

        let mut changed = AccountData::new();
        changed.generation = 7;
        let next = snap.next(
            ViewState::new(None, None),
            None,
            None,
            Some(Arc::new(changed)),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );

        assert!(Arc::ptr_eq(&next.live_vault_data, &old_vault));
        assert!(Arc::ptr_eq(&next.mission_view, &snap.mission_view));
        assert!(!Arc::ptr_eq(&next.account_data, &old_account));
        assert_eq!(next.account_data.generation, 7);
        assert_eq!(old_account.generation, 0, "old snapshot is immutable");

        let token = Arc::new(TokenState {
            token: "tok".to_string(),
            captured_at: Utc::now(),
            account_id: identity("ACCT-SNAPSHOT").account_id().clone(),
        });
        let with_token = next.next(
            ViewState::new(None, None),
            None,
            None,
            None,
            None,
            None,
            Some(token.clone()),
            None,
            None,
            None,
            None,
            None,
        );
        assert!(Arc::ptr_eq(with_token.token.as_ref().unwrap(), &token));
        let reused = with_token.next(
            ViewState::new(None, None),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(
            Arc::ptr_eq(reused.token.as_ref().unwrap(), &token),
            "None keeps the previous token"
        );
    }

    #[test]
    fn selected_identity_survives_a_publication_view_overwrite() {
        let selected = identity("ACCT-SELECTED");
        let mut snap = ModelSnapshot::new(
            selected.clone(),
            ViewState::new(None, None),
            AccountData::new(),
            LiveVaultData::default(),
            None,
        );

        // Publication overwrites `view` (and its packet-observed identity) each
        // frame. Reproduce that mutation and confirm the authoritative selected
        // identity is untouched.
        let mut observed = ViewState::new(
            Some("packet-name".to_string()),
            Some("PACKET-OBSERVED".to_string()),
        );
        observed.char_view_gen = 42;
        snap.view = observed;

        assert_eq!(snap.selected_identity, selected);
        assert_eq!(
            snap.selected_identity.account_id().as_str(),
            "ACCT-SELECTED"
        );
        // The packet-observed id reached only the advisory view, never identity.
        assert_eq!(
            snap.view.saved_account_id.as_deref(),
            Some("PACKET-OBSERVED")
        );
        assert_ne!(
            snap.selected_identity.account_id().as_str(),
            snap.view.saved_account_id.as_deref().unwrap()
        );
    }
}

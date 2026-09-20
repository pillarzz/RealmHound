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
    session::MainAccountStatus,
    settings::{CombatHistorySettings, LootTrackingSettings},
    stream::CaptureHealth,
    vault::{AccountData, LiveVaultData},
    GameEvent,
};

use crate::panels::chat::ChatMessage;
use crate::processing::mission_tracker::MissionView;
use crate::sound::{EventSound, SoundType};
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
#[derive(Debug)]
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
#[derive(Debug)]
pub enum UiPayload {
    /// Re-broadcast a domain event to the session-independent transient panels
    /// (quest / party / live feed / vault). The processor has already applied
    /// any model-side effects; this reproduces the panels' `handle_event`.
    Broadcast(GameEvent),

    /// A resolved chat message to append (whisper direction already tagged).
    Chat(ChatMessage),
    /// An encounter/boss callout for the live feed.
    BossCall { boss_id: i32, text: String },

    /// A Keyper seasonal-event cue for the live feed. `towers` = true when a
    /// crystal-tower wave appeared, false when The Keyper boss spawned. Carries
    /// its own Live Feed entry + sound (independent of the realm-event system).
    KeyperEvent { towers: bool },

    /// Display name for the connected server (from server-IP mapping).
    SetServerName(String),

    /// A processed loot drop (already recorded to the DB by the processor).
    PushLoot(ProcessedLootDrop),
    /// At least one loot drop was recorded this batch (refresh autocomplete).
    LootBatchAdded,

    /// An encounter spawned (forward to the live feed).
    AddEncounter {
        object_id: i32,
        object_type: u16,
        name: String,
    },
    /// An encounter was removed.
    RemoveEncounter(i32),

    /// A party member left (parsed from a chat "has left the party" message).
    PartyMemberLeft(String),

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

    /// Whether the local player is currently the party leader.
    PartyLocalLeaderStatus(bool),

    /// The local (main) account's player name, forwarded to the party panel so
    /// it can render the local player with their real skin/dyes.
    PartyLocalPlayerName(Option<String>),

    /// The party panel should be cleared (e.g. a rejected mule/alt connection).
    PartyCleared,

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

    /// Oryx realm-close announcement detected.
    RealmClosed,

    /// Oryx "MY MINIONS HAVE FAILED ME" detected while in dungeon.
    OryxLagWarning,

    /// The current dungeon run's timer has settled to its committed per-run
    /// value (fired when the run ends: leave/nexus, death, disconnect). Carries
    /// the map seed of the run so the Live Feed can target the matching row.
    DungeonTimerFrozen { map_seed: i32, elapsed_ms: i64 },
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
    /// Play a realm-event notification sound at a specific volume (already the
    /// per-event level; the audio thread scales it by master). `custom_key`
    /// selects a per-boss custom override from `SoundSettings::custom_sounds`.
    PlayEvent {
        sound: EventSound,
        custom_key: String,
        volume: f32,
    },
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
    /// Flush persistent state and exit the worker loop (app shutdown).
    Shutdown,
    /// Clear the reassembler's ignored-connection set.
    ClearIgnored,
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
    /// Remaining loot-drop-boost seconds for the live player, if a boost is
    /// active. Server-driven (already paused by the game in safe areas), shown
    /// on the live character card.
    pub loot_boost_secs: Option<u32>,
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
            loot_boost_secs: None,
        }
    }
}

// ---------------------------------------------------------------------------
// ModelSnapshot (worker -> UI shared latest-value model snapshot)
// ---------------------------------------------------------------------------

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

/// Heavy model state published by the worker into a shared `Arc<Mutex<..>>`.
///
/// The worker overwrites the latest value (coalescing); the App reads it each
/// frame and refreshes its owned copies + dependent panels only when the
/// matching generation in `view` bumps. Cloning the heavy fields is gated on the
/// generations so it happens only on actual change.
#[derive(Debug, Clone)]
pub struct ModelSnapshot {
    /// The immutable selected-account identity. Set once at construction and
    /// preserved across every publication; packet-observed identity in `view`
    /// never overwrites it.
    pub selected_identity: SelectedIdentity,
    /// Coalesced scalar view state (connection fields + generations).
    pub view: ViewState,
    /// Latest account data (characters + vaults). Single source of truth.
    pub account_data: AccountData,
    /// Latest live vault data.
    pub live_vault_data: LiveVaultData,
    /// Currently playing character id.
    pub live_char_id: Option<i32>,
    /// Socket of the verified main game-client connection, if tracked. Lets the
    /// UI map the connection to its owning OS process for the forge-fire
    /// relaunch safeguard.
    pub main_connection: Option<realmhound_core::stream::ConnectionKey>,
    /// Latest seasonal mission tracklist snapshot.
    pub mission_view: MissionView,
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
            account_data,
            live_vault_data,
            live_char_id,
            main_connection: None,
            mission_view: MissionView::default(),
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

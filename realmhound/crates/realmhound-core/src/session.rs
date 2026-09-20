//! Game session state.
//!
//! Groups player, map, vault, and connection state managed during a game session.
//! Provides targeted clear methods for specific state transitions (disconnect,
//! map change, death) and a full `reset_for_capture()` for restarting capture.

use crate::account::AccountId;
use crate::stream::ConnectionKey;
use std::net::Ipv4Addr;

/// Whether two raw server account-id strings denote the same account after
/// normalization (trim + reject empty/control). A value that fails to normalize
/// never matches, so unverifiable input cannot pass identity checks.
fn account_ids_match(a: &str, b: &str) -> bool {
    matches!((AccountId::new(a), AccountId::new(b)), (Ok(x), Ok(y)) if x == y)
}

/// Connection status for the main (tracked) account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MainAccountStatus {
    /// No account identified yet, waiting for first HELLO packet
    #[default]
    WaitingForConnection,
    /// Main account is actively connected
    Connected,
    /// Main account connection was lost (FIN/RST), still ignoring others
    Disconnected,
}

/// Data from an outgoing Create packet, used to detect new character creation.
#[derive(Debug, Clone)]
pub struct PendingCreate {
    /// Class type ID
    pub class_id: u16,
    /// Skin ID (0 = default)
    pub skin_id: u16,
    /// Whether seasonal
    pub is_seasonal: bool,
}

/// Action to take after evaluating a Hello packet for multi-client isolation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelloAction {
    /// Accept this connection as the tentative main account.
    Accept {
        /// Whether we still need to verify the account ID from the next Update.
        needs_verification: bool,
    },
    /// Reject this connection (secondary/alt account).
    Reject,
    /// Same connection reconnecting (e.g., map change). Already tracked.
    Reconnect,
}

/// Result of verifying a detected account identity against the saved one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountVerifyResult {
    /// Account matches the saved ID -- verification passed.
    Verified,
    /// Account doesn't match -- caller should ignore this connection.
    Mismatch {
        /// The account ID detected from the current connection.
        detected_id: String,
        /// The saved account ID from settings.
        saved_id: String,
    },
    /// First run -- no saved account, caller should persist this ID to settings.
    FirstRun {
        /// The detected account ID.
        account_id: String,
        /// The detected account name (if available).
        account_name: Option<String>,
    },
    /// No verification needed (not pending, or no account ID detected).
    NoAction,
}

/// Player identity and state within the current game connection.
#[derive(Debug, Clone, Default)]
pub struct PlayerState {
    /// Player's object ID from CreateSuccess packet (used to identify our player in Update packets)
    pub object_id: Option<i32>,
    /// Player's character ID from CreateSuccess packet (persistent across maps)
    pub char_id: Option<i32>,
    /// Whether current player is seasonal (from Update packet's SEASONAL stat)
    pub is_seasonal: Option<bool>,
    /// Whether the current player has an active crucible (from the Update
    /// packet's CRUCIBLE stat). Authoritative live source that corrects the
    /// stale char-list `CrucibleActive` flag.
    pub is_crucible: Option<bool>,
    /// Pet's object ID (detected from Update packet when pet spawns)
    pub pet_object_id: Option<i32>,
    /// Object IDs confirmed to be pets (any owner) from Update packets, i.e.
    /// objects carrying a `PetType`/`PetName` stat. Used by the processor to
    /// buffer each pet's spawn status (its full inventory + identity ride only
    /// in the spawn Update) for replay. Cleared on map change.
    pub known_pet_object_ids: std::collections::HashSet<i32>,
    /// Instance id of the currently active pet, learned live from the
    /// `ActivePetUpdate` packet. Buffered here so that a packet arriving before
    /// the character id / seasonal status is known can still be reconciled with
    /// the character cache on a later NewTick. Account-wide, so it persists
    /// across map changes.
    pub active_pet_instance_id: Option<i32>,
    /// Remaining loot-drop-boost seconds for the live player, from the
    /// `LootDropTimer` stat. The game already pauses this countdown in safe
    /// areas / loading, so the last-seen value is displayed as-is (it naturally
    /// freezes when no fresh stat arrives). `None` when no boost is active.
    pub loot_boost_secs: Option<u32>,
}

impl PlayerState {
    /// Clear all player state.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Clear player identity (object_id, char_id, is_seasonal) but keep pet.
    pub fn clear_identity(&mut self) {
        self.object_id = None;
        self.char_id = None;
        self.is_seasonal = None;
        self.is_crucible = None;
        self.active_pet_instance_id = None;
    }
}

/// Current map/location state.
#[derive(Debug, Clone, Default)]
pub struct MapState {
    /// Current map name (from MapInfo packet)
    pub name: Option<String>,
    /// Whether current map allows character list API
    pub allows_api: bool,
    /// Maximum realm score (from MapInfo, for calculating percentage)
    pub max_realm_score: Option<i32>,
}

impl MapState {
    /// Clear all map state.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Vault object and page tracking.
#[derive(Debug, Clone, Default)]
pub struct VaultState {
    /// Vault chest object ID (for real-time inventory updates in NewTick)
    pub chest_object_id: Option<i32>,
    /// Material chest object ID
    pub material_chest_object_id: Option<i32>,
    /// Gift chest object ID
    pub gift_chest_object_id: Option<i32>,
    /// Potion storage object ID
    pub potion_storage_object_id: Option<i32>,
    /// Currently active vault page (0-indexed, each page = 8 slots)
    pub active_vault_page: Option<usize>,
    /// Currently active material page
    pub active_material_page: Option<usize>,
    /// Currently active gift page
    pub active_gift_page: Option<usize>,
    /// Currently active potion page
    pub active_potion_page: Option<usize>,
}

impl VaultState {
    /// Clear all vault tracking state.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Multi-client connection isolation state.
#[derive(Debug, Clone, Default)]
pub struct ConnectionState {
    /// Current server IP (for server name lookup)
    pub server_ip: Option<Ipv4Addr>,
    /// Connection key of the main (tracked) account
    pub main_connection: Option<ConnectionKey>,
    /// Status of the main account connection
    pub main_account_status: MainAccountStatus,
    /// Detected account name from Update packets (for UI display)
    pub detected_account_name: Option<String>,
    /// Saved account ID loaded from settings (for returning user verification)
    pub saved_account_id: Option<String>,
    /// Whether we need to verify the tentative connection's account ID
    pub pending_account_verification: bool,
    /// Wall-clock timestamp (ms) when the current tentative candidate began
    /// awaiting identity verification. `None` once verified or when no candidate
    /// is pending. Used for a best-effort identity timeout; a candidate that
    /// never identifies itself is discarded once this age exceeds the timeout.
    pub candidate_since_ms: Option<i64>,
}

impl ConnectionState {
    /// Whether the current connection is the confirmed main account: a
    /// connection is accepted and its account identity is no longer pending
    /// verification. While verification is pending the connection is only a
    /// *tentative* main (e.g. a mule that opened before the main), so its
    /// account-wide stats must not be shown.
    pub fn is_account_verified(&self) -> bool {
        self.main_connection.is_some() && !self.pending_account_verification
    }

    /// Whether a packet arriving on `pkt_conn` belongs to a secondary client
    /// (mule/alt) and must be dropped before routing. Once the main account is
    /// verified on a specific socket, every legitimate packet -- including
    /// global broadcasts (key pops, area unlocks) -- arrives on that socket, so
    /// anything from a different socket is a secondary client. This catches a
    /// mule that was already connected before capture started (whose Hello and
    /// account identity were never observed), which the Hello-reject path alone
    /// cannot isolate.
    pub fn is_foreign_connection(&self, pkt_conn: ConnectionKey) -> bool {
        self.is_account_verified() && self.main_connection != Some(pkt_conn)
    }
}

/// Game session state.
///
/// Groups all state that belongs to the active game session: player identity,
/// map location, vault tracking, and connection isolation. Provides targeted
/// clear methods for each state transition and `reset_for_capture()` for
/// restarting capture.
#[derive(Debug, Clone, Default)]
pub struct GameSession {
    /// Player identity and state
    pub player: PlayerState,
    /// Current map/location
    pub map: MapState,
    /// Vault object and page tracking
    pub vault: VaultState,
    /// Multi-client connection isolation
    pub connection: ConnectionState,
    /// Pending character creation data from outgoing Create packet
    pub pending_create: Option<PendingCreate>,
}

impl GameSession {
    /// Create a new empty session.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset state for a new capture. Clears player, map, vault, and pending
    /// create state. Preserves connection identity (account name, saved ID,
    /// account status) since those survive across captures.
    pub fn reset_for_capture(&mut self) {
        self.player.clear();
        self.map.clear();
        self.vault.clear();
        self.pending_create = None;
    }

    /// Handle main account disconnect (FIN/RST). Sets status to Disconnected,
    /// clears the connection key, and clears player identity.
    pub fn on_disconnect(&mut self) {
        self.connection.main_account_status = MainAccountStatus::Disconnected;
        self.connection.main_connection = None;
        self.connection.candidate_since_ms = None;
        self.player.clear_identity();
    }

    /// Handle entering a new map. Always clears pet tracking. Clears vault
    /// object/page tracking when leaving the vault.
    pub fn on_map_change(&mut self, is_vault: bool) {
        self.player.pet_object_id = None;
        self.player.known_pet_object_ids.clear();
        if !is_vault {
            self.vault.clear();
        }
    }

    /// Handle player death. Clears player object and char IDs if the dead
    /// character matches the current player.
    pub fn on_death(&mut self, dead_char_id: i32) {
        if self.player.char_id == Some(dead_char_id) {
            self.player.object_id = None;
            self.player.char_id = None;
        }
    }

    /// Handle CreateSuccess packet. Sets player identity and resets seasonal
    /// and crucible flags (will be detected from the next Update packet).
    pub fn on_create_success(&mut self, object_id: i32, char_id: i32) {
        self.player.object_id = Some(object_id);
        self.player.char_id = Some(char_id);
        self.player.is_seasonal = None;
        self.player.is_crucible = None;
    }

    /// Evaluate a Hello packet for multi-client isolation.
    ///
    /// Returns the action the caller should take (accept, reject, or
    /// reconnect). Updates session state for accept/reconnect cases.
    /// The caller is responsible for actually ignoring the connection on
    /// `Reject` (since the reassembler lives outside core).
    pub fn evaluate_hello(&mut self, conn_key: ConnectionKey) -> HelloAction {
        match self.connection.main_connection {
            None => {
                // No main connection yet -- accept tentatively
                self.connection.main_connection = Some(conn_key);
                self.connection.main_account_status = MainAccountStatus::Connected;
                self.connection.pending_account_verification =
                    self.connection.saved_account_id.is_some();
                HelloAction::Accept {
                    needs_verification: self.connection.pending_account_verification,
                }
            }
            Some(existing) if existing != conn_key => {
                // Different connection -- mule/alt, reject
                HelloAction::Reject
            }
            _ => {
                // Same connection reconnecting (e.g., map change)
                self.connection.main_account_status = MainAccountStatus::Connected;
                HelloAction::Reconnect
            }
        }
    }

    /// Verify account identity from Update packet stats.
    ///
    /// Checks the detected account ID against the saved one for multi-client
    /// filtering. Also updates `detected_account_name` when a name is found.
    /// The caller should act on the returned `AccountVerifyResult`:
    ///
    /// - `Mismatch` -- ignore the connection and call `on_account_mismatch()`
    /// - `FirstRun` -- persist the account ID/name to settings
    /// - `Verified` / `NoAction` -- no further action needed
    pub fn verify_account_identity(
        &mut self,
        account_id: Option<&str>,
        account_name: Option<&str>,
    ) -> AccountVerifyResult {
        let Some(detected_id) = account_id else {
            return AccountVerifyResult::NoAction;
        };

        if self.connection.pending_account_verification {
            let saved = self.connection.saved_account_id.clone();
            if let Some(saved_id) = saved {
                if !account_ids_match(detected_id, &saved_id) {
                    return AccountVerifyResult::Mismatch {
                        detected_id: detected_id.to_string(),
                        saved_id,
                    };
                }
                self.connection.pending_account_verification = false;
                self.connection.candidate_since_ms = None;
                self.set_detected_name(account_name);
                return AccountVerifyResult::Verified;
            }
        } else if self.connection.saved_account_id.is_none() {
            self.connection.saved_account_id = Some(detected_id.to_string());
            self.connection.candidate_since_ms = None;
            self.set_detected_name(account_name);
            return AccountVerifyResult::FirstRun {
                account_id: detected_id.to_string(),
                account_name: account_name.map(|s| s.to_string()),
            };
        } else if self
            .connection
            .saved_account_id
            .as_deref()
            .is_some_and(|saved| account_ids_match(detected_id, saved))
        {
            // Already-verified main on a later update: keep the name fresh.
            self.set_detected_name(account_name);
        }

        AccountVerifyResult::NoAction
    }

    /// Commit the detected account name for UI display. Only called once a
    /// connection's identity is confirmed to be the saved main account, so a
    /// tentative/unverified connection (possibly a mule) never leaks its name
    /// to the widget bar.
    fn set_detected_name(&mut self, account_name: Option<&str>) {
        if let Some(name) = account_name {
            if self.connection.detected_account_name.as_deref() != Some(name) {
                self.connection.detected_account_name = Some(name.to_string());
            }
        }
    }

    /// Handle account identity mismatch during multi-client isolation.
    /// Resets to waiting state and clears player identity.
    pub fn on_account_mismatch(&mut self) {
        self.connection.main_account_status = MainAccountStatus::WaitingForConnection;
        self.connection.main_connection = None;
        self.player.object_id = None;
        self.player.char_id = None;
        self.connection.detected_account_name = None;
        self.connection.pending_account_verification = false;
        self.connection.candidate_since_ms = None;
    }

    /// Reset account identity (user-initiated). Clears all account-related
    /// state and returns to waiting for a new connection.
    pub fn on_account_reset(&mut self) {
        self.connection.saved_account_id = None;
        self.connection.detected_account_name = None;
        self.connection.main_connection = None;
        self.connection.main_account_status = MainAccountStatus::WaitingForConnection;
        self.connection.pending_account_verification = false;
        self.connection.candidate_since_ms = None;
        self.player.clear_identity();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_session() {
        let session = GameSession::new();
        assert_eq!(session.player.object_id, None);
        assert_eq!(session.player.char_id, None);
        assert_eq!(session.player.is_seasonal, None);
        assert_eq!(session.player.pet_object_id, None);
        assert_eq!(session.map.name, None);
        assert!(!session.map.allows_api);
        assert_eq!(session.map.max_realm_score, None);
        assert_eq!(session.vault.chest_object_id, None);
        assert_eq!(session.vault.active_vault_page, None);
        assert_eq!(session.connection.server_ip, None);
        assert_eq!(session.connection.main_connection, None);
        assert_eq!(
            session.connection.main_account_status,
            MainAccountStatus::WaitingForConnection
        );
        assert_eq!(session.connection.detected_account_name, None);
        assert_eq!(session.connection.saved_account_id, None);
        assert!(!session.connection.pending_account_verification);
        assert!(session.pending_create.is_none());
    }

    #[test]
    fn test_on_create_success() {
        let mut session = GameSession::new();
        session.player.is_seasonal = Some(true); // leftover from previous

        session.on_create_success(42, 100);

        assert_eq!(session.player.object_id, Some(42));
        assert_eq!(session.player.char_id, Some(100));
        // is_seasonal should be reset (will come from Update packet)
        assert_eq!(session.player.is_seasonal, None);
    }

    #[test]
    fn test_on_death_matching_char() {
        let mut session = GameSession::new();
        session.on_create_success(42, 100);

        session.on_death(100);

        assert_eq!(session.player.object_id, None);
        assert_eq!(session.player.char_id, None);
    }

    #[test]
    fn test_on_death_non_matching_char() {
        let mut session = GameSession::new();
        session.on_create_success(42, 100);

        session.on_death(999); // different char

        // Should not be cleared
        assert_eq!(session.player.object_id, Some(42));
        assert_eq!(session.player.char_id, Some(100));
    }

    #[test]
    fn test_on_disconnect() {
        let mut session = GameSession::new();
        session.on_create_success(42, 100);
        session.player.is_seasonal = Some(false);
        session.connection.main_account_status = MainAccountStatus::Connected;
        session.connection.main_connection = Some(ConnectionKey::new(
            "192.168.1.1".parse().unwrap(),
            12345,
            "10.0.0.1".parse().unwrap(),
            2050,
        ));

        session.on_disconnect();

        assert_eq!(
            session.connection.main_account_status,
            MainAccountStatus::Disconnected
        );
        assert_eq!(session.connection.main_connection, None);
        assert_eq!(session.player.object_id, None);
        assert_eq!(session.player.char_id, None);
        assert_eq!(session.player.is_seasonal, None);
    }

    #[test]
    fn test_on_disconnect_preserves_pet_and_vault() {
        let mut session = GameSession::new();
        session.player.pet_object_id = Some(99);
        session.vault.chest_object_id = Some(55);
        session.vault.active_vault_page = Some(2);

        session.on_disconnect();

        // Pet and vault state are NOT cleared on disconnect
        assert_eq!(session.player.pet_object_id, Some(99));
        assert_eq!(session.vault.chest_object_id, Some(55));
        assert_eq!(session.vault.active_vault_page, Some(2));
    }

    #[test]
    fn test_reset_for_capture() {
        let mut session = GameSession::new();

        // Populate everything
        session.on_create_success(42, 100);
        session.player.is_seasonal = Some(true);
        session.player.pet_object_id = Some(99);
        session.map.name = Some("Nexus".to_string());
        session.map.allows_api = true;
        session.map.max_realm_score = Some(500);
        session.vault.chest_object_id = Some(55);
        session.vault.material_chest_object_id = Some(56);
        session.vault.gift_chest_object_id = Some(57);
        session.vault.potion_storage_object_id = Some(58);
        session.vault.active_vault_page = Some(2);
        session.vault.active_material_page = Some(1);
        session.vault.active_gift_page = Some(0);
        session.vault.active_potion_page = Some(3);
        session.pending_create = Some(PendingCreate {
            class_id: 782,
            skin_id: 0,
            is_seasonal: false,
        });
        session.connection.saved_account_id = Some("abc123".to_string());
        session.connection.detected_account_name = Some("Player1".to_string());
        session.connection.main_account_status = MainAccountStatus::Connected;

        session.reset_for_capture();

        // Player, map, vault, pending_create should all be cleared
        assert_eq!(session.player.object_id, None);
        assert_eq!(session.player.char_id, None);
        assert_eq!(session.player.is_seasonal, None);
        assert_eq!(session.player.pet_object_id, None);
        assert_eq!(session.map.name, None);
        assert!(!session.map.allows_api);
        assert_eq!(session.map.max_realm_score, None);
        assert_eq!(session.vault.chest_object_id, None);
        assert_eq!(session.vault.material_chest_object_id, None);
        assert_eq!(session.vault.active_vault_page, None);
        assert!(session.pending_create.is_none());

        // Connection identity should be PRESERVED
        assert_eq!(
            session.connection.saved_account_id,
            Some("abc123".to_string())
        );
        assert_eq!(
            session.connection.detected_account_name,
            Some("Player1".to_string())
        );
        assert_eq!(
            session.connection.main_account_status,
            MainAccountStatus::Connected
        );
    }

    #[test]
    fn test_on_map_change_entering_vault() {
        let mut session = GameSession::new();
        session.player.pet_object_id = Some(99);
        session.vault.chest_object_id = Some(55);
        session.vault.active_vault_page = Some(2);

        session.on_map_change(true); // entering vault

        // Pet cleared, vault preserved
        assert_eq!(session.player.pet_object_id, None);
        assert_eq!(session.vault.chest_object_id, Some(55));
        assert_eq!(session.vault.active_vault_page, Some(2));
    }

    #[test]
    fn test_on_map_change_leaving_vault() {
        let mut session = GameSession::new();
        session.player.pet_object_id = Some(99);
        session.vault.chest_object_id = Some(55);
        session.vault.material_chest_object_id = Some(56);
        session.vault.gift_chest_object_id = Some(57);
        session.vault.potion_storage_object_id = Some(58);
        session.vault.active_vault_page = Some(2);
        session.vault.active_material_page = Some(1);
        session.vault.active_gift_page = Some(0);
        session.vault.active_potion_page = Some(3);

        session.on_map_change(false); // leaving vault

        // Pet cleared, vault cleared
        assert_eq!(session.player.pet_object_id, None);
        assert_eq!(session.vault.chest_object_id, None);
        assert_eq!(session.vault.material_chest_object_id, None);
        assert_eq!(session.vault.gift_chest_object_id, None);
        assert_eq!(session.vault.potion_storage_object_id, None);
        assert_eq!(session.vault.active_vault_page, None);
        assert_eq!(session.vault.active_material_page, None);
        assert_eq!(session.vault.active_gift_page, None);
        assert_eq!(session.vault.active_potion_page, None);
    }

    #[test]
    fn test_on_map_change_preserves_player_identity() {
        let mut session = GameSession::new();
        session.on_create_success(42, 100);
        session.player.is_seasonal = Some(false);

        session.on_map_change(false);

        // Player identity should be preserved across map changes
        assert_eq!(session.player.object_id, Some(42));
        assert_eq!(session.player.char_id, Some(100));
        assert_eq!(session.player.is_seasonal, Some(false));
    }

    #[test]
    fn test_on_account_mismatch() {
        let mut session = GameSession::new();
        session.on_create_success(42, 100);
        session.connection.main_account_status = MainAccountStatus::Connected;
        session.connection.main_connection = Some(ConnectionKey::new(
            "192.168.1.1".parse().unwrap(),
            12345,
            "10.0.0.1".parse().unwrap(),
            2050,
        ));
        session.connection.detected_account_name = Some("Player1".to_string());
        session.connection.pending_account_verification = true;
        session.connection.saved_account_id = Some("main_id".to_string());

        session.on_account_mismatch();

        assert_eq!(
            session.connection.main_account_status,
            MainAccountStatus::WaitingForConnection
        );
        assert_eq!(session.connection.main_connection, None);
        assert_eq!(session.player.object_id, None);
        assert_eq!(session.player.char_id, None);
        assert_eq!(session.connection.detected_account_name, None);
        assert!(!session.connection.pending_account_verification);
        // saved_account_id should be preserved
        assert_eq!(
            session.connection.saved_account_id,
            Some("main_id".to_string())
        );
    }

    #[test]
    fn test_on_account_reset() {
        let mut session = GameSession::new();
        session.on_create_success(42, 100);
        session.player.is_seasonal = Some(false);
        session.connection.main_account_status = MainAccountStatus::Connected;
        session.connection.main_connection = Some(ConnectionKey::new(
            "192.168.1.1".parse().unwrap(),
            12345,
            "10.0.0.1".parse().unwrap(),
            2050,
        ));
        session.connection.detected_account_name = Some("Player1".to_string());
        session.connection.saved_account_id = Some("main_id".to_string());
        session.connection.pending_account_verification = true;

        session.on_account_reset();

        assert_eq!(session.connection.saved_account_id, None);
        assert_eq!(session.connection.detected_account_name, None);
        assert_eq!(session.connection.main_connection, None);
        assert_eq!(
            session.connection.main_account_status,
            MainAccountStatus::WaitingForConnection
        );
        assert!(!session.connection.pending_account_verification);
        assert_eq!(session.player.object_id, None);
        assert_eq!(session.player.char_id, None);
        assert_eq!(session.player.is_seasonal, None);
    }

    #[test]
    fn test_on_account_reset_preserves_pet_and_vault() {
        let mut session = GameSession::new();
        session.player.pet_object_id = Some(99);
        session.vault.chest_object_id = Some(55);
        session.map.name = Some("Vault".to_string());

        session.on_account_reset();

        // Pet, vault, and map should NOT be cleared by account reset
        assert_eq!(session.player.pet_object_id, Some(99));
        assert_eq!(session.vault.chest_object_id, Some(55));
        assert_eq!(session.map.name, Some("Vault".to_string()));
    }

    #[test]
    fn test_player_clear_vs_clear_identity() {
        let mut player = PlayerState {
            object_id: Some(42),
            char_id: Some(100),
            is_seasonal: Some(true),
            pet_object_id: Some(99),
            ..Default::default()
        };

        // clear_identity keeps pet
        player.clear_identity();
        assert_eq!(player.object_id, None);
        assert_eq!(player.char_id, None);
        assert_eq!(player.is_seasonal, None);
        assert_eq!(player.pet_object_id, Some(99)); // preserved

        // clear removes everything
        player.pet_object_id = Some(99);
        player.clear();
        assert_eq!(player.pet_object_id, None); // gone
    }

    // -- evaluate_hello --

    fn conn_key_a() -> ConnectionKey {
        ConnectionKey::new(
            "192.168.1.1".parse().unwrap(),
            12345,
            "10.0.0.1".parse().unwrap(),
            2050,
        )
    }

    fn conn_key_b() -> ConnectionKey {
        ConnectionKey::new(
            "192.168.1.2".parse().unwrap(),
            54321,
            "10.0.0.1".parse().unwrap(),
            2050,
        )
    }

    #[test]
    fn evaluate_hello_accepts_first_connection() {
        let mut session = GameSession::new();
        let result = session.evaluate_hello(conn_key_a());
        assert_eq!(
            result,
            HelloAction::Accept {
                needs_verification: false
            }
        );
        assert_eq!(session.connection.main_connection, Some(conn_key_a()));
        assert_eq!(
            session.connection.main_account_status,
            MainAccountStatus::Connected
        );
    }

    #[test]
    fn evaluate_hello_accepts_with_verification_when_saved_id_exists() {
        let mut session = GameSession::new();
        session.connection.saved_account_id = Some("saved123".to_string());
        let result = session.evaluate_hello(conn_key_a());
        assert_eq!(
            result,
            HelloAction::Accept {
                needs_verification: true
            }
        );
        assert!(session.connection.pending_account_verification);
    }

    #[test]
    fn evaluate_hello_rejects_secondary_connection() {
        let mut session = GameSession::new();
        session.evaluate_hello(conn_key_a());
        let result = session.evaluate_hello(conn_key_b());
        assert_eq!(result, HelloAction::Reject);
        // Main connection unchanged
        assert_eq!(session.connection.main_connection, Some(conn_key_a()));
    }

    #[test]
    fn account_verified_only_after_pending_verification_clears() {
        let mut session = GameSession::new();
        // No connection yet: not verified.
        assert!(!session.connection.is_account_verified());

        // Returning user (saved id): tentative main is pending -> not verified,
        // so a mule that opened first cannot leak its account-wide stats.
        session.connection.saved_account_id = Some("main_id".to_string());
        session.evaluate_hello(conn_key_a());
        assert!(session.connection.pending_account_verification);
        assert!(!session.connection.is_account_verified());

        // Matching account clears pending -> verified.
        session.verify_account_identity(Some("main_id"), Some("Main"));
        assert!(session.connection.is_account_verified());
    }

    #[test]
    fn account_verified_immediately_on_first_run() {
        // Fresh install (no saved id): nothing to verify, so the sole
        // connection is treated as the main account right away.
        let mut session = GameSession::new();
        session.evaluate_hello(conn_key_a());
        assert!(!session.connection.pending_account_verification);
        assert!(session.connection.is_account_verified());
    }

    #[test]
    fn foreign_connection_dropped_once_main_verified() {
        // Main verified on connection A: a packet on A is the main's, a packet
        // on B is a secondary client that must be dropped -- even though B never
        // sent a Hello we rejected (it was already connected before capture).
        let mut session = GameSession::new();
        session.evaluate_hello(conn_key_a());
        assert!(session.connection.is_account_verified());

        assert!(
            !session.connection.is_foreign_connection(conn_key_a()),
            "the verified main's own socket is never foreign"
        );
        assert!(
            session.connection.is_foreign_connection(conn_key_b()),
            "an already-connected mule's socket is foreign and must be dropped"
        );
    }

    #[test]
    fn no_connection_is_treated_as_foreign_while_unverified() {
        // While no main is verified (tentative window / between maps), the guard
        // must stay off so the gated buffer/verify path can still classify the
        // real main -- otherwise every packet would be dropped.
        let mut session = GameSession::new();
        session.connection.saved_account_id = Some("main_id".to_string());
        session.evaluate_hello(conn_key_a());
        assert!(session.connection.pending_account_verification);
        assert!(
            !session.connection.is_foreign_connection(conn_key_b()),
            "no hard drop while the main is still unverified"
        );
    }

    #[test]
    fn evaluate_hello_reconnects_same_connection() {
        let mut session = GameSession::new();
        session.evaluate_hello(conn_key_a());
        session.connection.main_account_status = MainAccountStatus::Disconnected;
        let result = session.evaluate_hello(conn_key_a());
        assert_eq!(result, HelloAction::Reconnect);
        assert_eq!(
            session.connection.main_account_status,
            MainAccountStatus::Connected
        );
    }

    // -- verify_account_identity --

    #[test]
    fn verify_no_action_when_no_account_id() {
        let mut session = GameSession::new();
        let result = session.verify_account_identity(None, Some("Player1"));
        assert_eq!(result, AccountVerifyResult::NoAction);
        // Without an account id the connection can't be verified, so the name
        // must NOT be committed (it could be a mule) -- prevents widget flicker.
        assert_eq!(session.connection.detected_account_name, None);
    }

    #[test]
    fn verify_does_not_commit_name_on_mismatch() {
        let mut session = GameSession::new();
        session.connection.saved_account_id = Some("abc123".to_string());
        session.connection.pending_account_verification = true;
        let result = session.verify_account_identity(Some("xyz789"), Some("MulePlayer"));
        assert!(matches!(result, AccountVerifyResult::Mismatch { .. }));
        // A mismatching (mule) connection never leaks its name to the UI.
        assert_eq!(session.connection.detected_account_name, None);
    }

    #[test]
    fn verify_commits_name_on_match() {
        let mut session = GameSession::new();
        session.connection.saved_account_id = Some("abc123".to_string());
        session.connection.pending_account_verification = true;
        let result = session.verify_account_identity(Some("abc123"), Some("MainPlayer"));
        assert_eq!(result, AccountVerifyResult::Verified);
        assert_eq!(
            session.connection.detected_account_name,
            Some("MainPlayer".to_string())
        );
    }

    #[test]
    fn verify_first_run_saves_account_id() {
        let mut session = GameSession::new();
        let result = session.verify_account_identity(Some("abc123"), Some("Player1"));
        assert_eq!(
            result,
            AccountVerifyResult::FirstRun {
                account_id: "abc123".to_string(),
                account_name: Some("Player1".to_string()),
            }
        );
        assert_eq!(
            session.connection.saved_account_id,
            Some("abc123".to_string())
        );
    }

    #[test]
    fn verify_account_match_passes() {
        let mut session = GameSession::new();
        session.connection.saved_account_id = Some("abc123".to_string());
        session.connection.pending_account_verification = true;
        let result = session.verify_account_identity(Some("abc123"), Some("Player1"));
        assert_eq!(result, AccountVerifyResult::Verified);
        assert!(!session.connection.pending_account_verification);
    }

    #[test]
    fn verify_account_mismatch_returns_mismatch() {
        let mut session = GameSession::new();
        session.connection.saved_account_id = Some("abc123".to_string());
        session.connection.pending_account_verification = true;
        let result = session.verify_account_identity(Some("xyz789"), Some("AltPlayer"));
        assert_eq!(
            result,
            AccountVerifyResult::Mismatch {
                detected_id: "xyz789".to_string(),
                saved_id: "abc123".to_string(),
            }
        );
    }

    #[test]
    fn verify_no_action_when_already_verified() {
        let mut session = GameSession::new();
        session.connection.saved_account_id = Some("abc123".to_string());
        session.connection.pending_account_verification = false;
        let result = session.verify_account_identity(Some("abc123"), None);
        assert_eq!(result, AccountVerifyResult::NoAction);
    }

    #[test]
    fn verify_matches_normalized_account_id() {
        let mut session = GameSession::new();
        session.connection.saved_account_id = Some("abc123".to_string());
        session.connection.pending_account_verification = true;
        // A surrounding-whitespace variant normalizes to the same identity.
        let result = session.verify_account_identity(Some("  abc123  "), Some("Player1"));
        assert_eq!(result, AccountVerifyResult::Verified);
        assert!(!session.connection.pending_account_verification);
    }

    #[test]
    fn verify_clears_candidate_timer_on_match() {
        let mut session = GameSession::new();
        session.connection.saved_account_id = Some("abc123".to_string());
        session.connection.pending_account_verification = true;
        session.connection.candidate_since_ms = Some(5_000);
        let result = session.verify_account_identity(Some("abc123"), Some("Player1"));
        assert_eq!(result, AccountVerifyResult::Verified);
        assert_eq!(session.connection.candidate_since_ms, None);
    }
}

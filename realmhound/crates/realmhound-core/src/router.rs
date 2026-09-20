//! Packet routing and domain event generation.
//!
//! The `PacketRouter` processes parsed network packets, updates `GameSession`
//! state, and emits typed `GameEvent`s for the GUI to dispatch to panels.
//!
//! All known packet types are routed here. The `route()` method is the
//! single entry point for packet processing -- the legacy mega-method
//! `process_packets()` in the GUI crate now delegates entirely to this router.

use crate::api::ClassExaltation;
use crate::dust::DustAmounts;
use crate::protocol::packets::{
    IncomingPartyMemberInfoPacket, InvSwapPacket, NewTickPacket, ParsedPacket,
    PartyMemberAddedPacket, PartyRequestResponsePacket, QuestFetchResponsePacket, RotmgPacket,
    TextPacket, UpdatePacket, VaultContentPacket,
};
use crate::session::{GameSession, PendingCreate};
use crate::vault::VaultType;

// ---------------------------------------------------------------------------
// GameEvent
// ---------------------------------------------------------------------------

/// A typed domain event emitted by the `PacketRouter`.
///
/// Panels consume these events instead of raw packets, which decouples packet
/// parsing from UI updates and keeps business logic in the core crate.
#[derive(Debug, Clone)]
pub enum GameEvent {
    /// Player's character died.
    CharacterDied {
        char_id: i32,
        killed_by: String,
        total_fame: i32,
        /// Gravestone object id from the server (grave matching stats at death).
        gravestone_type: i32,
    },

    /// Daily quest data received from the Tinkerer.
    QuestsReceived(QuestFetchResponsePacket),

    /// Realm completion score changed.
    RealmScoreChanged { score: i32 },

    /// Player dealt damage to an entity (used for loot attribution).
    ///
    /// Produced by both `EnemyHit` and `Damage` packets.
    EntityHit { target_id: i32 },

    /// A `DamagePacket` was received - a player dealt damage to an entity.
    ///
    /// Carries full attribution for the combat engine: `attacker_id` is the
    /// bullet owner (`object_id`), `amount` is the server-computed post-defense
    /// damage. Emitted alongside `EntityHit` (which loot attribution consumes).
    DamageDealt {
        target_id: i32,
        attacker_id: i32,
        amount: u16,
    },

    /// The local player hit an entity (from the outgoing `EnemyHit` packet).
    ///
    /// Distinct from `EntityHit` (which is also produced by incoming `Damage`):
    /// the combat engine uses this to know the local player took part in a fight
    /// and how many times they hit, since the local player's own damage is not
    /// present in `DamagePacket`s. `shooter_id` is the bullet's source (the local
    /// player or one of their summons); `main_id` is the owning player.
    LocalPlayerHit {
        target_id: i32,
        bullet_id: i16,
        shooter_id: i32,
        main_id: i32,
    },

    /// The local player fired a shot (from the outgoing `PlayerShoot` packet).
    ///
    /// The combat engine rolls this shot's damage from the map-seeded RNG and
    /// correlates it to a later `LocalPlayerHit` via `bullet_id`.
    LocalPlayerShot {
        bullet_id: i16,
        weapon_id: i32,
        projectile_id: i32,
        client_time: i32,
        shot_x: f32,
        shot_y: f32,
        angle: f32,
    },

    /// A summon/ally shot (from the incoming `ServerPlayerShoot` packet).
    ///
    /// Carries the server-precomputed projectile `damage`. `owner_id` is the
    /// entity that fired (a summon), `summoner_id` is the owning player. The
    /// combat engine uses this to (a) attribute the local player's own summon
    /// damage and (b) redirect remote summon `DamagePacket`s to their owner.
    AllyOrSummonShot {
        owner_id: i32,
        summoner_id: i32,
        bullet_id: i16,
        bullet_count: i8,
        damage: i16,
        container_type: i32,
        bullet_type: i8,
    },

    /// An enemy fired a projectile (from the incoming `EnemyShoot` packet).
    ///
    /// Carries the projectile's pre-defense `damage`. The combat engine stashes
    /// it keyed by (owner, bullet id) so a later `LocalPlayerWasHit` can estimate
    /// the local player's damage taken. `num_shots` covers multi-projectile
    /// volleys (consecutive bullet ids). `bullet_type` indexes the firing
    /// enemy's projectile list, used to resolve whether the shot is
    /// armor-piercing (bypasses the local player's defense).
    EnemyShot {
        owner_id: i32,
        bullet_id: i16,
        bullet_type: u8,
        damage: i16,
        num_shots: u8,
    },

    /// The local player was hit (from the outgoing `PlayerHit` packet).
    ///
    /// `bullet_id` + `enemy_id` correlate to a prior `EnemyShot` so the combat
    /// engine can value the local player's damage taken.
    LocalPlayerWasHit { bullet_id: i16, enemy_id: i32 },

    /// The local player used an item (from the outgoing `UseItem` packet).
    ///
    /// Carries the used item's type id so the combat engine can open a
    /// Knight-shield damage-reduction window when the item is a shield, plus the
    /// throw/aim world position (blast center for self-computed ability damage).
    LocalItemUsed { item_id: i32, x: f32, y: f32 },

    /// Character XML data received from server (NewCharacterInfo packet).
    CharacterXmlReceived { xml: String },

    /// Seasonal status detected from Update packet.
    /// Emitted when a character's seasonal/regular status is first detected or changes.
    /// This handles season transitions where seasonal characters become regular.
    SeasonalStatusReceived { char_id: i32, is_seasonal: bool },

    /// Crucible status detected from the live player object's CRUCIBLE stat.
    /// Emitted when the active-crucible state is first detected or changes, so
    /// the stale char-list `CrucibleActive` flag can be corrected (it does not
    /// clear after a season ends).
    CrucibleStatusReceived { char_id: i32, is_active: bool },

    /// Account accelerator activated live (AcceleratorActivated packet id 153),
    /// e.g. drinking a dust-drop potion. Carries only the accelerator object
    /// type -- the packet has no timer -- so the consumer estimates the expiry
    /// from the accelerator's known fixed duration at the moment of activation.
    AccountAcceleratorActivated { object_type: i32 },

    /// Crucible season definitions parsed from a CrucibleResponse packet. Only
    /// entries carrying an actual `crucible` object are included; event-dungeon
    /// modifier entries are filtered out.
    CrucibleDefinitionsReceived(Vec<crate::api::crucible::CrucibleDef>),

    /// Full party member list received (IncomingPartyMemberInfo packet).
    PartyListReceived(IncomingPartyMemberInfoPacket),

    /// A new player joined the party (PartyMemberAdded packet).
    PartyMemberJoined(PartyMemberAddedPacket),

    /// A party action was received from the server (kick, leave, promote).
    PartyActionReceived { player_id: i16, action_id: u8 },

    /// A party join request response received (PartyRequestResponse packet).
    /// Carries the requesting player's name, class, skin, and invite state.
    PartyJoinRequestResponse(PartyRequestResponsePacket),

    /// Player fully loaded into a map (CreateSuccess packet).
    PlayerLoaded {
        char_id: i32,
        object_id: i32,
        /// Set when a brand-new character was created (preceded by Create packet).
        new_character: Option<PendingCreate>,
        pc_stats: String,
    },

    /// Player entered a new map (MapInfo packet).
    MapChanged {
        name: String,
        display_name: String,
        realm_name: String,
        fp: i32,
        current_realm_score: i32,
        max_realm_score: i32,
        allows_api: bool,
        /// Whether this map is a real dungeon (not a hub/realm), for feed output.
        is_dungeon: bool,
        /// Decoded dungeon modifier id tokens (e.g. "CHEF"), empty if none.
        dungeon_modifiers: Vec<String>,
        /// Dungeon grade (e.g. "S", "A"), if present.
        dungeon_grade: Option<String>,
    },

    /// Vault contents received while in vault (VaultContent packet).
    VaultContentReceived {
        packet: VaultContentPacket,
        vault_type: VaultType,
    },

    /// Chat/text message received (Text packet).
    TextReceived(TextPacket),

    /// A dungeon portal was opened via a key (PortalOpened notification whose
    /// message carries a `"player"` field). `message` is the raw JSON-ish
    /// notification text; `picture_type` is the portal object type id (resolves
    /// to the dungeon name via the asset manager).
    PortalOpened { message: String, picture_type: i32 },

    /// A server-wide area-unlock / monument-activation broadcast (ServerMessage
    /// notification, effect 1). `message` is the raw JSON-ish notification text
    /// (`s.dungeon_unlocked_by` or `s.something_by_player`); the processor
    /// decodes the dungeon/monument name and the popper.
    AreaUnlockBroadcast { message: String },

    /// Another player requested a trade (TradeRequested packet).
    TradeRequested { name: String },

    /// Hello packet processed - access token captured.
    HelloReceived { access_token: String },

    /// Update packet processed - carries data for loot/encounter GUI wiring.
    UpdateReceived(UpdatePacket, u64),

    /// NewTick packet processed - carries data for inventory/loot GUI wiring.
    NewTickReceived(NewTickPacket, u64),

    /// Inventory swap packet received (InvSwap packet).
    InvSwapReceived(InvSwapPacket),

    /// Dust amounts updated from player stats.
    DustUpdated {
        /// Parsed dust amounts (green, red, purple).
        amounts: DustAmounts,
        /// Whether this is for a seasonal character.
        is_seasonal: bool,
    },

    /// Account-level stats updated from the local player's object (Widget Bar).
    AccountStatsUpdated(crate::account_stats::AccountStatsUpdate),

    /// Exaltation progress updated (ExaltationBonusChanged packet).
    ExaltationUpdated {
        /// Class type ID that was updated.
        class_type: i32,
        /// Updated exaltation data.
        exaltation: ClassExaltation,
    },

    /// Server reconnect received (Reconnect packet).
    /// Contains the target server name, used to update the displayed server.
    ReconnectReceived {
        /// Human-readable server name (e.g. "USWest", "EUNorth")
        server_name: String,
    },

    /// Local player used a portal (outgoing UsePortalPacket). Carries the portal
    /// object id, used to anchor the dungeon join timer to the portal's spawn.
    PortalUsed {
        /// Object id of the portal the player entered.
        object_id: i32,
    },

    /// The active pet changed (incoming `ActivePetUpdate`, ID 76). Carries the
    /// instance id of the newly active pet so the character cache can remap
    /// live pet-inventory updates without waiting for an API refresh.
    ActivePetChanged {
        /// Instance id of the newly active pet.
        instance_id: i32,
    },

    /// Local player attempted to redeem a quest (outgoing QuestRedeemPacket).
    /// Carries the quest id so it can be matched up with the following
    /// `QuestRedeemResult`, since the server's response doesn't echo it back.
    QuestRedeemAttempted {
        /// Id of the quest the player tried to redeem (`QuestData::id`).
        quest_id: String,
    },

    /// Server responded to a quest redemption attempt (QuestRedeemResponse
    /// packet). Used to hide a completed non-repeatable quest immediately,
    /// instead of waiting for the player to re-enter the Tinkerer's portal.
    QuestRedeemResult {
        /// Whether the redemption succeeded.
        ok: bool,
    },
}

// ---------------------------------------------------------------------------
// RouteResult
// ---------------------------------------------------------------------------

/// Outcome of routing a single parsed packet.
#[derive(Debug)]
pub enum RouteResult {
    /// Packet was processed by the router.
    ///
    /// The `Vec` may be empty when the packet only updates session state
    /// without producing any UI-visible event.
    Routed(Vec<GameEvent>),

    /// Packet type has not been migrated to the router yet.
    /// The caller should handle it via its existing (legacy) match.
    Unmapped,
}

impl RouteResult {
    /// Returns `true` when the packet was handled by the router.
    pub fn is_routed(&self) -> bool {
        matches!(self, RouteResult::Routed(_))
    }
}

// ---------------------------------------------------------------------------
// PacketRouter
// ---------------------------------------------------------------------------

/// Routes parsed packets to session-state updates and domain events.
///
/// Packet types are migrated here incrementally. Unmapped types return
/// `RouteResult::Unmapped` so the GUI can fall through to its legacy handler.
#[derive(Debug)]
pub struct PacketRouter {
    _private: (), // prevent external construction; use PacketRouter::new()
}

impl PacketRouter {
    /// Create a new router.
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Process a single parsed packet.
    ///
    /// Updates `session` state where appropriate and returns domain events
    /// for the GUI to dispatch to panels.
    pub fn route(&mut self, packet: &ParsedPacket, session: &mut GameSession) -> RouteResult {
        match packet {
            // ----- Death -----
            ParsedPacket::Death(death) => {
                tracing::warn!(
                    "☠️ [DEATH] Character died: char_id={}, killed_by='{}', fame={}",
                    death.char_id,
                    death.killed_by,
                    death.total_fame
                );

                // Update session state (clears live char)
                session.on_death(death.char_id);

                RouteResult::Routed(vec![GameEvent::CharacterDied {
                    char_id: death.char_id,
                    killed_by: death.killed_by.clone(),
                    total_fame: death.total_fame,
                    gravestone_type: death.gravestone_type,
                }])
            }

            // ----- Quest fetch response -----
            ParsedPacket::QuestFetchResponse(quest_response) => {
                let displayable_count = quest_response.displayable_quests().len();
                tracing::info!(
                    "[QUEST] Received {} quests ({} displayable), refresh cost: {} gold",
                    quest_response.quests.len(),
                    displayable_count,
                    quest_response.next_refresh_price
                );

                RouteResult::Routed(vec![GameEvent::QuestsReceived(quest_response.clone())])
            }

            // ----- Realm score update -----
            ParsedPacket::RealmScoreUpdate(score_update) => {
                RouteResult::Routed(vec![GameEvent::RealmScoreChanged {
                    score: score_update.score,
                }])
            }

            // ----- Enemy hit (outgoing) -----
            ParsedPacket::EnemyHit(enemy_hit) => RouteResult::Routed(vec![
                GameEvent::EntityHit {
                    target_id: enemy_hit.target_id,
                },
                GameEvent::LocalPlayerHit {
                    target_id: enemy_hit.target_id,
                    bullet_id: enemy_hit.bullet_id,
                    shooter_id: enemy_hit.shooter_id,
                    main_id: enemy_hit.main_id,
                },
            ]),

            // ----- Player shoot (outgoing) -----
            ParsedPacket::PlayerShoot(shoot) => {
                RouteResult::Routed(vec![GameEvent::LocalPlayerShot {
                    bullet_id: shoot.bullet_id,
                    weapon_id: shoot.weapon_id as i32,
                    projectile_id: shoot.projectile_id as i32,
                    client_time: shoot.time,
                    shot_x: shoot.player_position.x,
                    shot_y: shoot.player_position.y,
                    angle: shoot.angle,
                }])
            }

            // ----- Server player shoot (incoming: summon/ally shots) -----
            ParsedPacket::ServerPlayerShoot(shoot) => {
                RouteResult::Routed(vec![GameEvent::AllyOrSummonShot {
                    owner_id: shoot.owner_id,
                    summoner_id: shoot.summoner_id,
                    bullet_id: shoot.bullet_id,
                    bullet_count: shoot.bullet_count,
                    damage: shoot.damage,
                    container_type: shoot.container_type,
                    bullet_type: shoot.bullet_type,
                }])
            }

            // ----- Enemy shoot (incoming) -----
            ParsedPacket::EnemyShoot(shoot) => RouteResult::Routed(vec![GameEvent::EnemyShot {
                owner_id: shoot.owner_id,
                bullet_id: shoot.bullet_id,
                bullet_type: shoot.bullet_type,
                damage: shoot.damage,
                num_shots: shoot.num_shots,
            }]),

            // ----- Player hit (outgoing: local player was hit) -----
            ParsedPacket::PlayerHit(hit) => {
                RouteResult::Routed(vec![GameEvent::LocalPlayerWasHit {
                    bullet_id: hit.bullet_id,
                    enemy_id: hit.object_id,
                }])
            }

            // ----- Use item (outgoing: local player used an ability/consumable) -----
            ParsedPacket::UseItem(use_item) => {
                RouteResult::Routed(vec![GameEvent::LocalItemUsed {
                    item_id: use_item.slot_object.item_type,
                    x: use_item.use_item_position.x,
                    y: use_item.use_item_position.y,
                }])
            }

            // ----- Accelerator activated (incoming: account boost consumed) -----
            ParsedPacket::AcceleratorActivated(p) => {
                RouteResult::Routed(vec![GameEvent::AccountAcceleratorActivated {
                    object_type: p.object_type,
                }])
            }

            // ----- Damage (incoming) -----
            ParsedPacket::Damage(damage) => {
                if damage.damage_amount > 0 {
                    RouteResult::Routed(vec![
                        GameEvent::EntityHit {
                            target_id: damage.target_id,
                        },
                        GameEvent::DamageDealt {
                            target_id: damage.target_id,
                            attacker_id: damage.object_id,
                            amount: damage.damage_amount,
                        },
                    ])
                } else {
                    RouteResult::Routed(vec![])
                }
            }

            // ----- No-op packets (handled, no events) -----
            ParsedPacket::QuestObjectId(_) | ParsedPacket::RealmHeroesLeft(_) => {
                // Already logged by the packet parser; nothing else to do.
                RouteResult::Routed(vec![])
            }

            // ----- Create (outgoing) -----
            ParsedPacket::Create(create) => {
                tracing::info!(
                    "[CREATE] New character creation: class={}, skin={}, seasonal={}",
                    create.class_type,
                    create.skin_type,
                    create.is_seasonal
                );
                session.pending_create = Some(PendingCreate {
                    class_id: create.class_type,
                    skin_id: create.skin_type,
                    is_seasonal: create.is_seasonal,
                });
                RouteResult::Routed(vec![])
            }

            // ----- CreateSuccess -----
            ParsedPacket::CreateSuccess(create_success) => {
                let new_character = session.pending_create.take();
                session.on_create_success(create_success.object_id, create_success.char_id);

                tracing::info!(
                    "[CREATE_SUCCESS] Player loaded (object_id={}, char_id={})",
                    create_success.object_id,
                    create_success.char_id
                );
                if let Some(ref pending) = new_character {
                    tracing::info!(
                        "[CREATE_SUCCESS] New character created! char_id={}, class={}, seasonal={}",
                        create_success.char_id,
                        pending.class_id,
                        pending.is_seasonal
                    );
                }

                RouteResult::Routed(vec![GameEvent::PlayerLoaded {
                    char_id: create_success.char_id,
                    object_id: create_success.object_id,
                    new_character,
                    pc_stats: create_success.pc_stats.clone(),
                }])
            }

            // ----- NewCharacterInfo -----
            ParsedPacket::NewCharacterInfo(char_info) => {
                tracing::info!(
                    "[NEW_CHARACTER_INFO] Received character data (xml_len={})",
                    char_info.character_xml.len()
                );
                tracing::trace!(
                    "[NEW_CHARACTER_INFO] Full XML content:\n{}",
                    char_info.character_xml
                );

                if char_info.has_character_data() {
                    let xml = char_info.get_parseable_xml();
                    RouteResult::Routed(vec![GameEvent::CharacterXmlReceived { xml }])
                } else {
                    RouteResult::Routed(vec![])
                }
            }

            // ----- IncomingPartyMemberInfo -----
            ParsedPacket::IncomingPartyMemberInfo(party_info) => {
                RouteResult::Routed(vec![GameEvent::PartyListReceived(party_info.clone())])
            }

            // ----- PartyMemberAdded -----
            ParsedPacket::PartyMemberAdded(member_added) => {
                RouteResult::Routed(vec![GameEvent::PartyMemberJoined(member_added.clone())])
            }

            // ----- PartyRequestResponse -----
            ParsedPacket::PartyRequestResponse(response) => {
                RouteResult::Routed(vec![GameEvent::PartyJoinRequestResponse(response.clone())])
            }

            // ----- PartyJoinRequest -----
            ParsedPacket::PartyJoinRequest(_) => RouteResult::Routed(vec![]),

            // ----- PartyActionResult -----
            ParsedPacket::PartyActionResult(_) => RouteResult::Routed(vec![]),

            // ----- MapInfo -----
            ParsedPacket::MapInfo(map_info) => {
                let display_name = map_info.display_name.clone();
                let allows_api = map_info.allows_char_list_api();
                tracing::info!(
                    "[MAP_INFO] Entering map: '{}' (allows_api={})",
                    display_name,
                    allows_api
                );

                // Update session: max realm score
                if map_info.max_realm_score > 0 {
                    session.map.max_realm_score = Some(map_info.max_realm_score);
                } else {
                    session.map.max_realm_score = None;
                }
                session.map.name = Some(display_name.clone());
                session.map.allows_api = allows_api;

                // Clear pet + vault tracking on map change
                let is_vault = display_name == "{s.vault}";
                session.on_map_change(is_vault);

                let is_dungeon = map_info.is_dungeon_for_feed();
                let (dungeon_modifiers, dungeon_grade) = map_info.decode_dungeon_modifiers();

                // TEMPORARY: log raw dungeon modifier wire data so we can verify
                // the exact token format from a real modified dungeon, then build
                // the loot-bonus lookup table. Remove once tokens are confirmed.
                if !map_info.dungeon_modifiers.trim().is_empty() {
                    tracing::info!(
                        "[DUNGEON_MODIFIERS] map='{}' raw='{}' tokens={:?} grade={:?}",
                        display_name,
                        map_info.dungeon_modifiers,
                        dungeon_modifiers,
                        dungeon_grade
                    );
                }

                RouteResult::Routed(vec![GameEvent::MapChanged {
                    name: map_info.name.clone(),
                    display_name,
                    realm_name: map_info.realm_name.clone(),
                    fp: map_info.fp,
                    current_realm_score: map_info.current_realm_score,
                    max_realm_score: map_info.max_realm_score,
                    allows_api,
                    is_dungeon,
                    dungeon_modifiers,
                    dungeon_grade,
                }])
            }

            // ----- VaultContent -----
            ParsedPacket::VaultContent(vault) => {
                // Only process vault packets when actually in the vault
                let in_vault = session
                    .map
                    .name
                    .as_ref()
                    .map(|name| name == "{s.vault}")
                    .unwrap_or(false);

                if !in_vault {
                    RouteResult::Routed(vec![])
                } else {
                    let vault_type = if session.player.is_seasonal == Some(true) {
                        VaultType::Seasonal
                    } else {
                        VaultType::Regular
                    };

                    // Store object IDs for real-time tracking in NewTick
                    session.vault.chest_object_id = Some(vault.vault_chest_object_id);
                    session.vault.material_chest_object_id = Some(vault.material_chest_object_id);
                    session.vault.gift_chest_object_id = Some(vault.gift_chest_object_id);
                    session.vault.potion_storage_object_id = Some(vault.potion_storage_object_id);

                    // Initialize page tracking to first page
                    session.vault.active_vault_page = Some(0);
                    session.vault.active_material_page = Some(0);
                    session.vault.active_gift_page = Some(0);
                    session.vault.active_potion_page = Some(0);

                    RouteResult::Routed(vec![GameEvent::VaultContentReceived {
                        packet: vault.clone(),
                        vault_type,
                    }])
                }
            }

            // ----- Text -----
            ParsedPacket::Text(text) => {
                RouteResult::Routed(vec![GameEvent::TextReceived(text.clone())])
            }

            // ----- Crucible definitions -----
            ParsedPacket::CrucibleResponse(resp) => {
                let defs = crate::api::crucible::parse_crucible_defs(&resp.crucible_jsons);
                if defs.is_empty() {
                    RouteResult::Routed(vec![])
                } else {
                    RouteResult::Routed(vec![GameEvent::CrucibleDefinitionsReceived(defs)])
                }
            }

            // ----- Hello -----
            // Note: Multi-client isolation is handled by the app before routing,
            // because it requires raw packet network metadata and the reassembler.
            ParsedPacket::Hello(hello) => {
                let mut events = Vec::new();
                if !hello.access_token.is_empty() {
                    tracing::info!(
                        "[HELLO] Captured access token: len={}, gameId={}, version={}",
                        hello.access_token.len(),
                        hello.game_id,
                        hello.build_version
                    );
                    events.push(GameEvent::HelloReceived {
                        access_token: hello.access_token.clone(),
                    });
                }
                RouteResult::Routed(events)
            }

            // ----- Update -----
            ParsedPacket::Update(update) => {
                let time_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;

                let mut events = vec![GameEvent::UpdateReceived(update.clone(), time_ms)];

                // Session state: seasonal status
                if let Some(player_id) = session.player.object_id {
                    if let Some(is_seasonal) = update.is_object_seasonal(player_id) {
                        if session.player.is_seasonal != Some(is_seasonal) {
                            session.player.is_seasonal = Some(is_seasonal);
                            tracing::info!(
                                "[UPDATE] Player seasonal status: {} (object_id={})",
                                if is_seasonal { "SEASONAL" } else { "REGULAR" },
                                player_id
                            );
                            // Emit event so characters panel can update cache
                            if let Some(char_id) = session.player.char_id {
                                events.push(GameEvent::SeasonalStatusReceived {
                                    char_id,
                                    is_seasonal,
                                });
                            }
                        }
                    }

                    // Session state: crucible status. A map-load snapshot may
                    // carry the CRUCIBLE stat present-but-empty on non-crucible
                    // areas, and identity is reset on every area switch (see
                    // `on_create_success`), so honouring an empty value here would
                    // spuriously clear the flag on every zone change. A full
                    // snapshot may therefore only *confirm* an active crucible;
                    // an actual deactivation is authoritative only from a NewTick
                    // delta (a genuine transition), handled below. Require char_id
                    // so the session state and the cache event move together.
                    if let Some(char_id) = session.player.char_id {
                        if update.is_object_crucible(player_id) == Some(true)
                            && session.player.is_crucible != Some(true)
                        {
                            session.player.is_crucible = Some(true);
                            tracing::info!(
                                "[UPDATE] Player crucible status: ACTIVE (object_id={})",
                                player_id
                            );
                            events.push(GameEvent::CrucibleStatusReceived {
                                char_id,
                                is_active: true,
                            });
                        }
                    }

                    // Session state: loot-drop boost. The player's LootDropTimer
                    // stat carries the remaining boost seconds; the game already
                    // pauses this in safe areas / loading, so store the value
                    // as-is for the character card. Tri-state: only act when the
                    // stat is present (absence carries no information and must
                    // not clear a live boost).
                    if let Some(secs) = update.loot_drop_timer_for(player_id) {
                        session.player.loot_boost_secs =
                            if secs > 0 { Some(secs as u32) } else { None };
                    }
                }

                // Session state: pet detection from new objects
                for obj in &update.new_objects {
                    use crate::protocol::data::StatType;
                    let has_pet_type = obj
                        .status
                        .stats
                        .iter()
                        .any(|s| s.stat_type == StatType::PetType);
                    let has_pet_name = obj
                        .status
                        .stats
                        .iter()
                        .any(|s| s.stat_type == StatType::PetName);

                    if has_pet_type || has_pet_name {
                        // Record this object as a known pet so the processor can
                        // buffer its spawn status (full inventory + identity ride
                        // only in the spawn Update) for replay once the active
                        // pet is reconciled.
                        session.player.known_pet_object_ids.insert(obj.object_id());

                        let pet_name = obj
                            .status
                            .stats
                            .iter()
                            .find(|s| s.stat_type == StatType::PetName)
                            .and_then(|s| s.string_stat_value.as_ref())
                            .cloned()
                            .unwrap_or_else(|| "Unknown".to_string());

                        let owner_account = obj
                            .status
                            .stats
                            .iter()
                            .find(|s| s.stat_type == StatType::OwnerAccountId)
                            .and_then(|s| s.string_stat_value.as_ref());

                        // Only claim a pet when ownership is positively confirmed:
                        // the object's OwnerAccountId must match our saved account id.
                        // Anything we can't confirm is left to the position-gated
                        // NewTick heuristic, avoiding false positives from foreign pets.
                        let is_our_pet =
                            match (&owner_account, &session.connection.saved_account_id) {
                                (Some(owner), Some(saved)) => *owner == saved,
                                _ => false,
                            };

                        if is_our_pet {
                            tracing::debug!(
                                "[PET] Detected player's pet '{}': object_id={}",
                                pet_name,
                                obj.object_id()
                            );
                            session.player.pet_object_id = Some(obj.object_id());
                        }
                    }
                }

                // Extract dust stats from player object
                if let Some(player_id) = session.player.object_id {
                    // Only route dust / account stats once the character's
                    // seasonal status is known AND this connection is the
                    // verified main account. During a portal reconnect the
                    // identity is briefly cleared (is_seasonal = None), and a
                    // mule that opened before the main is only a tentative main
                    // until its account ID is verified -- emitting in either
                    // case would flash the widgets to the wrong account.
                    if let (Some(is_seasonal), true) = (
                        session.player.is_seasonal,
                        session.connection.is_account_verified(),
                    ) {
                        if let Some(dust_event) = Self::extract_dust_from_objects(
                            &update.new_objects,
                            player_id,
                            is_seasonal,
                        ) {
                            events.push(dust_event);
                        }

                        // Extract account-level Widget Bar stats from the player object.
                        if let Some(obj) = update
                            .new_objects
                            .iter()
                            .find(|o| o.object_id() == player_id)
                        {
                            if let Some(ev) =
                                Self::extract_account_stats(&obj.status.stats, is_seasonal)
                            {
                                events.push(ev);
                            }
                        }
                    }
                }

                RouteResult::Routed(events)
            }

            // ----- NewTick -----
            ParsedPacket::NewTick(new_tick) => {
                let time_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;

                // Session state: pet detection from NewTick if not yet known.
                //
                // Attribute the pet from any inventory-bearing object near the
                // player. This is deliberately permissive so the equipped pet's
                // inventory updates live in every instance (Vault, Guild Hall,
                // dungeons, etc.), matching the earlier behaviour. Self and the
                // vault storage objects are skipped.
                if session.player.pet_object_id.is_none() {
                    use crate::protocol::data::StatType;

                    let player_pos = session
                        .player
                        .object_id
                        .and_then(|pid| new_tick.find_status(pid).map(|s| (s.pos.x, s.pos.y)));

                    for status in &new_tick.statuses {
                        if Some(status.object_id) == session.player.object_id {
                            continue;
                        }
                        if Some(status.object_id) == session.vault.chest_object_id
                            || Some(status.object_id) == session.vault.material_chest_object_id
                            || Some(status.object_id) == session.vault.gift_chest_object_id
                            || Some(status.object_id) == session.vault.potion_storage_object_id
                        {
                            continue;
                        }
                        let has_inventory = status.stats.iter().any(|s| {
                            matches!(
                                s.stat_type,
                                StatType::Inventory0
                                    | StatType::Inventory1
                                    | StatType::Inventory2
                                    | StatType::Inventory3
                                    | StatType::Inventory4
                                    | StatType::Inventory5
                                    | StatType::Inventory6
                                    | StatType::Inventory7
                            )
                        });

                        if has_inventory {
                            let is_close_to_player = player_pos
                                .map(|(px, py)| {
                                    let dx = (status.pos.x - px).abs();
                                    let dy = (status.pos.y - py).abs();
                                    dx < 3.0 && dy < 3.0
                                })
                                .unwrap_or(true);

                            if is_close_to_player {
                                session.player.pet_object_id = Some(status.object_id);
                                break;
                            }
                        }
                    }
                }

                // Extract dust stats from player in NewTick
                let mut events = vec![GameEvent::NewTickReceived(new_tick.clone(), time_ms)];
                if let Some(player_id) = session.player.object_id {
                    // Crucible status. The player's CRUCIBLE stat is streamed as a
                    // NewTick delta (it is not guaranteed to be in the map-load
                    // snapshot), so watch for it here too. Tri-state: only act
                    // when the stat is actually present -- an omitted stat carries
                    // no information and must not clear a live-detected flag.
                    // Require char_id so the session state and the cache event
                    // always move together (a delta only arrives once).
                    if session.connection.is_account_verified() {
                        if let Some(char_id) = session.player.char_id {
                            if let Some(is_crucible) = new_tick
                                .find_status(player_id)
                                .and_then(|s| s.crucible_stat_state())
                            {
                                if session.player.is_crucible != Some(is_crucible) {
                                    session.player.is_crucible = Some(is_crucible);
                                    tracing::info!(
                                        "[NEWTICK] Player crucible status: {} (object_id={})",
                                        if is_crucible { "ACTIVE" } else { "INACTIVE" },
                                        player_id
                                    );
                                    events.push(GameEvent::CrucibleStatusReceived {
                                        char_id,
                                        is_active: is_crucible,
                                    });
                                }
                            }
                        }
                    }

                    // Session state: loot-drop boost (see the Update arm). The
                    // LootDropTimer streams as a NewTick delta too, so watch it
                    // here to keep the value fresh. Gated on the verified main so
                    // a mule's boost can't leak onto the card.
                    if session.connection.is_account_verified() {
                        if let Some(secs) = new_tick
                            .find_status(player_id)
                            .and_then(|s| s.loot_drop_timer())
                        {
                            session.player.loot_boost_secs =
                                if secs > 0 { Some(secs as u32) } else { None };
                        }
                    }

                    // Skip until the seasonal status is known and this is the
                    // verified main connection (see the Update arm) so a
                    // mid-reconnect tick or a mule can't flip the widgets to the
                    // wrong account.
                    if let (Some(is_seasonal), true) = (
                        session.player.is_seasonal,
                        session.connection.is_account_verified(),
                    ) {
                        if let Some(dust_event) = Self::extract_dust_from_statuses(
                            &new_tick.statuses,
                            player_id,
                            is_seasonal,
                        ) {
                            events.push(dust_event);
                        }

                        // Extract account-level Widget Bar stats from the player status.
                        if let Some(status) =
                            new_tick.statuses.iter().find(|s| s.object_id == player_id)
                        {
                            if let Some(ev) =
                                Self::extract_account_stats(&status.stats, is_seasonal)
                            {
                                events.push(ev);
                            }
                        }
                    }
                }

                RouteResult::Routed(events)
            }

            // ----- InvSwap -----
            ParsedPacket::InvSwap(inv_swap) => {
                RouteResult::Routed(vec![GameEvent::InvSwapReceived(inv_swap.clone())])
            }

            // ----- Reconnect -----
            ParsedPacket::Reconnect(reconnect) => {
                // Only update server name when the target host differs from the
                // current server (cross-server reconnect, e.g. party teleport).
                // Same-server reconnects (dungeons, vault, nexus) are ignored.
                let target_ip = reconnect.host.parse::<std::net::Ipv4Addr>().ok();
                let is_new_server = match (target_ip, session.connection.server_ip) {
                    (Some(target), Some(current)) => target != current,
                    _ => false,
                };

                tracing::info!(
                    "[RECONNECT] name='{}' host='{}' port={} current_server={:?} (cross_server={})",
                    reconnect.name,
                    reconnect.host,
                    reconnect.port,
                    session.connection.server_ip,
                    is_new_server
                );

                if is_new_server && !reconnect.name.is_empty() {
                    RouteResult::Routed(vec![GameEvent::ReconnectReceived {
                        server_name: reconnect.name.clone(),
                    }])
                } else {
                    RouteResult::Routed(vec![])
                }
            }

            // ----- ExaltationUpdate -----
            ParsedPacket::ExaltationUpdate(exalt) => {
                let exaltation = ClassExaltation::from_packet(
                    exalt.obj_type,
                    exalt.dexterity_progress,
                    exalt.speed_progress,
                    exalt.vitality_progress,
                    exalt.wisdom_progress,
                    exalt.defense_progress,
                    exalt.attack_progress,
                    exalt.mana_progress,
                    exalt.health_progress,
                );
                RouteResult::Routed(vec![GameEvent::ExaltationUpdated {
                    class_type: exalt.obj_type as i32,
                    exaltation,
                }])
            }

            // ----- Quest redeem / chest rewards -----
            ParsedPacket::QuestRedeem(p) => {
                RouteResult::Routed(vec![GameEvent::QuestRedeemAttempted {
                    quest_id: p.quest_id_string.clone(),
                }])
            }
            ParsedPacket::QuestRedeemResponse(p) => {
                tracing::debug!("[QUEST] {}", p.description());
                RouteResult::Routed(vec![GameEvent::QuestRedeemResult { ok: p.ok }])
            }
            ParsedPacket::ClaimRewardsInfoPrompt(p) => {
                tracing::debug!("[REWARD] {}", p.description());
                RouteResult::Routed(vec![])
            }
            ParsedPacket::ClaimChestReward(p) => {
                tracing::debug!("[REWARD] {}", p.description());
                RouteResult::Routed(vec![])
            }
            ParsedPacket::ChestRewardResult(p) => {
                tracing::debug!("[REWARD] {}", p.description());
                RouteResult::Routed(vec![])
            }

            // ----- UsePortal (outgoing): anchor dungeon join timer to spawn -----
            ParsedPacket::UsePortal(p) => RouteResult::Routed(vec![GameEvent::PortalUsed {
                object_id: p.object_id,
            }]),

            // ----- PartyAction (incoming from server) -----
            ParsedPacket::PartyAction(action) => {
                RouteResult::Routed(vec![GameEvent::PartyActionReceived {
                    player_id: action.player_id,
                    action_id: action.action_id,
                }])
            }

            // ----- ActivePetUpdate (incoming): live active-pet instance id -----
            ParsedPacket::ActivePetUpdate(p) => {
                // Ignore non-positive ids (treated as "no active pet"): leaving a
                // stale pet mapping is harmless for a viewer, and clearing risks
                // dropping a valid mapping on spurious packets.
                if p.instance_id > 0 {
                    let changed = session.player.active_pet_instance_id != Some(p.instance_id);
                    session.player.active_pet_instance_id = Some(p.instance_id);
                    // A pet swap (e.g. via the Pet Yard) does not reload the map,
                    // so drop the stale object lock. The newly equipped pet dashes
                    // onto the player and the NewTick detection re-acquires it.
                    if changed {
                        session.player.pet_object_id = None;
                    }
                    RouteResult::Routed(vec![GameEvent::ActivePetChanged {
                        instance_id: p.instance_id,
                    }])
                } else {
                    RouteResult::Routed(vec![])
                }
            }

            // ----- Notification (incoming): key-pop + area-unlock detection ----
            ParsedPacket::Notification(n) => {
                // Effect 8 == PortalOpened. A key-pop notification carries a
                // `"player"` field in its message; realm/other portal opens do
                // not. Forward only PortalOpened; the processor filters on the
                // player field.
                if n.effect == 8 {
                    RouteResult::Routed(vec![GameEvent::PortalOpened {
                        message: n.message.clone(),
                        picture_type: n.picture_type,
                    }])
                } else if n.effect == 1
                    && (n.message.contains("s.dungeon_unlocked_by")
                        || n.message.contains("s.something_by_player"))
                {
                    // ServerMessage broadcasts carry area-unlock (Wine Cellar /
                    // Void) and Lost Halls monument activations as JSON.
                    // Forward the raw message; the processor decodes name+popper.
                    RouteResult::Routed(vec![GameEvent::AreaUnlockBroadcast {
                        message: n.message.clone(),
                    }])
                } else {
                    RouteResult::Routed(vec![])
                }
            }

            // ----- GlobalNotification (incoming): server-wide broadcast -----
            ParsedPacket::GlobalNotification(_g) => {
                // Server-wide banners (ID 66) are not the area-unlock carrier
                // (confirmed from captures -- those arrive as ServerMessage
                // notifications). Nothing to route yet.
                RouteResult::Routed(vec![])
            }

            // ----- TradeRequested (incoming): another player offered a trade ---
            ParsedPacket::TradeRequested(t) => {
                RouteResult::Routed(vec![GameEvent::TradeRequested {
                    name: t.name.clone(),
                }])
            }

            // All known packet types are now routed.
            // Unmapped kept for forward compatibility with new ParsedPacket variants.
            #[allow(unreachable_patterns)]
            _ => RouteResult::Unmapped,
        }
    }

    /// Extract dust stats from new objects (Update packet).
    fn extract_dust_from_objects(
        objects: &[crate::protocol::data::ObjectData],
        player_id: i32,
        is_seasonal: bool,
    ) -> Option<GameEvent> {
        use crate::protocol::data::StatType;

        for obj in objects {
            if obj.object_id() != player_id {
                continue;
            }

            let dust_stat = obj
                .status
                .stats
                .iter()
                .find(|s| s.stat_type == StatType::Dust)
                .and_then(|s| s.string_stat_value.as_ref());

            let dust_amount_stat = obj
                .status
                .stats
                .iter()
                .find(|s| s.stat_type == StatType::DustAmount)
                .and_then(|s| s.string_stat_value.as_ref());

            if let (Some(amounts_str), Some(caps_str)) = (dust_stat, dust_amount_stat) {
                if let Some(amounts) = crate::dust::parse_dust_stat(amounts_str, caps_str) {
                    tracing::debug!(
                        "[DUST] Updated from Update: G={}/{} R={}/{} P={}/{} (seasonal={})",
                        amounts.green.0,
                        amounts.green.1,
                        amounts.red.0,
                        amounts.red.1,
                        amounts.purple.0,
                        amounts.purple.1,
                        is_seasonal
                    );
                    return Some(GameEvent::DustUpdated {
                        amounts,
                        is_seasonal,
                    });
                }
            }
        }
        None
    }

    /// Extract dust stats from object statuses (NewTick packet).
    fn extract_dust_from_statuses(
        statuses: &[crate::protocol::data::ObjectStatusData],
        player_id: i32,
        is_seasonal: bool,
    ) -> Option<GameEvent> {
        use crate::protocol::data::StatType;

        for status in statuses {
            if status.object_id != player_id {
                continue;
            }

            let dust_stat = status
                .stats
                .iter()
                .find(|s| s.stat_type == StatType::Dust)
                .and_then(|s| s.string_stat_value.as_ref());

            let dust_amount_stat = status
                .stats
                .iter()
                .find(|s| s.stat_type == StatType::DustAmount)
                .and_then(|s| s.string_stat_value.as_ref());

            if let (Some(amounts_str), Some(caps_str)) = (dust_stat, dust_amount_stat) {
                if let Some(amounts) = crate::dust::parse_dust_stat(amounts_str, caps_str) {
                    return Some(GameEvent::DustUpdated {
                        amounts,
                        is_seasonal,
                    });
                }
            }
        }
        None
    }

    /// Extract account-level Widget Bar stats (stars, fame, gold, forge fire,
    /// materials) from a slice of the local player's stats. Returns an event
    /// only when at least one value was present.
    fn extract_account_stats(
        stats: &[crate::protocol::data::StatData],
        is_seasonal: bool,
    ) -> Option<GameEvent> {
        use crate::account_stats::{parse_material_stat, AccountStatsUpdate};
        use crate::protocol::data::StatType;

        let mut update = AccountStatsUpdate {
            is_seasonal,
            ..Default::default()
        };

        for s in stats {
            match s.stat_type {
                StatType::Stars => update.stars = Some(s.stat_value),
                StatType::Fame => update.account_fame = Some(s.stat_value),
                StatType::Credits => update.gold = Some(s.stat_value),
                // The game sends -1 for Forge Fire during portal loading
                // screens; treat any negative value as "unknown" so it doesn't
                // overwrite the last good value (avoids a -1 flicker).
                StatType::ForgeFire => {
                    if s.stat_value >= 0 {
                        update.forge_fire = Some(s.stat_value);
                    }
                }
                StatType::Material => {
                    if let Some(v) = s.string_stat_value.as_deref() {
                        update.material_current = parse_material_stat(v);
                    }
                }
                StatType::MaterialCap => {
                    if let Some(v) = s.string_stat_value.as_deref() {
                        update.material_cap = parse_material_stat(v);
                    }
                }
                _ => {}
            }
        }

        if update.is_empty() {
            None
        } else {
            Some(GameEvent::AccountStatsUpdated(update))
        }
    }
}

impl Default for PacketRouter {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::data::{PartyPlayerData, QuestData};
    use crate::protocol::packets::{
        CreatePacket, CreateSuccessPacket, DamagePacket, DeathPacket, EnemyHitPacket, HelloPacket,
        IncomingPartyMemberInfoPacket, InvSwapPacket, MapInfoPacket, NewCharacterInfoPacket,
        PartyMemberAddedPacket, QuestFetchResponsePacket, QuestObjectIdPacket,
        RealmHeroesLeftPacket, RealmScoreUpdatePacket, SlotObjectData, TextPacket,
        VaultContentPacket,
    };
    use crate::session::GameSession;

    fn make_session() -> GameSession {
        let mut s = GameSession::new();
        s.player.object_id = Some(1);
        s.player.char_id = Some(100);
        s
    }

    // -- Death --

    #[test]
    fn death_emits_character_died_event() {
        let mut router = PacketRouter::new();
        let mut session = make_session();

        let packet = ParsedPacket::Death(DeathPacket {
            account_id: "abc".into(),
            char_id: 100,
            killed_by: "Medusa".into(),
            gravestone_type: 1,
            total_fame: 5000,
            fame_data: vec![],
            stats: String::new(),
        });

        let result = router.route(&packet, &mut session);

        match result {
            RouteResult::Routed(events) => {
                assert_eq!(events.len(), 1);
                match &events[0] {
                    GameEvent::CharacterDied {
                        char_id,
                        killed_by,
                        total_fame,
                        gravestone_type,
                    } => {
                        assert_eq!(*char_id, 100);
                        assert_eq!(killed_by, "Medusa");
                        assert_eq!(*total_fame, 5000);
                        assert_eq!(*gravestone_type, 1);
                    }
                    other => panic!("Expected CharacterDied, got {:?}", other),
                }
            }
            RouteResult::Unmapped => panic!("Expected Routed, got Unmapped"),
        }
    }

    #[test]
    fn death_clears_session_player() {
        let mut router = PacketRouter::new();
        let mut session = make_session();
        assert!(session.player.char_id.is_some());

        let packet = ParsedPacket::Death(DeathPacket {
            account_id: "abc".into(),
            char_id: 100,
            killed_by: "Dragon".into(),
            gravestone_type: 1,
            total_fame: 100,
            fame_data: vec![],
            stats: String::new(),
        });

        router.route(&packet, &mut session);
        // on_death clears player identity
        assert!(session.player.char_id.is_none());
    }

    // -- Quest fetch response --

    #[test]
    fn quest_fetch_emits_quests_received() {
        let mut router = PacketRouter::new();
        let mut session = make_session();

        let quest = QuestData {
            id: "daily1".into(),
            name: "Collect Marks".into(),
            description: "Get 3 marks".into(),
            expiration: String::new(),
            category: 1,
            unknown_int: 0,
            requirements: vec![],
            rewards: vec![],
            completed: false,
            item_of_choice: false,
            repeatable: false,
        };
        let packet = ParsedPacket::QuestFetchResponse(QuestFetchResponsePacket {
            quests: vec![quest.clone()],
            next_refresh_price: 50,
            unknown: 0,
        });

        let result = router.route(&packet, &mut session);

        match result {
            RouteResult::Routed(events) => {
                assert_eq!(events.len(), 1);
                match &events[0] {
                    GameEvent::QuestsReceived(resp) => {
                        assert_eq!(resp.quests.len(), 1);
                        assert_eq!(resp.quests[0].name, "Collect Marks");
                        assert_eq!(resp.next_refresh_price, 50);
                    }
                    other => panic!("Expected QuestsReceived, got {:?}", other),
                }
            }
            RouteResult::Unmapped => panic!("Expected Routed"),
        }
    }

    // -- Realm score update --

    #[test]
    fn realm_score_emits_event() {
        let mut router = PacketRouter::new();
        let mut session = make_session();

        let packet = ParsedPacket::RealmScoreUpdate(RealmScoreUpdatePacket { score: 42 });
        let result = router.route(&packet, &mut session);

        match result {
            RouteResult::Routed(events) => {
                assert_eq!(events.len(), 1);
                match &events[0] {
                    GameEvent::RealmScoreChanged { score } => assert_eq!(*score, 42),
                    other => panic!("Expected RealmScoreChanged, got {:?}", other),
                }
            }
            RouteResult::Unmapped => panic!("Expected Routed"),
        }
    }

    // -- Enemy hit --

    #[test]
    fn enemy_hit_emits_entity_hit() {
        let mut router = PacketRouter::new();
        let mut session = make_session();

        let packet = ParsedPacket::EnemyHit(EnemyHitPacket {
            time: 0,
            bullet_id: 1,
            shooter_id: 10,
            target_id: 999,
            kill: false,
            main_id: 0,
        });

        let result = router.route(&packet, &mut session);

        match result {
            RouteResult::Routed(events) => {
                assert_eq!(events.len(), 2);
                match &events[0] {
                    GameEvent::EntityHit { target_id } => assert_eq!(*target_id, 999),
                    other => panic!("Expected EntityHit, got {:?}", other),
                }
                match &events[1] {
                    GameEvent::LocalPlayerHit { target_id, .. } => assert_eq!(*target_id, 999),
                    other => panic!("Expected LocalPlayerHit, got {:?}", other),
                }
            }
            RouteResult::Unmapped => panic!("Expected Routed"),
        }
    }

    // -- Damage --

    #[test]
    fn damage_with_positive_amount_emits_entity_hit() {
        let mut router = PacketRouter::new();
        let mut session = make_session();

        let packet = ParsedPacket::Damage(DamagePacket {
            target_id: 555,
            effects: vec![],
            damage_amount: 100,
            damage_properties: false,
            bullet_id: 1,
            object_id: 10,
            unknown: 0,
        });

        let result = router.route(&packet, &mut session);

        match result {
            RouteResult::Routed(events) => {
                assert_eq!(events.len(), 2);
                match &events[0] {
                    GameEvent::EntityHit { target_id } => assert_eq!(*target_id, 555),
                    other => panic!("Expected EntityHit, got {:?}", other),
                }
                match &events[1] {
                    GameEvent::DamageDealt {
                        target_id,
                        attacker_id,
                        amount,
                    } => {
                        assert_eq!(*target_id, 555);
                        assert_eq!(*attacker_id, 10);
                        assert_eq!(*amount, 100);
                    }
                    other => panic!("Expected DamageDealt, got {:?}", other),
                }
            }
            RouteResult::Unmapped => panic!("Expected Routed"),
        }
    }

    #[test]
    fn damage_with_zero_amount_emits_no_events() {
        let mut router = PacketRouter::new();
        let mut session = make_session();

        let packet = ParsedPacket::Damage(DamagePacket {
            target_id: 555,
            effects: vec![],
            damage_amount: 0,
            damage_properties: false,
            bullet_id: 1,
            object_id: 10,
            unknown: 0,
        });

        let result = router.route(&packet, &mut session);

        match result {
            RouteResult::Routed(events) => assert!(events.is_empty()),
            RouteResult::Unmapped => panic!("Expected Routed"),
        }
    }

    // -- No-op packets --

    #[test]
    fn active_pet_update_routes_and_buffers_instance_id() {
        use crate::protocol::packets::ActivePetUpdatePacket;
        let mut router = PacketRouter::new();
        let mut session = make_session();

        let packet = ParsedPacket::ActivePetUpdate(ActivePetUpdatePacket { instance_id: 4242 });
        let result = router.route(&packet, &mut session);

        match result {
            RouteResult::Routed(events) => {
                assert!(events
                    .iter()
                    .any(|e| matches!(e, GameEvent::ActivePetChanged { instance_id: 4242 })));
            }
            RouteResult::Unmapped => panic!("Expected Routed"),
        }
        assert_eq!(session.player.active_pet_instance_id, Some(4242));
    }

    #[test]
    fn active_pet_update_swap_reopens_detection_window() {
        use crate::protocol::packets::ActivePetUpdatePacket;
        let mut router = PacketRouter::new();
        let mut session = make_session();

        // Simulate a stable, already-latched pet.
        session.player.active_pet_instance_id = Some(1111);
        session.player.pet_object_id = Some(9999);

        // A swap to a different pet instance arrives (no map reload).
        let packet = ParsedPacket::ActivePetUpdate(ActivePetUpdatePacket { instance_id: 2222 });
        router.route(&packet, &mut session);

        assert_eq!(session.player.active_pet_instance_id, Some(2222));
        // The stale object lock must clear so the newly equipped pet re-latches.
        assert_eq!(session.player.pet_object_id, None);
    }

    #[test]
    fn active_pet_update_ignores_non_positive_instance_id() {
        use crate::protocol::packets::ActivePetUpdatePacket;
        let mut router = PacketRouter::new();
        let mut session = make_session();

        let packet = ParsedPacket::ActivePetUpdate(ActivePetUpdatePacket { instance_id: 0 });
        let result = router.route(&packet, &mut session);

        match result {
            RouteResult::Routed(events) => assert!(events.is_empty()),
            RouteResult::Unmapped => panic!("Expected Routed"),
        }
        assert_eq!(session.player.active_pet_instance_id, None);
    }

    #[test]
    fn quest_object_id_is_routed_with_no_events() {
        let mut router = PacketRouter::new();
        let mut session = make_session();

        let packet = ParsedPacket::QuestObjectId(QuestObjectIdPacket {
            object_id: 1,
            list: vec![],
        });

        let result = router.route(&packet, &mut session);

        match result {
            RouteResult::Routed(events) => assert!(events.is_empty()),
            RouteResult::Unmapped => panic!("Expected Routed"),
        }
    }

    #[test]
    fn realm_heroes_left_is_routed_with_no_events() {
        let mut router = PacketRouter::new();
        let mut session = make_session();

        let packet = ParsedPacket::RealmHeroesLeft(RealmHeroesLeftPacket { heroes_left: 5 });

        let result = router.route(&packet, &mut session);

        match result {
            RouteResult::Routed(events) => assert!(events.is_empty()),
            RouteResult::Unmapped => panic!("Expected Routed"),
        }
    }

    // -- Create --

    #[test]
    fn create_stores_pending_create_in_session() {
        let mut router = PacketRouter::new();
        let mut session = make_session();
        assert!(session.pending_create.is_none());

        let packet = ParsedPacket::Create(CreatePacket {
            class_type: 782,
            skin_type: 5,
            is_challenger: false,
            is_seasonal: true,
        });

        let result = router.route(&packet, &mut session);
        match result {
            RouteResult::Routed(events) => assert!(events.is_empty()),
            RouteResult::Unmapped => panic!("Expected Routed"),
        }
        let pending = session.pending_create.as_ref().unwrap();
        assert_eq!(pending.class_id, 782);
        assert_eq!(pending.skin_id, 5);
        assert!(pending.is_seasonal);
    }

    // -- CreateSuccess --

    #[test]
    fn create_success_emits_player_loaded() {
        let mut router = PacketRouter::new();
        let mut session = make_session();

        let packet = ParsedPacket::CreateSuccess(CreateSuccessPacket {
            object_id: 42,
            char_id: 200,
            pc_stats: "stats123".into(),
        });

        let result = router.route(&packet, &mut session);
        match result {
            RouteResult::Routed(events) => {
                assert_eq!(events.len(), 1);
                match &events[0] {
                    GameEvent::PlayerLoaded {
                        char_id,
                        object_id,
                        new_character,
                        pc_stats,
                    } => {
                        assert_eq!(*char_id, 200);
                        assert_eq!(*object_id, 42);
                        assert!(new_character.is_none());
                        assert_eq!(pc_stats, "stats123");
                    }
                    other => panic!("Expected PlayerLoaded, got {:?}", other),
                }
            }
            RouteResult::Unmapped => panic!("Expected Routed"),
        }
        assert_eq!(session.player.object_id, Some(42));
        assert_eq!(session.player.char_id, Some(200));
    }

    #[test]
    fn create_success_includes_pending_create() {
        let mut router = PacketRouter::new();
        let mut session = make_session();

        // First, send a Create packet
        let create = ParsedPacket::Create(CreatePacket {
            class_type: 782,
            skin_type: 0,
            is_challenger: false,
            is_seasonal: true,
        });
        router.route(&create, &mut session);

        // Then CreateSuccess
        let cs = ParsedPacket::CreateSuccess(CreateSuccessPacket {
            object_id: 42,
            char_id: 300,
            pc_stats: String::new(),
        });
        let result = router.route(&cs, &mut session);
        match result {
            RouteResult::Routed(events) => {
                assert_eq!(events.len(), 1);
                match &events[0] {
                    GameEvent::PlayerLoaded { new_character, .. } => {
                        let nc = new_character.as_ref().unwrap();
                        assert_eq!(nc.class_id, 782);
                        assert!(nc.is_seasonal);
                    }
                    other => panic!("Expected PlayerLoaded, got {:?}", other),
                }
            }
            RouteResult::Unmapped => panic!("Expected Routed"),
        }
        // pending_create should be consumed
        assert!(session.pending_create.is_none());
    }

    // -- NewCharacterInfo --

    #[test]
    fn new_character_info_emits_xml_received() {
        let mut router = PacketRouter::new();
        let mut session = make_session();

        let packet = ParsedPacket::NewCharacterInfo(NewCharacterInfoPacket {
            size: 0,
            character_xml: "<Char id=\"1\"><Level>20</Level></Char>".into(),
        });

        let result = router.route(&packet, &mut session);
        match result {
            RouteResult::Routed(events) => {
                assert_eq!(events.len(), 1);
                match &events[0] {
                    GameEvent::CharacterXmlReceived { xml } => {
                        assert!(xml.contains("<Char"));
                    }
                    other => panic!("Expected CharacterXmlReceived, got {:?}", other),
                }
            }
            RouteResult::Unmapped => panic!("Expected Routed"),
        }
    }

    #[test]
    fn new_character_info_empty_xml_no_event() {
        let mut router = PacketRouter::new();
        let mut session = make_session();

        let packet = ParsedPacket::NewCharacterInfo(NewCharacterInfoPacket {
            size: 0,
            character_xml: "empty".into(),
        });

        let result = router.route(&packet, &mut session);
        match result {
            RouteResult::Routed(events) => assert!(events.is_empty()),
            RouteResult::Unmapped => panic!("Expected Routed"),
        }
    }

    // -- IncomingPartyMemberInfo --

    #[test]
    fn party_list_emits_party_list_received() {
        let mut router = PacketRouter::new();
        let mut session = make_session();

        let packet = ParsedPacket::IncomingPartyMemberInfo(IncomingPartyMemberInfoPacket {
            party_id: 10,
            leader_id: 1,
            max_size: 6,
            party_players: vec![PartyPlayerData {
                id: 1,
                name: "Leader".into(),
                object_id: 100,
                unknown: 0,
            }],
            description: "Test Party".into(),
        });

        let result = router.route(&packet, &mut session);
        match result {
            RouteResult::Routed(events) => {
                assert_eq!(events.len(), 1);
                match &events[0] {
                    GameEvent::PartyListReceived(info) => {
                        assert_eq!(info.party_players.len(), 1);
                        assert_eq!(info.party_players[0].name, "Leader");
                    }
                    other => panic!("Expected PartyListReceived, got {:?}", other),
                }
            }
            RouteResult::Unmapped => panic!("Expected Routed"),
        }
    }

    // -- PartyMemberAdded --

    #[test]
    fn party_member_added_emits_party_member_joined() {
        let mut router = PacketRouter::new();
        let mut session = make_session();

        let packet = ParsedPacket::PartyMemberAdded(PartyMemberAddedPacket {
            player_id: 5,
            name: "NewPlayer".into(),
            class_id: 300,
            skin_id: 0,
        });

        let result = router.route(&packet, &mut session);
        match result {
            RouteResult::Routed(events) => {
                assert_eq!(events.len(), 1);
                match &events[0] {
                    GameEvent::PartyMemberJoined(member) => {
                        assert_eq!(member.name, "NewPlayer");
                        assert_eq!(member.player_id, 5);
                    }
                    other => panic!("Expected PartyMemberJoined, got {:?}", other),
                }
            }
            RouteResult::Unmapped => panic!("Expected Routed"),
        }
    }

    // -- MapInfo --

    #[test]
    fn map_info_emits_map_changed_and_updates_session() {
        let mut router = PacketRouter::new();
        let mut session = make_session();
        session.player.pet_object_id = Some(99);

        let packet = ParsedPacket::MapInfo(MapInfoPacket {
            width: 100,
            height: 100,
            name: "nexus".into(),
            display_name: "Nexus".into(),
            realm_name: String::new(),
            fp: 12345,
            background: 0,
            difficulty: 1.0,
            allow_player_teleport: true,
            no_save: false,
            show_displays: true,
            max_player_count: 25,
            game_opened_time: 0,
            version_number: "6.5.0".into(),
            view_distance: 15,
            dungeon_modifiers: String::new(),
            bg_color: 0,
            max_realm_score: 500,
            current_realm_score: 100,
        });

        let result = router.route(&packet, &mut session);
        match result {
            RouteResult::Routed(events) => {
                assert_eq!(events.len(), 1);
                match &events[0] {
                    GameEvent::MapChanged {
                        display_name,
                        max_realm_score,
                        allows_api,
                        ..
                    } => {
                        assert_eq!(display_name, "Nexus");
                        assert_eq!(*max_realm_score, 500);
                        assert!(!allows_api);
                    }
                    other => panic!("Expected MapChanged, got {:?}", other),
                }
            }
            RouteResult::Unmapped => panic!("Expected Routed"),
        }
        assert_eq!(session.map.name, Some("Nexus".to_string()));
        assert_eq!(session.map.max_realm_score, Some(500));
        // Pet cleared on map change
        assert_eq!(session.player.pet_object_id, None);
    }

    // -- Pet detection (NewTick fallback) --

    fn pet_stat(
        stat_type: crate::protocol::data::StatType,
        value: i32,
    ) -> crate::protocol::data::StatData {
        crate::protocol::data::StatData {
            stat_type_id: 0,
            stat_type,
            stat_value: value,
            string_stat_value: None,
            stat_value_two: 0,
        }
    }

    fn status_at(
        object_id: i32,
        x: f32,
        y: f32,
        stats: Vec<crate::protocol::data::StatData>,
    ) -> crate::protocol::data::ObjectStatusData {
        crate::protocol::data::ObjectStatusData {
            object_id,
            pos: crate::protocol::data::WorldPosData { x, y },
            stats,
        }
    }

    fn new_tick_with(statuses: Vec<crate::protocol::data::ObjectStatusData>) -> ParsedPacket {
        ParsedPacket::NewTick(NewTickPacket {
            tick_id: 1,
            tick_time: 200,
            server_realtime_ms: 0,
            server_last_rtt_ms: 0,
            statuses,
            unknown_byte: 0,
        })
    }

    #[test]
    fn newtick_latches_nearby_inventory_object() {
        use crate::protocol::data::StatType;
        // The equipped pet hugs the player and carries inventory slots, so a
        // nearby inventory-bearing object is attributed as the pet.
        let mut router = PacketRouter::new();
        let mut session = make_session(); // player object_id = 1

        let packet = new_tick_with(vec![
            status_at(1, 10.0, 10.0, vec![]),
            status_at(7, 10.4, 10.4, vec![pet_stat(StatType::Inventory0, 999)]),
        ]);
        router.route(&packet, &mut session);
        assert_eq!(session.player.pet_object_id, Some(7));
    }

    #[test]
    fn newtick_latches_when_player_position_unknown() {
        use crate::protocol::data::StatType;
        // With no player status this tick, proximity can't be checked, so an
        // inventory-bearing object is latched anyway to preserve live updates.
        let mut router = PacketRouter::new();
        let mut session = make_session();

        let packet = new_tick_with(vec![status_at(
            7,
            30.0,
            30.0,
            vec![pet_stat(StatType::Inventory0, 999)],
        )]);
        router.route(&packet, &mut session);
        assert_eq!(session.player.pet_object_id, Some(7));
    }

    #[test]
    fn newtick_skips_far_inventory_object() {
        use crate::protocol::data::StatType;
        // A known player position gates by proximity: an object more than ~3
        // tiles away is not latched.
        let mut router = PacketRouter::new();
        let mut session = make_session();

        let packet = new_tick_with(vec![
            status_at(1, 10.0, 10.0, vec![]),
            status_at(7, 30.0, 30.0, vec![pet_stat(StatType::Inventory0, 999)]),
        ]);
        router.route(&packet, &mut session);
        assert_eq!(session.player.pet_object_id, None);
    }

    #[test]
    fn newtick_skips_vault_chests() {
        use crate::protocol::data::StatType;
        // Vault storage objects carry inventory slots but must never be
        // mistaken for the pet.
        let mut router = PacketRouter::new();
        let mut session = make_session();
        session.vault.chest_object_id = Some(7);

        let packet = new_tick_with(vec![
            status_at(1, 10.0, 10.0, vec![]),
            status_at(7, 10.1, 10.1, vec![pet_stat(StatType::Inventory0, 999)]),
        ]);
        router.route(&packet, &mut session);
        assert_eq!(session.player.pet_object_id, None);
    }

    // -- VaultContent --

    #[test]
    fn vault_content_not_in_vault_emits_no_events() {
        let mut router = PacketRouter::new();
        let mut session = make_session();
        session.map.name = Some("Nexus".into());

        let packet = ParsedPacket::VaultContent(VaultContentPacket {
            last_vault_packet: true,
            vault_chest_object_id: 10,
            material_chest_object_id: 11,
            gift_chest_object_id: 12,
            potion_storage_object_id: 13,
            seasonal_spoil_chest_object_id: 14,
            vault_contents: vec![],
            material_contents: vec![],
            gift_contents: vec![],
            potion_contents: vec![],
            seasonal_spoil_contents: vec![],
            vault_upgrade_cost: 100,
            material_upgrade_cost: 50,
            potion_upgrade_cost: 75,
            current_potion_max: 16,
            next_potion_max: 24,
            unknown_short: 0,
            vault_chest_enchants: String::new(),
            gift_chest_enchants: String::new(),
            spoils_chest_enchants: String::new(),
        });

        let result = router.route(&packet, &mut session);
        match result {
            RouteResult::Routed(events) => assert!(events.is_empty()),
            RouteResult::Unmapped => panic!("Expected Routed"),
        }
    }

    #[test]
    fn vault_content_in_vault_emits_event_and_updates_session() {
        let mut router = PacketRouter::new();
        let mut session = make_session();
        session.map.name = Some("{s.vault}".into());

        let packet = ParsedPacket::VaultContent(VaultContentPacket {
            last_vault_packet: true,
            vault_chest_object_id: 10,
            material_chest_object_id: 11,
            gift_chest_object_id: 12,
            potion_storage_object_id: 13,
            seasonal_spoil_chest_object_id: 14,
            vault_contents: vec![100, 200],
            material_contents: vec![],
            gift_contents: vec![],
            potion_contents: vec![],
            seasonal_spoil_contents: vec![],
            vault_upgrade_cost: 100,
            material_upgrade_cost: 50,
            potion_upgrade_cost: 75,
            current_potion_max: 16,
            next_potion_max: 24,
            unknown_short: 0,
            vault_chest_enchants: String::new(),
            gift_chest_enchants: String::new(),
            spoils_chest_enchants: String::new(),
        });

        let result = router.route(&packet, &mut session);
        match result {
            RouteResult::Routed(events) => {
                assert_eq!(events.len(), 1);
                assert!(matches!(&events[0], GameEvent::VaultContentReceived { .. }));
            }
            RouteResult::Unmapped => panic!("Expected Routed"),
        }
        assert_eq!(session.vault.chest_object_id, Some(10));
        assert_eq!(session.vault.material_chest_object_id, Some(11));
        assert_eq!(session.vault.active_vault_page, Some(0));
    }

    // -- Text --

    #[test]
    fn text_emits_text_received() {
        let mut router = PacketRouter::new();
        let mut session = make_session();

        let packet = ParsedPacket::Text(TextPacket {
            name: "Player".into(),
            object_id: 1,
            num_stars: 50,
            bubble_time: 5,
            recipient: String::new(),
            text: "Hello world".into(),
            clean_text: String::new(),
            is_supporter: false,
            star_background: 0,
        });

        let result = router.route(&packet, &mut session);
        match result {
            RouteResult::Routed(events) => {
                assert_eq!(events.len(), 1);
                match &events[0] {
                    GameEvent::TextReceived(text) => {
                        assert_eq!(text.name, "Player");
                        assert_eq!(text.text, "Hello world");
                    }
                    other => panic!("Expected TextReceived, got {:?}", other),
                }
            }
            RouteResult::Unmapped => panic!("Expected Routed"),
        }
    }

    // -- Hello --

    #[test]
    fn hello_with_token_emits_hello_received() {
        let mut router = PacketRouter::new();
        let mut session = make_session();

        let packet = ParsedPacket::Hello(HelloPacket {
            game_id: -2,
            build_version: "1.0".into(),
            access_token: "my_token_12345".into(),
            key_time: 0,
            key: vec![],
            user_platform: "Steam".into(),
            play_platform: "Steam".into(),
            platform_token: String::new(),
            client_token: String::new(),
            user_token: String::new(),
        });

        let result = router.route(&packet, &mut session);
        match result {
            RouteResult::Routed(events) => {
                assert_eq!(events.len(), 1);
                match &events[0] {
                    GameEvent::HelloReceived { access_token } => {
                        assert_eq!(access_token, "my_token_12345");
                    }
                    other => panic!("Expected HelloReceived, got {:?}", other),
                }
            }
            RouteResult::Unmapped => panic!("Expected Routed"),
        }
    }

    #[test]
    fn hello_without_token_emits_no_events() {
        let mut router = PacketRouter::new();
        let mut session = make_session();

        let packet = ParsedPacket::Hello(HelloPacket {
            game_id: -2,
            build_version: "1.0".into(),
            access_token: String::new(),
            key_time: 0,
            key: vec![],
            user_platform: "Steam".into(),
            play_platform: "Steam".into(),
            platform_token: String::new(),
            client_token: String::new(),
            user_token: String::new(),
        });

        let result = router.route(&packet, &mut session);
        match result {
            RouteResult::Routed(events) => assert!(events.is_empty()),
            RouteResult::Unmapped => panic!("Expected Routed"),
        }
    }

    // -- InvSwap --

    #[test]
    fn inv_swap_emits_inv_swap_received() {
        let mut router = PacketRouter::new();
        let mut session = make_session();

        let packet = ParsedPacket::InvSwap(InvSwapPacket {
            time: 100,
            player_x: 1.0,
            player_y: 2.0,
            slot_from: SlotObjectData {
                object_id: 10,
                slot_id: 0,
                item_type: 500,
            },
            slot_to: SlotObjectData {
                object_id: 20,
                slot_id: 3,
                item_type: -1,
            },
        });

        let result = router.route(&packet, &mut session);
        match result {
            RouteResult::Routed(events) => {
                assert_eq!(events.len(), 1);
                assert!(matches!(&events[0], GameEvent::InvSwapReceived(_)));
            }
            RouteResult::Unmapped => panic!("Expected Routed"),
        }
    }

    // -- RouteResult helpers --

    #[test]
    fn route_result_is_routed() {
        assert!(RouteResult::Routed(vec![]).is_routed());
        assert!(!RouteResult::Unmapped.is_routed());
    }
}

//! Combat History reconstruction module.
//!
//! Rebuilds boss fights from the live packet stream: fight boundary detection,
//! boss HP trajectory, participant roster + equipment, and per-player damage
//! attribution, based on the data observable from a single client's traffic.
//!
//! Damage attribution uses the two signal paths a single client receives:
//! other players' damage from incoming `DamagePacket`s (accurate while visible),
//! and the local player's hit count from outgoing `EnemyHit` (own damage value
//! is filled by a later shot-simulation change and is tagged `SelfPending`).

pub(crate) mod damage;
mod database;
pub mod export;
mod manager;
mod tracker;
mod types;

pub use database::{
    CombatDatabase, DungeonTimeTotal, EncounterRecord, FightQuery, FightRecord, FightSummary,
    ParticipantRecord, LOOT_LINK_POST_MS, LOOT_LINK_PRE_MS, REQUIRED_TABLES,
    SUPPORTED_SCHEMA_VERSION,
};
pub use export::{build_bundle, ExportBundle};
pub use manager::CombatManager;
pub use tracker::{is_groupable_dungeon, normalize_dungeon, sanitize_player_name, CombatTracker};
pub use types::{
    CompletedFight, DamageProvenance, DamageTakenProvenance, FightParticipant, FightSelection,
    ParticipantEndStatus, PetInfo,
};

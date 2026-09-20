//! RealmHound Core Library
//!
//! This crate provides the core functionality for capturing and processing
//! network packets from Realm of the Mad God.
//!
//! # Architecture
//!
//! The library is organized into several modules:
//!
//! - `api` - RotMG web API client for fetching character data
//! - `assets` - Game asset management (object definitions, sprites)
//! - `capture` - Network packet capture using Npcap/pcap
//! - `crypto` - RC4 encryption/decryption
//! - `loot` - Loot detection, tracking, and history database
//! - `party_cache` - In-memory player class/skin/guild sightings for the Party panel
//! - `protocol` - RotMG packet definitions and parsing
//! - `router` - Packet routing and game event generation
//! - `session` - Central game session state model
//! - `settings` - User-configurable settings with persistence
//! - `stats` - Player stats, inventory parsing, bandwidth tracking
//! - `storage` - Validated storage roots and recoverable document persistence
//! - `stream` - TCP stream reassembly
//! - `update` - Version checking and update notifications
//! - `vault` - Vault/character cache and live vault data
//! - `watchlist` - Player watchlist (whitelist/blacklist)
//!
//! # Usage
//!
//! Import types from their specific modules for clarity:
//!
//! ```no_run
//! use realmhound_core::capture::{NetworkInterface, Sniffer, SnifferConfig};
//! use realmhound_core::protocol::{parse_packet, ParsedPacket};
//! use realmhound_core::session::GameSession;
//! ```

pub mod account;
pub mod account_stats;
pub mod api;
pub mod assets;
pub mod capture;
pub mod combat;
pub mod crypto;
pub mod dungeon_modifiers;
pub mod dust;
pub mod keyper;
pub mod loot;
pub mod party_cache;
pub mod profiling;
pub mod protocol;
pub mod realmshark_import;
pub mod relaunch;
pub mod router;
pub mod season;
pub mod session;
pub mod settings;
pub mod stats;
pub mod storage;
pub mod stream;
pub mod update;
pub mod vault;
pub mod watchlist;

// Re-export key entry points used across the application.
// For other types, use qualified module paths (e.g., `realmhound_core::loot::LootTracker`).
pub use router::{GameEvent, PacketRouter, RouteResult};
pub use session::GameSession;
pub use settings::Settings;

/// Re-export puffin at the crate root so the `prof_scope!` / `prof_function!`
/// macros resolve it via `$crate::__puffin` from any dependent crate.
#[cfg(feature = "profiling")]
#[doc(hidden)]
pub use puffin as __puffin;
